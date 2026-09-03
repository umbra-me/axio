//! Several agents on one prompt.
//!
//! A group is the smallest thing that makes "many agents at once" a workflow
//! rather than a list: one prompt, N sessions, one id they all carry, and a
//! view that lays their results side by side. Nothing is pooled beyond the id
//! — each member has its own worktree, branch, approvals and transcript, and is
//! closed on its own — so a group costs the supervisor nothing new.

use crate::hosted::StartHostedInput;
use crate::model::{
    AddToGroupInput, AppError, GroupStart, Isolation, StartGroupInput, StartSessionInput,
};

use super::AppState;

impl AppState {
    /// Start `count` axio sessions and one hosted terminal per agent named,
    /// all on the same prompt and repository, tagged with one fresh group id.
    ///
    /// Members start in order and a failure stops the rest: a group of five
    /// where the fourth could not cut its worktree is three sessions and an
    /// error, which is what the interface then shows, rather than a pretence
    /// of five.
    pub async fn start_group(
        &self,
        input: StartGroupInput,
        on_hosted_activity: impl Fn(String) + Send + Clone + 'static,
    ) -> Result<GroupStart, AppError> {
        let prompt = input.prompt.trim().to_owned();
        if prompt.is_empty() {
            return Err(AppError::NoRepository("a group needs a prompt".to_owned()));
        }
        if input.count == 0 && input.agents.is_empty() {
            return Err(AppError::NoRepository(
                "a group needs at least one session or one agent".to_owned(),
            ));
        }
        let group = ulid::Ulid::generate().to_string().to_lowercase();
        let mut out = GroupStart {
            group: group.clone(),
            sessions: Vec::new(),
            terminals: Vec::new(),
        };

        for _ in 0..input.count {
            let session = self
                .start_session(StartSessionInput {
                    path: input.path.clone(),
                    prompt: Some(prompt.clone()),
                    isolation: Some(Isolation::Worktree),
                    group: Some(group.clone()),
                })
                .await?;
            out.sessions.push(session);
        }

        for harness in &input.agents {
            let terminal = self
                .start_hosted(
                    StartHostedInput {
                        harness: harness.clone(),
                        cwd: input.path.clone(),
                        isolation: Some(Isolation::Worktree),
                        group: Some(group.clone()),
                        args: String::new(),
                        rows: None,
                        cols: None,
                    },
                    on_hosted_activity.clone(),
                )
                .await?;
            // The prompt reaches a hosted agent as its first line. It is what
            // the person would have typed; typing it for them is the whole
            // point of asking once for several — once the agent is ready to
            // be typed at, which is not the moment it was spawned.
            self.hosted
                .submit_when_ready(&terminal.id, prompt.clone())?;
            out.terminals.push(terminal);
        }

        Ok(out)
    }

    /// One more member for a group already running: an axio session, or a
    /// hosted agent, in a fresh worktree, asked the group's prompt.
    pub async fn add_to_group(
        &self,
        input: AddToGroupInput,
        on_hosted_activity: impl Fn(String) + Send + 'static,
    ) -> Result<GroupStart, AppError> {
        let prompt = input
            .prompt
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty());
        let mut out = GroupStart {
            group: input.group.clone(),
            sessions: Vec::new(),
            terminals: Vec::new(),
        };
        match input.harness {
            None => {
                let session = self
                    .start_session(StartSessionInput {
                        path: input.path,
                        prompt,
                        isolation: Some(Isolation::Worktree),
                        group: Some(input.group),
                    })
                    .await?;
                out.sessions.push(session);
            }
            Some(harness) => {
                let terminal = self
                    .start_hosted(
                        StartHostedInput {
                            harness,
                            cwd: input.path,
                            isolation: Some(Isolation::Worktree),
                            group: Some(input.group),
                            args: String::new(),
                            rows: None,
                            cols: None,
                        },
                        on_hosted_activity,
                    )
                    .await?;
                if let Some(prompt) = prompt {
                    self.hosted.submit_when_ready(&terminal.id, prompt)?;
                }
                out.terminals.push(terminal);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_empty_group_is_refused_before_anything_starts() {
        let state = AppState::unavailable("no supervisor in this test");
        let err = state
            .start_group(
                StartGroupInput {
                    path: ".".into(),
                    prompt: "do the thing".into(),
                    count: 0,
                    agents: Vec::new(),
                },
                |_| {},
            )
            .await
            .expect_err("nothing to start");
        assert!(err.to_string().contains("at least one"));

        let err = state
            .start_group(
                StartGroupInput {
                    path: ".".into(),
                    prompt: "   ".into(),
                    count: 2,
                    agents: Vec::new(),
                },
                |_| {},
            )
            .await
            .expect_err("no prompt");
        assert!(err.to_string().contains("prompt"));
    }
}
