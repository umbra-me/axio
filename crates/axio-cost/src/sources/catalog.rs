//! The agents covered by the table-driven parser, and where each keeps its sessions.
//!
//! Paths were taken from the reference implementation named in `NOTICE` and checked
//! against this machine where the agent is installed. An entry whose directory does not
//! exist costs nothing — [`super::scan`] reports it as *not installed*, which is a
//! different answer from *recorded nothing* and is what `--diagnose` prints.
//!
//! Adding an agent is a row here. That is the point of [`GenericAgent`]: these formats all
//! log the same event and differ only in where they put it, so a bespoke module per agent
//! would be twenty copies of one bug.

use super::generic::{GenericAgent, Matcher};
use super::usage::Convention;

/// Every table-driven agent, in a stable order so output does not reshuffle between runs.
pub const CATALOG: &[GenericAgent] = &[
    GenericAgent {
        client: "gemini",
        display_name: "Gemini CLI",
        // Sessions live under a per-workspace hash: `.gemini/tmp/<hash>/chats/*.jsonl`.
        // The root is the parent of all of them because the walk recurses anyway, and a
        // hash this crate would have to compute is a hash it could compute wrongly.
        roots: &[".gemini/tmp"],
        matcher: Matcher::Extension("jsonl"),
        // Gemini relays an OpenAI-shaped usage block: `prompt_tokens` with `cached_tokens`
        // inside it.
        convention: Convention::CacheInclusive,
    },
    GenericAgent {
        client: "qwen",
        display_name: "Qwen Code",
        roots: &[".qwen/projects"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::CacheInclusive,
    },
    GenericAgent {
        client: "pi",
        display_name: "Pi",
        roots: &[".pi/agent/sessions"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "kiro",
        display_name: "Kiro",
        roots: &[".kiro/sessions"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "kimi",
        display_name: "Kimi",
        roots: &[".kimi/sessions", ".kimi-code/sessions"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "jcode",
        display_name: "JCode",
        roots: &[".jcode/sessions"],
        matcher: Matcher::Extension("jsonl"),
        // Anthropic-shaped: `input_tokens` beside `cache_read_input_tokens`.
        convention: Convention::CacheExclusive,
    },
    GenericAgent {
        client: "openclaw",
        display_name: "OpenClaw",
        roots: &[".openclaw/agents"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "droid",
        display_name: "Droid",
        roots: &[".factory/sessions"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "junie",
        display_name: "Junie",
        roots: &[".junie/sessions"],
        matcher: Matcher::FileName("events.jsonl"),
        convention: Convention::BySpelling,
    },
    GenericAgent {
        client: "cline",
        display_name: "Cline",
        // A VS Code extension, so its store is under the editor's global storage rather
        // than a dotfile of its own.
        roots: &[
            ".config/Code/User/globalStorage/saoudrizwan.claude-dev/tasks",
            "AppData/Roaming/Code/User/globalStorage/saoudrizwan.claude-dev/tasks",
        ],
        matcher: Matcher::FileName("api_conversation_history.json"),
        convention: Convention::CacheExclusive,
    },
    GenericAgent {
        client: "roo-code",
        display_name: "Roo Code",
        roots: &[
            ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks",
            "AppData/Roaming/Code/User/globalStorage/rooveterinaryinc.roo-cline/tasks",
        ],
        matcher: Matcher::FileName("api_conversation_history.json"),
        convention: Convention::CacheExclusive,
    },
    GenericAgent {
        client: "copilot-cli",
        display_name: "Copilot CLI",
        roots: &[".copilot/otel"],
        matcher: Matcher::Extension("jsonl"),
        convention: Convention::BySpelling,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::Source;
    use std::path::Path;

    #[test]
    fn client_ids_are_unique_across_the_catalog() {
        let mut ids: Vec<_> = CATALOG.iter().map(|agent| agent.client).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), before, "the client id is the grouping key");
    }

    #[test]
    fn every_agent_declares_at_least_one_root() {
        for agent in CATALOG {
            assert!(
                !agent.roots.is_empty(),
                "{} has nowhere to look",
                agent.client
            );
        }
    }

    /// A root written with `/` must become a real path on every platform, not a directory
    /// whose name contains slashes.
    #[test]
    fn roots_resolve_to_nested_directories() {
        let gemini = CATALOG
            .iter()
            .find(|a| a.client == "gemini")
            .expect("present");
        let resolved = gemini.roots(Path::new("/home/u"));
        assert!(resolved[0].ends_with("tmp"));
        assert!(
            resolved[0]
                .parent()
                .is_some_and(|dir| dir.ends_with(".gemini"))
        );
    }

    /// The two conventions are not interchangeable, so an agent relaying an OpenAI-shaped
    /// response must not be listed as cache-exclusive.
    #[test]
    fn openai_shaped_agents_are_marked_inclusive() {
        for client in ["gemini", "qwen"] {
            let agent = CATALOG
                .iter()
                .find(|a| a.client == client)
                .expect("present");
            assert_eq!(agent.convention, Convention::CacheInclusive, "{client}");
        }
    }
}
