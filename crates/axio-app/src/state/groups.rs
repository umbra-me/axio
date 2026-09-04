//! Several agents on one prompt.
//!
//! A group is the smallest thing that makes "many agents at once" a workflow
//! rather than a list: one prompt, N sessions, one id they all carry, and a
//! view that lays their results side by side. Nothing is pooled beyond the id
//! — each member has its own worktree, branch, approvals and transcript, and is
//! closed on its own — so a group costs the supervisor nothing new.

use axio_pty::Harness;

use crate::hosted::StartHostedInput;
use crate::model::{
    AddToGroupInput, AppError, GroupMember, GroupStart, Isolation, StartGroupInput,
    StartSessionInput,
};

use super::AppState;

impl AppState {
    /// Start `count` axio sessions and one hosted terminal per agent named,
    /// all on the same prompt and repository, tagged with one fresh group id.
    /// With no prompt they start and wait: a session without a first turn,
    /// a terminal nobody has typed at.
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
        let prompt = Some(input.prompt.trim().to_owned()).filter(|p| !p.is_empty());
        // The short form — a count and a list of names — is the long form
        // with nothing set on each member.
        let members: Vec<GroupMember> = if input.members.is_empty() {
            (0..input.count)
                .map(|_| GroupMember::default())
                .chain(input.agents.iter().map(|h| GroupMember {
                    harness: Some(h.clone()),
                    ..GroupMember::default()
                }))
                .collect()
        } else {
            input.members.clone()
        };
        if members.is_empty() {
            return Err(AppError::NoRepository(
                "a group needs at least one session or one agent".to_owned(),
            ));
        }
        let group = ulid::Ulid::generate().to_string().to_lowercase();
        let mut out = GroupStart {
            group: group.clone(),
            sessions: Vec::new(),
            terminals: Vec::new(),
            order: Vec::new(),
        };

        for member in &members {
            match member.harness.as_deref().filter(|h| *h != "axio-session") {
                None => {
                    let session = self
                        .start_session(StartSessionInput {
                            path: input.path.clone(),
                            prompt: prompt.clone(),
                            isolation: Some(Isolation::Worktree),
                            group: Some(group.clone()),
                        })
                        .await?;
                    out.order.push(session.id.clone());
                    out.sessions.push(session);
                }
                Some(harness) => {
                    // The prompt goes on the command line where the tool
                    // takes one there, and is typed in once the interface
                    // is up where it does not — the command line first,
                    // because there is nothing to wait for and nothing to
                    // mistake for a paste.
                    let structured = member.transport.as_deref() == Some("app");
                    let on_command_line = structured
                        || prompt.as_deref().is_some_and(|p| {
                            Harness::parse(harness).is_some_and(|h| h.prompt_args(p).is_some())
                        });
                    let terminal = self
                        .start_hosted(
                            StartHostedInput {
                                harness: harness.to_owned(),
                                cwd: input.path.clone(),
                                isolation: Some(Isolation::Worktree),
                                group: Some(group.clone()),
                                args: if structured {
                                    member.args.clone()
                                } else {
                                    member_args(harness, member)
                                },
                                prompt: on_command_line.then(|| prompt.clone()).flatten(),
                                transport: member.transport.clone(),
                                model: member.model.clone(),
                                effort: member.effort.clone(),
                                permission: member.permission.clone(),
                                rows: None,
                                cols: None,
                            },
                            on_hosted_activity.clone(),
                        )
                        .await?;
                    if let Some(prompt) = &prompt
                        && !on_command_line
                    {
                        self.hosted
                            .submit_when_ready(&terminal.id, prompt.clone())?;
                    }
                    out.order.push(terminal.id.clone());
                    out.terminals.push(terminal);
                }
            }
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
            order: Vec::new(),
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
                            prompt: None,
                            transport: None,
                            model: None,
                            effort: None,
                            permission: None,
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

/// A member's command line: its model, its effort where the tool takes one,
/// then whatever else it asked for. Quoted as a shell would read it, since
/// that is how the hosted start splits it again.
fn member_args(harness: &str, member: &GroupMember) -> String {
    let Some(h) = Harness::parse(harness) else {
        return member.args.clone();
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = member
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        parts.extend(h.model_args(model));
    }
    if let Some(effort) = member
        .effort
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty())
    {
        parts.extend(h.effort_args(effort));
    }
    if let Some(mode) = member
        .permission
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        parts.extend(h.permission_args(mode));
    }
    let mut line: Vec<String> = parts
        .into_iter()
        .map(|p| {
            if p.chars().any(|c| c.is_whitespace() || c == '"') {
                format!("'{}'", p.replace('\'', "'\\''"))
            } else {
                p
            }
        })
        .collect();
    if !member.args.trim().is_empty() {
        line.push(member.args.trim().to_owned());
    }
    line.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_line_carries_model_then_effort_then_its_own_args() {
        let m = GroupMember {
            harness: Some("codex".into()),
            model: Some("gpt-5.4".into()),
            effort: Some("high".into()),
            permission: Some("edits".into()),
            transport: None,
            args: "--search".into(),
        };
        assert_eq!(
            member_args("codex", &m),
            "-m gpt-5.4 -c 'model_reasoning_effort=\"high\"' -a on-request -s workspace-write --search"
        );
        // Claude Code has a model flag and no effort flag: the effort is
        // dropped, not passed as something it would refuse.
        let m = GroupMember {
            harness: Some("claude".into()),
            model: Some("opus".into()),
            effort: Some("high".into()),
            permission: Some("full".into()),
            transport: None,
            args: String::new(),
        };
        assert_eq!(
            member_args("claude", &m),
            "--model opus --dangerously-skip-permissions"
        );
        assert_eq!(member_args("claude", &GroupMember::default()), "");
    }

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
                    members: Vec::new(),
                },
                |_| {},
            )
            .await
            .expect_err("nothing to start");
        assert!(err.to_string().contains("at least one"));

        // No prompt is not the same refusal: the members would start and
        // wait. Here there is no supervisor, so it is the supervisor's error.
        let err = state
            .start_group(
                StartGroupInput {
                    path: ".".into(),
                    prompt: "   ".into(),
                    count: 2,
                    agents: Vec::new(),
                    members: Vec::new(),
                },
                |_| {},
            )
            .await
            .expect_err("no supervisor");
        assert!(!err.to_string().contains("prompt"), "{err}");
    }
}
