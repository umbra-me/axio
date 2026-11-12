//! What `/mcp`, `/skills`, `/agents`, `/hooks`, `/plugins` and `/status`
//! show: rows for the pickers and lines for the plain lists, built from data
//! so they can be tested without a terminal.

use crate::mcp::{ServerConfig, ServerInfo};
use crate::menu::Choice;
use crate::roles::{Pick, Role};
use crate::skills::Library;
use std::path::Path;

/// The first line of `text`, at most `n` characters.
fn short(text: &str, n: usize) -> String {
    let line = text.lines().next().unwrap_or("").trim();
    if line.chars().count() <= n {
        line.to_string()
    } else {
        let cut: String = line.chars().take(n.saturating_sub(1)).collect();
        format!("{}…", cut.trim_end())
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// One row per configured MCP server: its tools and transport when it is
/// running, why not when it is not.
pub fn mcp_rows(
    configured: &[ServerConfig],
    running: &[ServerInfo],
    failed: &[String],
    signed_in: &dyn Fn(&str) -> bool,
) -> Vec<Choice> {
    configured
        .iter()
        .map(|c| {
            let transport = match (&c.url, c.sse) {
                (Some(_), true) => "sse",
                (Some(_), false) => "http",
                (None, _) => "stdio",
            };
            let detail = if let Some(r) = running.iter().find(|r| r.name == c.name) {
                let mut d = format!("{} · {transport}", plural(r.tools.len(), "tool"));
                if r.url.as_deref().is_some_and(signed_in) {
                    d.push_str(" · signed in");
                }
                d
            } else {
                let why = failed
                    .iter()
                    .find_map(|f| f.strip_prefix(&format!("{}: ", c.name)))
                    .unwrap_or("not started");
                if why.starts_with(crate::mcp::SIGN_IN) {
                    format!("needs sign-in · {transport}")
                } else {
                    format!("not connected: {}", short(why, 90))
                }
            };
            Choice::new(c.name.clone(), detail)
        })
        .collect()
}

/// Skills and commands, the ones you can run first as `/name`.
pub fn skill_rows(lib: &Library) -> Vec<Choice> {
    let mut runnable = Vec::new();
    let mut others = Vec::new();
    for c in &lib.commands {
        runnable.push(Choice::new(
            format!("/{}", c.name),
            format!("command · {}", short(&c.description, 80)),
        ));
    }
    for k in &lib.skills {
        let detail = format!("skill · {}", short(&k.description, 80));
        if k.invocable {
            runnable.push(Choice::new(format!("/{}", k.name), detail));
        } else {
            others.push(Choice::new(k.name.clone(), detail));
        }
    }
    runnable.sort_by(|a, b| a.label.cmp(&b.label));
    others.sort_by(|a, b| a.label.cmp(&b.label));
    runnable.extend(others);
    runnable
}

/// A role's model or effort in words.
fn pick(p: &Pick) -> String {
    match p {
        Pick::Inherit => "the main agent's".into(),
        Pick::Choose => "chosen per call".into(),
        Pick::Value(v) => v.clone(),
    }
}

/// The same, short, for a picker row.
fn pick_short(p: &Pick, what: &str) -> String {
    match p {
        Pick::Inherit => format!("main {what}"),
        Pick::Choose => format!("{what} chosen per call"),
        Pick::Value(v) if what == "model" => v.clone(),
        Pick::Value(v) => format!("{v} {what}"),
    }
}

/// One row per sub-agent role: model, effort and tools.
pub fn role_rows(roles: &[Role]) -> Vec<Choice> {
    roles
        .iter()
        .map(|r| {
            let tools = r
                .tools
                .as_ref()
                .map_or("the main agent's tools".into(), |t| t.join(", "));
            Choice::new(
                r.name.clone(),
                format!(
                    "{} · {} · {tools}",
                    pick_short(&r.model, "model"),
                    pick_short(&r.effort, "effort")
                ),
            )
        })
        .collect()
}

/// A role in full, for `/agents`.
pub fn role_detail(r: &Role) -> Vec<String> {
    let sandbox = match r.sandbox {
        Some(crate::child::Confine::ReadOnly) => "read-only",
        Some(crate::child::Confine::Project) => "project",
        None => "the main agent's",
    };
    let context = match r.context {
        crate::child::Sees::Brief => "brief",
        crate::child::Sees::Rules => "rules (adds AGENTS.md)",
        crate::child::Sees::Fork => "fork (adds the conversation)",
    };
    let mut lines = vec![
        format!("{}: {}", r.name, r.description),
        format!("model: {} · effort: {}", pick(&r.model), pick(&r.effort)),
        format!(
            "tools: {}",
            r.tools
                .as_ref()
                .map_or("the main agent's".into(), |t| t.join(", "))
        ),
        format!("sees: {context}"),
        format!("sandbox: {sandbox}"),
        format!("source: {}", r.source),
    ];
    if let Some(n) = r.max_turns {
        lines.push(format!("turn limit: {n}"));
    }
    lines.push(String::new());
    lines.extend(r.instructions.lines().take(8).map(String::from));
    if r.instructions.lines().count() > 8 {
        lines.push("…".into());
    }
    lines
}

/// Hooks grouped by event: matcher, command and where each came from.
pub fn hook_lines(hooks: &[(&str, &str, &str, &str)]) -> Vec<String> {
    if hooks.is_empty() {
        return vec!["no hooks are configured".into()];
    }
    let mut events: Vec<&str> = Vec::new();
    for (e, ..) in hooks {
        if !events.contains(e) {
            events.push(e);
        }
    }
    events.sort();
    let mut lines = Vec::new();
    for e in events {
        lines.push(e.to_string());
        for (_, matcher, command, source) in hooks.iter().filter(|h| h.0 == e) {
            let tool_event = matches!(e, "PreToolUse" | "PostToolUse");
            let on = match (*matcher, tool_event) {
                ("" | "*", true) => "every tool  ".to_string(),
                ("" | "*", false) => String::new(),
                (m, _) => format!("{m}  "),
            };
            lines.push(format!("  {on}{}  ({source})", short(command, 70)));
        }
    }
    lines
}

/// What a plugin folder provides.
pub fn plugin_provides(root: &Path) -> Vec<&'static str> {
    let mut v = Vec::new();
    if root.join("hooks/hooks.json").is_file() {
        v.push("hooks");
    }
    if root.join(".mcp.json").is_file() {
        v.push("MCP servers");
    }
    for (dir, what) in [
        ("skills", "skills"),
        ("commands", "commands"),
        ("agents", "agents"),
    ] {
        if root.join(dir).is_dir() {
            v.push(what);
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn server(name: &str, url: Option<&str>) -> ServerConfig {
        ServerConfig {
            name: name.into(),
            command: if url.is_some() {
                String::new()
            } else {
                "srv".into()
            },
            args: Vec::new(),
            env: Vec::new(),
            timeout: Duration::from_secs(1),
            url: url.map(String::from),
            headers: Vec::new(),
            sse: false,
        }
    }

    #[test]
    fn mcp_rows_say_what_each_server_is_doing() {
        let configured = [
            server("local", None),
            server("remote", Some("https://r.example/mcp")),
            server("broken", None),
            server("locked", Some("https://l.example/mcp")),
        ];
        let running = [
            ServerInfo {
                name: "local".into(),
                url: None,
                tools: vec!["mcp__local__a".into()],
            },
            ServerInfo {
                name: "remote".into(),
                url: Some("https://r.example/mcp".into()),
                tools: vec!["mcp__remote__a".into(), "mcp__remote__b".into()],
            },
        ];
        let failed = [
            "broken: could not start `srv`: not found".to_string(),
            "locked: the server needs a sign-in: run `axio mcp login locked`".to_string(),
        ];
        let rows = mcp_rows(&configured, &running, &failed, &|u| u.contains("r.example"));
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|c| (c.label.as_str(), c.detail.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("local", "1 tool · stdio"),
                ("remote", "2 tools · http · signed in"),
                ("broken", "not connected: could not start `srv`: not found"),
                ("locked", "needs sign-in · http"),
            ]
        );
    }

    #[test]
    fn skills_you_can_run_come_first() {
        let dir = std::env::temp_dir().join(format!("axio-panels-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (p, t) in [
            (
                "skills/pdf/SKILL.md",
                "---\nname: pdf\ndescription: Read PDFs\n---\n",
            ),
            (
                "skills/deploy/SKILL.md",
                "---\nname: deploy\ndescription: Ship it\nuser-invocable: true\n---\n",
            ),
            (
                "commands/review.md",
                "---\ndescription: Review the diff\n---\nReview $ARGUMENTS",
            ),
        ] {
            std::fs::create_dir_all(dir.join(p).parent().unwrap()).unwrap();
            std::fs::write(dir.join(p), t).unwrap();
        }
        let mut lib = Library::default();
        lib.add_dir(&dir, None);
        let rows = skill_rows(&lib);
        let shown: Vec<(&str, &str)> = rows
            .iter()
            .map(|c| (c.label.as_str(), c.detail.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("/deploy", "skill · Ship it"),
                ("/review", "command · Review the diff"),
                ("pdf", "skill · Read PDFs"),
            ]
        );
    }

    #[test]
    fn roles_show_model_effort_and_tools() {
        let (roles, _) = crate::roles::load(&[]);
        let rows = role_rows(&roles);
        assert_eq!(rows[0].label, "explore");
        assert_eq!(
            rows[0].detail,
            "main model · main effort · Read, Grep, Glob"
        );
        assert_eq!(
            rows[2].detail,
            "main model · high effort · Read, Grep, Glob, Bash"
        );
        let detail = role_detail(&roles[0]);
        assert!(detail.iter().any(|l| l == "source: built in"), "{detail:?}");
        assert!(
            detail
                .iter()
                .any(|l| l == "model: the main agent's · effort: the main agent's"),
            "{detail:?}"
        );
        assert!(
            detail.iter().any(|l| l.starts_with("sandbox: read-only")),
            "{detail:?}"
        );
        assert!(
            detail
                .iter()
                .any(|l| l.contains("You search the code base")),
            "{detail:?}"
        );
    }

    #[test]
    fn hooks_are_grouped_by_event() {
        let lines = hook_lines(&[
            (
                "PreToolUse",
                "Bash",
                "guard",
                "plugins/guard/hooks/hooks.json",
            ),
            ("SessionStart", "", "echo hi", "settings.json"),
            ("PreToolUse", "", "log", "settings.json"),
        ]);
        assert_eq!(
            lines,
            [
                "PreToolUse",
                "  Bash  guard  (plugins/guard/hooks/hooks.json)",
                "  every tool  log  (settings.json)",
                "SessionStart",
                "  echo hi  (settings.json)",
            ]
        );
        assert_eq!(hook_lines(&[]), ["no hooks are configured"]);
    }

    #[test]
    fn plugins_say_what_they_provide() {
        let dir = std::env::temp_dir().join(format!("axio-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("hooks")).unwrap();
        std::fs::write(dir.join("hooks/hooks.json"), "{}").unwrap();
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join(".mcp.json"), "{}").unwrap();
        assert_eq!(plugin_provides(&dir), ["hooks", "MCP servers", "skills"]);
        assert_eq!(short("one two three\nmore", 8), "one two…");
    }
}
