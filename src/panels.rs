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

/// One part of the context window, for `/context`.
pub struct Part {
    pub label: String,
    pub tokens: u64,
}

/// `/context`: a bar of the window, then each part with its tokens and
/// share, the free space and where compaction starts.
pub fn context_view(parts: &[Part], window: u64, compact_at: u64, color: bool) -> Vec<String> {
    const CELLS: u64 = 50;
    const MARKS: [&str; 8] = ["#", "=", "*", "+", "%", "@", "&", "~"];
    const COLORS: [&str; 6] = [
        "\x1b[36m", "\x1b[32m", "\x1b[33m", "\x1b[35m", "\x1b[34m", "\x1b[31m",
    ];
    let window = window.max(1);
    let used: u64 = parts.iter().map(|p| p.tokens).sum();
    let mark = |i: usize| {
        if color {
            crate::ui::paint(true, COLORS[i % COLORS.len()], "█")
        } else {
            MARKS[i % MARKS.len()].to_string()
        }
    };
    let free_mark = if color {
        crate::ui::paint(true, crate::ui::DIM, "░")
    } else {
        ".".to_string()
    };
    let mut bar = String::new();
    let mut cells = 0;
    for (i, p) in parts.iter().enumerate() {
        let n = if p.tokens == 0 {
            0
        } else {
            ((p.tokens * CELLS + window / 2) / window).max(1)
        };
        bar.push_str(&mark(i).repeat(n as usize));
        cells += n;
    }
    bar.push_str(&free_mark.repeat(CELLS.saturating_sub(cells) as usize));
    let pct = |t: u64| t * 100 / window;
    let tok = crate::ui::tokens;
    let mut lines = vec![
        bar,
        format!("{} of {} tokens ({}%)", tok(used), tok(window), pct(used)),
    ];
    for (i, p) in parts.iter().enumerate() {
        lines.push(format!(
            "  {} {:<14} {:>6} {:>3}%",
            mark(i),
            p.label,
            tok(p.tokens),
            pct(p.tokens)
        ));
    }
    let free = window.saturating_sub(used);
    lines.push(format!(
        "  {free_mark} {:<14} {:>6} {:>3}%",
        "Free space",
        tok(free),
        pct(free)
    ));
    lines.push(format!(
        "compaction starts at {} ({}%)",
        tok(compact_at),
        pct(compact_at)
    ));
    lines
}

/// A setting `/settings` can change: its key path in settings.json, a
/// label, the value now and what it does.
pub struct Setting {
    pub path: &'static [&'static str],
    pub label: &'static str,
    pub value: String,
    pub about: &'static str,
}

/// The settings `/settings` lists, with their values in `s`.
pub fn settings_rows(s: &crate::settings::Settings) -> Vec<Setting> {
    let or = |v: Option<String>, default: &str| v.unwrap_or_else(|| format!("{default} (default)"));
    let on = |v: Option<bool>| match v {
        Some(true) => "on".to_string(),
        Some(false) => "off".to_string(),
        None => "off (default)".to_string(),
    };
    let num = |v: Option<usize>, d: usize| v.map_or(format!("{d} (default)"), |n| n.to_string());
    let sa = &s.subagents;
    let items = |chosen: &Option<Vec<String>>, all: &[(&str, &str)]| match shown(chosen, all) {
        v if v.is_empty() => "hidden".to_string(),
        v => v.join(", "),
    };
    vec![
        Setting {
            path: &["statusLine", "underPrompt"],
            label: "Under the prompt",
            value: items(&s.status_line.under_prompt, &UNDER_PROMPT),
            about: "what the status line under the prompt shows",
        },
        Setting {
            path: &["statusLine", "afterTurn"],
            label: "After each turn",
            value: items(&s.status_line.after_turn, &AFTER_TURN),
            about: "what the line after each turn shows",
        },
        Setting {
            path: &["permissions", "defaultMode"],
            label: "Permission mode",
            value: s
                .permissions
                .default_mode
                .clone()
                .unwrap_or_else(|| "default (default)".into()),
            about: "when tools ask first: default, acceptEdits, plan or bypassPermissions",
        },
        Setting {
            path: &["model"],
            label: "Default model",
            value: or(s.model.clone(), crate::provider::DEFAULT_MODEL),
            about: "the model new sessions start on",
        },
        Setting {
            path: &["effort"],
            label: "Effort",
            value: s.effort.clone().unwrap_or_else(|| "model default".into()),
            about: "reasoning effort for new sessions",
        },
        Setting {
            path: &["fastMode"],
            label: "Fast mode by default",
            value: on(s.fast_mode),
            about: "faster output at a higher price, on models that have it",
        },
        Setting {
            path: &["profile"],
            label: "Profile",
            value: or(s.profile.clone(), crate::profile::DEFAULT),
            about: "which modules and plugins load",
        },
        Setting {
            path: &["compactModel"],
            label: "Compaction model",
            value: s
                .compact_model
                .clone()
                .unwrap_or_else(|| "main model (default)".into()),
            about: "the model that writes compaction summaries",
        },
        Setting {
            path: &["sandboxNetwork"],
            label: "Sandbox network",
            value: on(s.sandbox_network),
            about: "lets sandboxed commands reach the network",
        },
        Setting {
            path: &["importFromOtherHarnesses"],
            label: "Import from other harnesses",
            value: s
                .import_from_other_harnesses
                .clone()
                .unwrap_or_else(|| "off (default)".into()),
            about: "use hooks, plugins and servers from Claude Code, Codex, Grok and Pi",
        },
        Setting {
            path: &["subagents", "parallel"],
            label: "Sub-agents at once",
            value: num(sa.parallel, crate::subagent::PARALLEL),
            about: "how many sub-agents may run at the same time",
        },
        Setting {
            path: &["subagents", "nesting"],
            label: "Sub-agent nesting",
            value: num(sa.nesting.map(|n| n as usize), 0),
            about: "levels of sub-agents that may start their own",
        },
        Setting {
            path: &["subagents", "fast"],
            label: "Sub-agents use fast mode",
            value: on(sa.fast),
            about: "fast mode for sub-agents on models that have it",
        },
        Setting {
            path: &["subagents", "fastModel"],
            label: "Fast model",
            value: or(sa.fast_model.clone(), "by provider"),
            about: "what a role's `fast` model means",
        },
        Setting {
            path: &["subagents", "resultLimit"],
            label: "Sub-agent answer limit",
            value: num(sa.result_limit, crate::child::RESULT_LIMIT),
            about: "characters of a sub-agent's answer returned",
        },
    ]
}

/// What the line under the prompt can show, in order, and what each is.
pub const UNDER_PROMPT: [(&str, &str); 3] = [
    ("model", "model, effort and fast mode"),
    ("context", "how full the context is"),
    ("cost", "what the session has cost"),
];

/// What the line after each turn can show, in order.
pub const AFTER_TURN: [(&str, &str); 6] = [
    ("context", "context used of the window"),
    ("output", "output tokens this turn"),
    ("cache", "share of the prompt read from cache"),
    ("time", "how long the turn took"),
    ("speed", "output tokens a second"),
    ("first", "wait for the first token"),
];

/// The items switched on, in display order: all of them unless the
/// setting lists some (unknown names are ignored).
pub fn shown<'a>(chosen: &Option<Vec<String>>, all: &[(&'a str, &str)]) -> Vec<&'a str> {
    all.iter()
        .map(|(name, _)| *name)
        .filter(|name| chosen.as_ref().is_none_or(|c| c.iter().any(|x| x == name)))
        .collect()
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

    #[test]
    fn context_shows_each_part_and_the_free_space() {
        let parts = [
            Part {
                label: "System prompt".into(),
                tokens: 2_000,
            },
            Part {
                label: "Tools".into(),
                tokens: 6_000,
            },
            Part {
                label: "Messages".into(),
                tokens: 12_000,
            },
        ];
        let lines = context_view(&parts, 100_000, 85_000, false);
        // 50 cells: 1 + 3 + 6 used, 40 free.
        assert_eq!(
            lines[0],
            format!(
                "{}{}{}{}",
                "#",
                "=".repeat(3),
                "*".repeat(6),
                ".".repeat(40)
            )
        );
        assert_eq!(lines[1], "20.0k of 100.0k tokens (20%)");
        assert!(
            lines.iter().any(|l| l == "  # System prompt    2.0k   2%"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "  * Messages        12.0k  12%"),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l == "  . Free space      80.0k  80%"),
            "{lines:?}"
        );
        assert_eq!(lines.last().unwrap(), "compaction starts at 85.0k (85%)");
    }

    #[test]
    fn settings_rows_show_values_and_defaults() {
        let s: crate::settings::Settings = serde_json::from_str(
            r#"{"model": "anthropic/claude-opus-5-5", "fastMode": true, "subagents": {"parallel": 2}}"#,
        )
        .unwrap();
        let rows = settings_rows(&s);
        let value = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(value("Default model"), "anthropic/claude-opus-5-5");
        assert_eq!(value("Fast mode by default"), "on");
        assert_eq!(value("Effort"), "model default");
        assert_eq!(value("Sub-agents at once"), "2");
        assert_eq!(value("Sub-agent nesting"), "0 (default)");
        assert_eq!(value("Compaction model"), "main model (default)");
        assert!(rows.iter().all(|r| !r.about.is_empty()));
    }

    #[test]
    fn status_items_default_to_all_and_keep_their_order() {
        assert_eq!(shown(&None, &UNDER_PROMPT), ["model", "context", "cost"]);
        let chosen = Some(vec![
            "cost".to_string(),
            "model".to_string(),
            "nope".to_string(),
        ]);
        assert_eq!(shown(&chosen, &UNDER_PROMPT), ["model", "cost"]);
        assert!(
            shown(&Some(Vec::new()), &AFTER_TURN).is_empty(),
            "an empty list hides the line"
        );
        let s: crate::settings::Settings =
            serde_json::from_str(r#"{"statusLine": {"afterTurn": ["speed"]}}"#).unwrap();
        let rows = settings_rows(&s);
        let value = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(value("Under the prompt"), "model, context, cost");
        assert_eq!(value("After each turn"), "speed");
        let none: crate::settings::Settings =
            serde_json::from_str(r#"{"statusLine": {"underPrompt": []}}"#).unwrap();
        assert_eq!(
            settings_rows(&none)
                .iter()
                .find(|r| r.label == "Under the prompt")
                .unwrap()
                .value,
            "hidden"
        );
    }
}
