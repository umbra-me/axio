//! Many sessions, many repositories, one place to watch them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axio_core::config::WorktreeSection;
use axio_core::protocol::{ApprovalId, Decision, Event, Notice, SessionId};
use tokio::sync::mpsc;
use ulid::Ulid;

use crate::approval::{ApprovalQueue, PendingApproval, QueueApprover, SessionSlot};
use crate::error::{Result, SupervisorError};
use crate::factory::{AgentFactory, AgentRequest};
use crate::index::{IndexEntry, SessionIndex};
use crate::project::{Project, ProjectId, Projects};
use crate::session::{self, SessionHandle};
use crate::worktree::{Checkout, Disposition, Isolation};

/// An event, and which project it came from.
///
/// `Event` already carries its session. The project is the one thing a
/// multi-repository surface needs and cannot derive: a worktree's path is not
/// its repository's, which is the same reason the index exists.
#[derive(Debug, Clone)]
pub struct SupervisedEvent {
    pub project: ProjectId,
    pub event: Event,
}

/// Where the supervisor keeps its own state.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// Worktrees and the session index live under here. Outside every
    /// repository, deliberately: a worktree inside the repo would show up in
    /// its own `git status`, and an agent could read the work of every sibling
    /// session through a path its `Workspace` never had to leave.
    pub state_root: PathBuf,
    /// Resolved from `[worktree]`. Isolation is on unless a *user* turned it
    /// off — a project config that tries is refused in `axio_core::config`.
    pub worktree: WorktreeSection,
}

/// How one session starts.
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Override the configured isolation for this session alone.
    ///
    /// The escape hatch for the case configuration cannot express: a repository
    /// whose build needs untracked files that a fresh worktree does not have.
    pub isolation: Option<Isolation>,
    /// Continue an existing session instead of starting one.
    pub resume: Option<SessionId>,
    /// The first prompt, for the session header's label.
    pub label: Option<String>,
    /// Replayed onto this session's event stream after `SessionStarted`.
    pub notices: Vec<Notice>,
}

pub struct Supervisor {
    config: SupervisorConfig,
    factory: Arc<dyn AgentFactory>,
    approvals: Arc<ApprovalQueue>,
    index: Mutex<SessionIndex>,
    live: Mutex<BTreeMap<SessionId, SessionHandle>>,
    projects: Mutex<Projects>,
    events: mpsc::UnboundedSender<SupervisedEvent>,
}

impl Supervisor {
    /// Read the index, adopt whatever projects it names, and hand back the
    /// merged event stream.
    ///
    /// Unbounded for the same reason the agent's own channel is: a bounded send
    /// from inside a turn can deadlock against a surface that is slow to drain,
    /// and here there are several turns that could do it at once.
    pub fn new(
        config: SupervisorConfig,
        factory: Arc<dyn AgentFactory>,
    ) -> Result<(Self, mpsc::UnboundedReceiver<SupervisedEvent>)> {
        let index = SessionIndex::open(config.state_root.join("index.jsonl"))?;
        let projects = index.projects();
        let (tx, rx) = mpsc::unbounded_channel();
        Ok((
            Self {
                config,
                factory,
                approvals: Arc::new(ApprovalQueue::default()),
                index: Mutex::new(index),
                live: Mutex::new(BTreeMap::new()),
                projects: Mutex::new(projects),
                events: tx,
            },
            rx,
        ))
    }

    /// Register the repository containing `path` without starting anything.
    pub async fn open_project(&self, path: &Path) -> Result<Project> {
        let project = Project::open(path).await?;
        Ok(self.lock_projects().insert(project))
    }

    /// Start a session on the repository containing `path`.
    ///
    /// Isolation failing is an error, never a downgrade. Falling back to the
    /// live checkout would hand an agent write access to the files you are
    /// using, silently, at the moment isolation was most clearly wanted — so
    /// the caller is told, and can offer [`Isolation::Direct`] as a choice
    /// someone actually makes.
    pub async fn start(&self, path: &Path, options: StartOptions) -> Result<SessionHandle> {
        let project = self.open_project(path).await?;

        let isolation = options
            .isolation
            .unwrap_or(if self.config.worktree.enabled {
                Isolation::Worktree
            } else {
                Isolation::Direct
            });

        let checkout = match isolation {
            Isolation::Direct => Checkout::direct(&project),
            Isolation::Worktree => {
                Checkout::worktree(
                    &project,
                    &self.config.state_root.join("worktrees"),
                    Ulid::generate(),
                    &self.config.worktree.branch_prefix,
                )
                .await?
            }
        };

        match self.build(&project, checkout.clone(), &options).await {
            Ok(handle) => Ok(handle),
            Err(e) => {
                // A worktree cut for a session that never started is litter, and
                // the branch would collide with nothing but still be there.
                let _ = checkout.close(Disposition::Discard).await;
                Err(e)
            }
        }
    }

    async fn build(
        &self,
        project: &Project,
        checkout: Checkout,
        options: &StartOptions,
    ) -> Result<SessionHandle> {
        let slot = SessionSlot::default();
        let approver = Arc::new(QueueApprover::new(
            Arc::clone(&self.approvals),
            slot.clone(),
            project.id.clone(),
        ));

        let (tx, rx) = mpsc::unbounded_channel();
        let agent = self
            .factory
            .build(AgentRequest {
                project: project.clone(),
                checkout: checkout.clone(),
                approver,
                events: tx,
                resume: options.resume,
                label: options.label.clone(),
            })
            .await
            .map_err(SupervisorError::Factory)?;

        let session = agent.session_id();
        // Before the task starts, so no approval can be registered against a
        // slot that is still empty.
        slot.set(session);
        self.forward(project.id.clone(), rx);

        self.lock_index().record_started(IndexEntry {
            session,
            project: project.id.clone(),
            project_root: project.root.clone(),
            project_name: project.name.clone(),
            workspace: checkout.path.clone(),
            branch: checkout.branch.clone(),
            isolation: checkout.isolation,
            label: options.label.clone(),
            started_ms: crate::approval::now_ms(),
            closed_ms: None,
            discarded: false,
        })?;

        let handle = session::spawn(
            agent,
            project.id.clone(),
            checkout,
            options.resume.is_some(),
            options.notices.clone(),
        );
        self.lock_live().insert(session, handle.clone());
        Ok(handle)
    }

    /// Re-label every event with its project and merge it into one stream.
    fn forward(&self, project: ProjectId, mut rx: mpsc::UnboundedReceiver<Event>) {
        let out = self.events.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // A closed receiver means the surface went away. Every session
                // still has to finish cleanly, so this ends the forwarder and
                // nothing else.
                if out
                    .send(SupervisedEvent {
                        project: project.clone(),
                        event,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
    }

    /// End a session and decide what happens to its worktree.
    ///
    /// Order matters and is the whole method. Pending approvals are answered
    /// first, or the task is parked inside `decide` and never reads the close;
    /// then the turn is cancelled and the task joined; only then is the
    /// worktree touched, because removing a checkout out from under a running
    /// tool is how a session ends in a way nobody can explain afterwards.
    pub async fn close(&self, session: SessionId, disposition: Disposition) -> Result<()> {
        let handle = self
            .lock_live()
            .remove(&session)
            .ok_or(SupervisorError::NoSuchSession(session))?;

        self.approvals.close_session(session);
        handle.close().await?;
        handle.checkout.close(disposition).await?;
        self.lock_index()
            .record_closed(session, disposition == Disposition::Discard)?;
        Ok(())
    }

    /// Interrupt whatever every session is doing, without closing any of them.
    pub fn cancel_all(&self) {
        for handle in self.lock_live().values() {
            handle.cancel();
        }
    }

    pub fn session(&self, session: SessionId) -> Result<SessionHandle> {
        self.lock_live()
            .get(&session)
            .cloned()
            .ok_or(SupervisorError::NoSuchSession(session))
    }

    /// Every live session, oldest first.
    pub fn sessions(&self) -> Vec<SessionHandle> {
        self.lock_live().values().cloned().collect()
    }

    pub fn sessions_for(&self, project: &ProjectId) -> Vec<SessionHandle> {
        self.sessions()
            .into_iter()
            .filter(|h| &h.project == project)
            .collect()
    }

    /// Every project with work, live or historical.
    pub fn projects(&self) -> Vec<Project> {
        self.lock_projects().all()
    }

    /// Everything the index knows, including sessions that are not running.
    pub fn history(&self) -> Vec<IndexEntry> {
        self.lock_index().all()
    }

    pub fn history_for(&self, project: &ProjectId) -> Vec<IndexEntry> {
        self.lock_index().for_project(project)
    }

    pub fn pending_approvals(&self) -> Vec<PendingApproval> {
        self.approvals.pending()
    }

    /// Answer one question. `false` if it was already answered.
    pub fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> bool {
        self.approvals.resolve(id, decision)
    }

    fn lock_live(&self) -> std::sync::MutexGuard<'_, BTreeMap<SessionId, SessionHandle>> {
        self.live
            .lock()
            .expect("no supervisor lock is held across an await")
    }

    fn lock_index(&self) -> std::sync::MutexGuard<'_, SessionIndex> {
        self.index
            .lock()
            .expect("no supervisor lock is held across an await")
    }

    fn lock_projects(&self) -> std::sync::MutexGuard<'_, Projects> {
        self.projects
            .lock()
            .expect("no supervisor lock is held across an await")
    }
}

/// Sessions run on detached tasks and outlive the supervisor that started them.
///
/// That is deliberate — a turn must finish cleanly rather than be torn down
/// mid-tool-call — but it leaves one way to hang: a task that asks for approval
/// after the last surface has gone waits for an answer nobody can give. The
/// queue cannot be dropped to solve it, because every session's approver holds
/// an `Arc` to it. So it is told, and it answers.
///
/// Worktrees are deliberately left alone here. Removing one needs to await git,
/// which `Drop` cannot do, and it is the right default anyway: a supervisor
/// going away is not a decision to throw away the work in five branches. They
/// are in the index, and the next run finds them.
impl Drop for Supervisor {
    fn drop(&mut self) {
        self.approvals.shutdown();
        for handle in self.lock_live().values() {
            handle.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Repo, ScriptedFactory};
    use axio_core::protocol::{EventKind, TurnOutcome};

    fn supervisor(
        state: &Path,
        factory: Arc<dyn AgentFactory>,
    ) -> (Supervisor, mpsc::UnboundedReceiver<SupervisedEvent>) {
        Supervisor::new(
            SupervisorConfig {
                state_root: state.to_path_buf(),
                worktree: WorktreeSection::default(),
            },
            factory,
        )
        .expect("a supervisor")
    }

    fn state() -> tempfile::TempDir {
        tempfile::tempdir().expect("a state dir")
    }

    #[tokio::test]
    async fn a_session_works_in_its_own_worktree_and_never_in_the_repository() {
        let repo = Repo::new().await;
        let state = state();
        let factory = Arc::new(ScriptedFactory::default());
        let (sup, _events) = supervisor(state.path(), factory.clone());

        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .expect("a session");

        let workspace = &handle.checkout.path;
        assert!(workspace.join(".git").exists(), "a real worktree");
        assert!(
            workspace.starts_with(state.path()),
            "{} is not under the state directory",
            workspace.display()
        );
        assert_ne!(
            workspace.canonicalize().ok(),
            repo.path().canonicalize().ok(),
            "the agent must not be in the checkout someone is using"
        );

        let branch = handle.checkout.branch.clone().expect("a branch");
        assert!(branch.starts_with("axio/"));
        assert!(repo.has_branch(&branch).await);
        // And the factory was handed that path, not the repository's.
        assert_eq!(factory.workspaces(), vec![workspace.clone()]);
    }

    #[tokio::test]
    async fn two_sessions_on_one_repository_never_share_a_worktree() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let first = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();
        let second = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();

        assert_ne!(first.checkout.path, second.checkout.path);
        assert_ne!(first.checkout.branch, second.checkout.branch);
        assert_eq!(sup.sessions().len(), 2);
        assert_eq!(sup.projects().len(), 1, "one repository, one project");
    }

    /// Isolation that silently did not happen is the failure nobody notices, so
    /// direct mode has to be asked for.
    #[tokio::test]
    async fn direct_isolation_is_chosen_and_uses_the_repository_itself() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let handle = sup
            .start(
                repo.path(),
                StartOptions {
                    isolation: Some(Isolation::Direct),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(handle.checkout.isolation, Isolation::Direct);
        assert!(handle.checkout.branch.is_none());
        assert_eq!(
            handle.checkout.path.canonicalize().ok(),
            repo.path().canonicalize().ok()
        );
    }

    #[tokio::test]
    async fn a_repository_with_no_commits_says_so_rather_than_falling_back() {
        let repo = Repo::empty().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        match sup.start(repo.path(), StartOptions::default()).await {
            Err(SupervisorError::NoCommits(_)) => {}
            other => panic!("expected NoCommits, got {other:?}"),
        }
        assert!(sup.sessions().is_empty(), "nothing half-started");
    }

    #[tokio::test]
    async fn a_directory_outside_a_repository_is_refused() {
        let outside = tempfile::tempdir().unwrap();
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        match sup.start(outside.path(), StartOptions::default()).await {
            Err(SupervisorError::NotARepository(_)) => {}
            other => panic!("expected NotARepository, got {other:?}"),
        }
    }

    /// A worktree cut for a session that never started is litter, and its
    /// branch would sit in the repository forever.
    #[tokio::test]
    async fn a_factory_failure_leaves_no_worktree_or_branch_behind() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(
            state.path(),
            Arc::new(ScriptedFactory::failing("no credential")),
        );

        match sup.start(repo.path(), StartOptions::default()).await {
            Err(SupervisorError::Factory(message)) => assert!(message.contains("no credential")),
            other => panic!("expected a factory error, got {other:?}"),
        }

        let branches = repo.git(&["branch", "--list", "axio/*"]).await;
        assert!(branches.is_empty(), "a branch survived: {branches}");
        let worktrees = repo.git(&["worktree", "list"]).await;
        assert_eq!(worktrees.lines().count(), 1, "{worktrees}");
    }

    #[tokio::test]
    async fn closing_with_keep_leaves_the_branch_alone() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();
        let branch = handle.checkout.branch.clone().unwrap();
        let path = handle.checkout.path.clone();
        sup.close(handle.session, Disposition::Keep).await.unwrap();

        assert!(repo.has_branch(&branch).await, "the work must survive");
        // Regression: keeping used to delete the worktree and spare only the
        // branch, so the CLI's "run `session diff`" pointed at a directory it
        // had just removed. Reviewing means opening the checkout.
        assert!(
            path.exists(),
            "the worktree someone will review must survive"
        );
        assert!(sup.sessions().is_empty());
        // The index still knows it existed, and that it is closed.
        let entry = sup
            .history()
            .into_iter()
            .find(|e| e.session == handle.session)
            .expect("the index remembers");
        assert!(!entry.is_open());
        assert!(!entry.discarded);
    }

    #[tokio::test]
    async fn closing_with_discard_removes_the_worktree_and_the_branch() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();
        let branch = handle.checkout.branch.clone().unwrap();
        let path = handle.checkout.path.clone();
        sup.close(handle.session, Disposition::Discard)
            .await
            .unwrap();

        assert!(!repo.has_branch(&branch).await);
        assert!(!path.exists(), "the worktree directory is gone");
    }

    /// Closing a window is not consent to delete an afternoon's work.
    #[tokio::test]
    async fn discarding_refuses_while_the_branch_holds_commits() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();
        let work = &handle.checkout.path;
        std::fs::write(work.join("new.txt"), "an afternoon\n").unwrap();
        crate::git::run(work, &["add", "."]).await.unwrap();
        crate::git::run(work, &["commit", "-m", "work"])
            .await
            .unwrap();

        let branch = handle.checkout.branch.clone().unwrap();
        match sup.close(handle.session, Disposition::Discard).await {
            Err(SupervisorError::Git { message, .. }) => {
                assert!(message.contains("nowhere else"), "{message}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(repo.has_branch(&branch).await, "the work survived");
    }

    #[tokio::test]
    async fn sessions_are_grouped_by_repository_across_several_of_them() {
        let (one, two) = (Repo::new().await, Repo::new().await);
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));

        let a = sup
            .start(one.path(), StartOptions::default())
            .await
            .unwrap();
        let b = sup
            .start(one.path(), StartOptions::default())
            .await
            .unwrap();
        let c = sup
            .start(two.path(), StartOptions::default())
            .await
            .unwrap();

        assert_eq!(sup.projects().len(), 2);
        assert_eq!(sup.sessions_for(&a.project).len(), 2);
        assert_eq!(sup.sessions_for(&c.project).len(), 1);
        assert_eq!(a.project, b.project);
        assert_ne!(a.project, c.project);
        assert_eq!(sup.history_for(&c.project).len(), 1);
    }

    /// The index is what a restart reads, so what it holds has to be the truth
    /// about a repository rather than about a worktree.
    #[tokio::test]
    async fn the_index_records_the_repository_not_the_worktree() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));
        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();

        let entry = sup.history().into_iter().next().expect("one entry");
        assert_eq!(entry.session, handle.session);
        assert_eq!(
            entry.project_root.canonicalize().ok(),
            repo.path().canonicalize().ok(),
            "the index must point at the repository"
        );
        assert_eq!(entry.workspace, handle.checkout.path);
        assert_eq!(entry.isolation, Isolation::Worktree);

        // And a fresh supervisor over the same state finds the project again
        // without anyone re-registering it.
        let (restarted, _) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));
        assert_eq!(restarted.projects().len(), 1);
        assert_eq!(restarted.history().len(), 1);
        assert!(restarted.sessions().is_empty(), "nothing is running yet");
    }

    #[tokio::test]
    async fn a_turn_runs_and_its_events_carry_their_project() {
        let repo = Repo::new().await;
        let state = state();
        let (sup, mut events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));
        let handle = sup
            .start(repo.path(), StartOptions::default())
            .await
            .unwrap();

        let outcome = handle.turn("do a thing").await.unwrap();
        assert!(matches!(outcome, TurnOutcome::Completed), "{outcome:?}");

        let mut started = false;
        let mut ended = false;
        while let Ok(event) = events.try_recv() {
            assert_eq!(event.project, handle.project, "every event is labelled");
            assert_eq!(event.event.session, handle.session);
            match event.event.kind {
                EventKind::SessionStarted { .. } => started = true,
                EventKind::TurnEnded { .. } => ended = true,
                _ => {}
            }
        }
        assert!(
            started,
            "SessionStarted must be the first thing a surface sees"
        );
        assert!(ended, "a turn always ends");
    }

    #[tokio::test]
    async fn closing_a_session_that_is_not_running_is_an_error_not_a_panic() {
        let state = state();
        let (sup, _events) = supervisor(state.path(), Arc::new(ScriptedFactory::default()));
        match sup.close(SessionId::generate(), Disposition::Keep).await {
            Err(SupervisorError::NoSuchSession(_)) => {}
            other => panic!("expected NoSuchSession, got {other:?}"),
        }
    }
}
