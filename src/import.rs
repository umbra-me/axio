//! Offers hooks, plugins, MCP servers and skills that other agent harnesses
//! on this machine already use. Accepted items are used where they are, never
//! copied; answers live in `~/.axio/agent/imports.json`.

use crate::hooks::Event;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// A file with a `{"hooks": …}` document.
    Hooks(PathBuf),
    /// A plugin folder; `codex` plugins keep their files in `.codex-plugin/`.
    Plugin { root: PathBuf, codex: bool },
    /// One MCP server entry, already in axio's `mcpServers` shape.
    Mcp(Value),
    /// A folder holding `skills/` (and possibly `commands/`).
    Skills(PathBuf),
    /// Something axio cannot run, listed so the user knows.
    Unusable,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Support {
    Works,
    Partly(String),
    No(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub harness: &'static str,
    pub kind: Kind,
    pub label: &'static str,
    pub name: String,
    pub version: Option<String>,
    pub support: Support,
    /// Changes when the item does, so a changed item is offered again.
    pub fingerprint: String,
}

impl Item {
    pub fn id(&self) -> String {
        format!("{}/{}/{}", self.harness, self.label, self.name)
    }
}

fn fnv(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h = (h ^ u64::from(*b)).wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn read_toml(path: &Path) -> Option<Value> {
    crate::toml_lite::parse(&std::fs::read_to_string(path).ok()?).ok()
}

/// How well axio runs a hooks document.
fn hook_support(doc: &Value) -> Support {
    let events = doc["hooks"].as_object().cloned().unwrap_or_default();
    if events.is_empty() {
        return Support::No("declares no hooks".into());
    }
    let mut missing: Vec<String> = events
        .keys()
        .filter(|e| !Event::ALL.iter().any(|a| a.name() == e.as_str()))
        .cloned()
        .collect();
    let non_command = events
        .values()
        .flat_map(|g| g.as_array().cloned().unwrap_or_default())
        .any(|g| {
            g["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|h| h["type"].as_str() != Some("command"))
        });
    if missing.len() == events.len() {
        return Support::No(format!("{} have no axio event", missing.join(", ")));
    }
    let mut notes = Vec::new();
    if !missing.is_empty() {
        missing.sort();
        notes.push(format!(
            "{} of {} events work; {} {} no axio event",
            events.len() - missing.len(),
            events.len(),
            missing.join(", "),
            if missing.len() == 1 { "has" } else { "have" }
        ));
    }
    if non_command {
        notes.push("only command hooks run".into());
    }
    if notes.is_empty() {
        Support::Works
    } else {
        Support::Partly(notes.join("; "))
    }
}

/// Converts an MCP entry from any harness to axio's shape, or says why not.
fn mcp_entry(v: &Value) -> Result<Value, String> {
    if v.get("url").is_some()
        || matches!(v["type"].as_str(), Some("http" | "sse" | "streamable-http"))
    {
        return Err("HTTP and SSE servers are not supported yet".into());
    }
    let command = v["command"].as_str().ok_or("has no command")?;
    Ok(json!({"command": command, "args": v["args"].clone(), "env": v["env"].clone()}))
}

fn mcp_items(harness: &'static str, servers: Option<&serde_json::Map<String, Value>>) -> Vec<Item> {
    servers
        .into_iter()
        .flatten()
        .filter(|(_, v)| v["enabled"] != false && v["disabled"] != true)
        .map(|(name, v)| {
            let fingerprint = fnv(v.to_string().as_bytes());
            let (kind, support) = match mcp_entry(v) {
                Ok(entry) => (Kind::Mcp(entry), Support::Works),
                Err(why) => (Kind::Unusable, Support::No(why)),
            };
            Item {
                harness,
                kind,
                label: "mcp",
                name: name.clone(),
                version: None,
                support,
                fingerprint,
            }
        })
        .collect()
}

/// What axio can use from a plugin folder.
fn plugin_support(root: &Path, codex: bool) -> Support {
    let dir = |p: &str| {
        if codex {
            root.join(".codex-plugin").join(p)
        } else {
            root.join(p)
        }
    };
    let hooks = read_json(&if codex {
        dir("hooks.json")
    } else {
        root.join("hooks/hooks.json")
    });
    let mcp = read_json(&if codex {
        dir("mcp.json")
    } else {
        root.join(".mcp.json")
    });
    let has_skills = root.join("skills").is_dir();
    let has_commands = root.join("commands").is_dir();
    let mut works = Vec::new();
    let mut notes = Vec::new();
    if let Some(h) = &hooks {
        match hook_support(h) {
            Support::Works => works.push("hooks"),
            Support::Partly(n) => {
                works.push("hooks");
                notes.push(n)
            }
            Support::No(n) => notes.push(format!("hooks: {n}")),
        }
    }
    if let Some(m) = &mcp {
        for (name, v) in m["mcpServers"].as_object().into_iter().flatten() {
            match mcp_entry(v) {
                Ok(_) => works.push("MCP"),
                Err(why) => notes.push(format!("MCP server {name}: {why}")),
            }
        }
    }
    if has_skills {
        works.push("skills");
    }
    if has_commands {
        works.push("commands");
    }
    if root.join("agents").is_dir() {
        notes.push("sub-agents are not supported".into());
    }
    works.dedup();
    if works.is_empty() {
        return Support::No(if notes.is_empty() {
            "has nothing axio uses (for example an LSP server or an app integration)".into()
        } else {
            notes.join("; ")
        });
    }
    if notes.is_empty() {
        Support::Works
    } else {
        Support::Partly(notes.join("; "))
    }
}

fn skills_item(harness: &'static str, base: &Path) -> Option<Item> {
    let skills: Vec<String> = std::fs::read_dir(base.join("skills"))
        .ok()?
        .flatten()
        .filter(|e| e.path().join("SKILL.md").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    if skills.is_empty() {
        return None;
    }
    let mut sorted = skills.clone();
    sorted.sort();
    Some(Item {
        harness,
        kind: Kind::Skills(base.to_path_buf()),
        label: "skills",
        name: sorted.join(", "),
        version: None,
        support: Support::Works,
        fingerprint: fnv(sorted.join("\n").as_bytes()),
    })
}

/// Everything found under the user's home folder `h`.
pub fn scan(h: &Path) -> Vec<Item> {
    let mut items = Vec::new();

    // Claude Code
    let claude = h.join(".claude");
    if let Some(s) = read_json(&claude.join("settings.json"))
        && s["hooks"].as_object().is_some_and(|m| !m.is_empty())
    {
        items.push(Item {
            harness: "Claude Code",
            kind: Kind::Hooks(claude.join("settings.json")),
            label: "hooks",
            name: "~/.claude/settings.json".into(),
            version: None,
            support: hook_support(&s),
            fingerprint: fnv(s["hooks"].to_string().as_bytes()),
        });
    }
    if let Some(reg) = read_json(&claude.join("plugins/installed_plugins.json")) {
        for (id, installs) in reg["plugins"].as_object().into_iter().flatten() {
            let Some(e) = installs.as_array().and_then(|a| a.last()) else {
                continue;
            };
            let root = PathBuf::from(e["installPath"].as_str().unwrap_or(""));
            if !root.is_dir() {
                continue;
            }
            let version = e["version"].as_str().map(String::from);
            items.push(Item {
                harness: "Claude Code",
                support: plugin_support(&root, false),
                kind: Kind::Plugin { root, codex: false },
                label: "plugin",
                name: id.split('@').next().unwrap_or(id).to_string(),
                fingerprint: version.clone().unwrap_or_default(),
                version,
            });
        }
    }
    if let Some(c) = read_json(&h.join(".claude.json")) {
        items.extend(mcp_items("Claude Code", c["mcpServers"].as_object()));
    }
    items.extend(skills_item("Claude Code", &claude));

    // Codex
    let codex = h.join(".codex");
    if let Some(doc) = read_json(&codex.join("hooks.json")) {
        items.push(Item {
            harness: "Codex",
            support: hook_support(&doc),
            kind: Kind::Hooks(codex.join("hooks.json")),
            label: "hooks",
            name: "~/.codex/hooks.json".into(),
            version: None,
            fingerprint: fnv(doc.to_string().as_bytes()),
        });
    }
    if let Some(cfg) = read_toml(&codex.join("config.toml")) {
        items.extend(mcp_items("Codex", cfg["mcp_servers"].as_object()));
        for (id, p) in cfg["plugins"].as_object().into_iter().flatten() {
            if p["enabled"] != true {
                continue;
            }
            let (name, market) = id.split_once('@').unwrap_or((id, ""));
            // The newest version folder in the plugin cache.
            let mut versions: Vec<PathBuf> =
                std::fs::read_dir(codex.join("plugins/cache").join(market).join(name))
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.join(".codex-plugin/plugin.json").is_file())
                    .collect();
            versions.sort();
            let Some(root) = versions.pop() else { continue };
            let version = read_json(&root.join(".codex-plugin/plugin.json"))
                .and_then(|m| m["version"].as_str().map(String::from));
            items.push(Item {
                harness: "Codex",
                support: plugin_support(&root, true),
                kind: Kind::Plugin { root, codex: true },
                label: "plugin",
                name: name.to_string(),
                fingerprint: version.clone().unwrap_or_default(),
                version,
            });
        }
    }

    // Grok Build
    let grok = h.join(".grok");
    let mut grok_hooks: Vec<PathBuf> = std::fs::read_dir(grok.join("hooks"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    grok_hooks.sort();
    for file in grok_hooks {
        let Some(doc) = read_json(&file) else {
            continue;
        };
        items.push(Item {
            harness: "Grok Build",
            support: hook_support(&doc),
            name: format!(
                "~/.grok/hooks/{}",
                file.file_name().unwrap_or_default().to_string_lossy()
            ),
            kind: Kind::Hooks(file),
            label: "hooks",
            version: None,
            fingerprint: fnv(doc.to_string().as_bytes()),
        });
    }
    if let Some(cfg) = read_toml(&grok.join("config.toml")) {
        items.extend(mcp_items("Grok Build", cfg["mcp_servers"].as_object()));
    }
    items.extend(skills_item("Grok Build", &grok));

    // Pi
    let pi = h.join(".pi/agent");
    if let Some(m) = read_json(&pi.join("mcp.json")) {
        items.extend(mcp_items("Pi", m["mcpServers"].as_object()));
    }
    items.extend(skills_item("Pi", &pi));
    let mut extensions: Vec<String> = std::fs::read_dir(pi.join("extensions"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| {
            e.file_name()
                .to_string_lossy()
                .trim_end_matches(".ts")
                .to_string()
        })
        .filter(|n| !n.starts_with('.'))
        .collect();
    extensions.sort();
    for name in extensions {
        items.push(Item {
            harness: "Pi",
            kind: Kind::Unusable,
            label: "ext",
            fingerprint: name.clone(),
            name,
            version: None,
            support: Support::No("TypeScript extension: axio runs no JavaScript".into()),
        });
    }
    items
}

fn run_version(cmd: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    while child.try_wait().ok()?.is_none() {
        if started.elapsed() > Duration::from_secs(3) {
            let _ = child.kill();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    out.split_whitespace()
        .find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(String::from)
}

/// The installed harness's version, found only when there is something to
/// show, because asking each CLI takes a moment.
pub fn harness_version(harness: &str, h: &Path) -> Option<String> {
    match harness {
        "Claude Code" => run_version("claude", &["--version"]),
        "Codex" => run_version("codex", &["--version"]),
        "Grok Build" => read_json(&h.join(".grok/version.json"))
            .and_then(|v| v["version"].as_str().map(String::from)),
        "Pi" => run_version("pi", &["--version"]),
        _ => None,
    }
}

/// Answers already given, in `~/.axio/agent/imports.json`.
#[derive(Debug, Default)]
pub struct Record {
    pub never: bool,
    /// id -> (fingerprint, accepted)
    pub items: std::collections::BTreeMap<String, (String, bool)>,
}

impl Record {
    pub fn load(home: &Path) -> Record {
        let Some(v) = read_json(&home.join("imports.json")) else {
            return Record::default();
        };
        Record {
            never: v["never"] == true,
            items: v["items"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, e)| {
                    (
                        k.clone(),
                        (
                            e["fingerprint"].as_str().unwrap_or("").to_string(),
                            e["accepted"] == true,
                        ),
                    )
                })
                .collect(),
        }
    }

    pub fn save(&self, home: &Path) -> Result<(), String> {
        let items: serde_json::Map<String, Value> = self
            .items
            .iter()
            .map(|(k, (f, a))| (k.clone(), json!({"fingerprint": f, "accepted": a})))
            .collect();
        std::fs::create_dir_all(home).map_err(|e| e.to_string())?;
        let path = home.join("imports.json");
        std::fs::write(
            &path,
            format!("{:#}\n", json!({"never": self.never, "items": items})),
        )
        .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Items not answered yet, or changed since.
    pub fn pending<'a>(&self, items: &'a [Item]) -> Vec<&'a Item> {
        items
            .iter()
            .filter(|i| {
                self.items
                    .get(&i.id())
                    .is_none_or(|(f, _)| *f != i.fingerprint)
            })
            .collect()
    }

    /// Accepted items that are still current and usable.
    pub fn accepted<'a>(&self, items: &'a [Item]) -> Vec<&'a Item> {
        items
            .iter()
            .filter(|i| !matches!(i.support, Support::No(_)))
            .filter(|i| {
                self.items
                    .get(&i.id())
                    .is_some_and(|(f, a)| *a && *f == i.fingerprint)
            })
            .collect()
    }

    pub fn answer(&mut self, item: &Item, accepted: bool) {
        self.items
            .insert(item.id(), (item.fingerprint.clone(), accepted));
    }
}

/// The listing shown before the first turn.
pub fn render(pending: &[&Item], versions: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::from("Found in other harnesses on this machine:\n");
    let mut harness = "";
    for i in pending {
        if i.harness != harness {
            harness = i.harness;
            out.push_str(&format!(
                "\n{}{}\n",
                harness,
                versions(harness).map_or(String::new(), |v| format!(" {v}"))
            ));
        }
        let name = match &i.version {
            Some(v) => format!("{} {v}", i.name),
            None => i.name.clone(),
        };
        let how = match &i.support {
            Support::Works => "works as is".to_string(),
            Support::Partly(n) => format!("partly: {n}"),
            Support::No(n) => format!("cannot run: {n}"),
        };
        out.push_str(&format!("  {:<7} {:<34} {how}\n", i.label, name));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, p: &str, t: &str) {
        let p = root.join(p);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, t).unwrap();
    }

    fn fake_home(name: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("axio-import-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        let plugin = h.join("claude-plugins/figma");
        write(
            &h,
            ".claude/settings.json",
            r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "echo hi"}]}]}}"#,
        );
        write(
            &h,
            ".claude/plugins/installed_plugins.json",
            &format!(
                r#"{{"plugins": {{"figma@official": [{{"installPath": "{}", "version": "2.2.157"}}], "lsp@official": [{{"installPath": "{}", "version": "1.0.0"}}]}}}}"#,
                plugin.display(),
                h.join("claude-plugins/lsp").display()
            ),
        );
        write(
            &h,
            "claude-plugins/figma/.mcp.json",
            r#"{"mcpServers": {"figma": {"type": "http", "url": "https://x.invalid"}}}"#,
        );
        write(
            &h,
            "claude-plugins/figma/skills/use/SKILL.md",
            "---\nname: use\n---\n",
        );
        write(
            &h,
            "claude-plugins/lsp/.claude-plugin/plugin.json",
            r#"{"name": "lsp"}"#,
        );
        write(
            &h,
            ".claude.json",
            r#"{"mcpServers": {"browse": {"command": "browse", "args": ["mcp"]}, "web": {"type": "http", "url": "https://x.invalid"}}}"#,
        );
        write(
            &h,
            ".claude/skills/charon/SKILL.md",
            "---\nname: charon\n---\n",
        );
        write(
            &h,
            ".codex/hooks.json",
            r#"{"hooks": {"PreToolUse": [{"hooks": [{"type": "command", "command": "x"}]}], "PermissionRequest": [{"hooks": [{"type": "command", "command": "y"}]}]}}"#,
        );
        write(
            &h,
            ".codex/config.toml",
            "[mcp_servers.node]\ncommand = \"node\"\nargs = [\"repl.js\"]\n[mcp_servers.off]\ncommand = \"x\"\nenabled = false\n[plugins.\"ida@Hex\"]\nenabled = true\n",
        );
        write(
            &h,
            ".codex/plugins/cache/Hex/ida/1.0/.codex-plugin/plugin.json",
            r#"{"name": "ida", "version": "1.0"}"#,
        );
        write(
            &h,
            ".codex/plugins/cache/Hex/ida/2.0/.codex-plugin/plugin.json",
            r#"{"name": "ida", "version": "2.0"}"#,
        );
        write(
            &h,
            ".codex/plugins/cache/Hex/ida/2.0/.codex-plugin/mcp.json",
            r#"{"mcpServers": {"ida": {"command": "ida-mcp"}}}"#,
        );
        write(
            &h,
            ".grok/hooks/session.json",
            r#"{"hooks": {"Notification": [{"hooks": [{"type": "command", "command": "n"}]}]}}"#,
        );
        write(
            &h,
            ".grok/config.toml",
            "[mcp_servers.browse]\ncommand = \"browse\"\n",
        );
        write(
            &h,
            ".pi/agent/mcp.json",
            r#"{"mcpServers": {"ida": {"command": "ida", "args": []}}}"#,
        );
        write(
            &h,
            ".pi/agent/extensions/ollama.ts",
            "export default () => {}",
        );
        h
    }

    fn line(items: &[Item], harness: &str, name: &str) -> Support {
        items
            .iter()
            .find(|i| i.harness == harness && i.name == name)
            .unwrap_or_else(|| panic!("{harness} {name} missing"))
            .support
            .clone()
    }

    #[test]
    fn scans_every_harness_and_says_what_works() {
        let h = fake_home("scan");
        let items = scan(&h);
        assert_eq!(
            line(&items, "Claude Code", "~/.claude/settings.json"),
            Support::Works
        );
        assert_eq!(
            line(&items, "Claude Code", "figma"),
            Support::Partly("MCP server figma: HTTP and SSE servers are not supported yet".into())
        );
        assert!(
            matches!(line(&items, "Claude Code", "lsp"), Support::No(n) if n.starts_with("has nothing axio uses"))
        );
        assert_eq!(line(&items, "Claude Code", "browse"), Support::Works);
        assert_eq!(
            line(&items, "Claude Code", "web"),
            Support::No("HTTP and SSE servers are not supported yet".into())
        );
        assert_eq!(line(&items, "Claude Code", "charon"), Support::Works);
        assert_eq!(
            line(&items, "Codex", "~/.codex/hooks.json"),
            Support::Partly("1 of 2 events work; PermissionRequest has no axio event".into())
        );
        assert_eq!(line(&items, "Codex", "node"), Support::Works);
        assert!(
            !items.iter().any(|i| i.name == "off"),
            "disabled servers are not offered"
        );
        let ida = items
            .iter()
            .find(|i| i.harness == "Codex" && i.name == "ida")
            .unwrap();
        assert_eq!(
            (ida.version.as_deref(), &ida.support),
            (Some("2.0"), &Support::Works)
        );
        assert!(
            matches!(line(&items, "Grok Build", "~/.grok/hooks/session.json"), Support::No(n) if n.contains("Notification"))
        );
        assert_eq!(line(&items, "Grok Build", "browse"), Support::Works);
        assert_eq!(line(&items, "Pi", "ida"), Support::Works);
        assert!(matches!(line(&items, "Pi", "ollama"), Support::No(n) if n.contains("TypeScript")));
        std::fs::remove_dir_all(h).unwrap();
    }

    #[test]
    fn remembers_answers_and_reoffers_changes() {
        let h = fake_home("record");
        let items = scan(&h);
        let mut r = Record::default();
        assert_eq!(r.pending(&items).len(), items.len());
        for i in &items {
            r.answer(i, i.name == "browse" || i.name == "lsp");
        }
        r.save(&h).unwrap();
        let r = Record::load(&h);
        assert!(r.pending(&items).is_empty());
        let accepted: Vec<String> = r.accepted(&items).iter().map(|i| i.id()).collect();
        assert_eq!(
            accepted,
            ["Claude Code/mcp/browse", "Grok Build/mcp/browse"],
            "unusable items never load"
        );
        write(
            &h,
            ".grok/config.toml",
            "[mcp_servers.browse]\ncommand = \"browse2\"\n",
        );
        let changed = scan(&h);
        assert_eq!(
            r.pending(&changed)
                .iter()
                .map(|i| i.id())
                .collect::<Vec<_>>(),
            ["Grok Build/mcp/browse"]
        );
        assert_eq!(
            r.accepted(&changed).len(),
            1,
            "a changed item waits for a new answer"
        );
        std::fs::remove_dir_all(h).unwrap();
    }

    #[test]
    fn renders_the_prompt_listing() {
        let h = fake_home("render");
        let items = scan(&h);
        let pending: Vec<&Item> = items.iter().filter(|i| i.harness == "Pi").collect();
        let text = render(&pending, &|_| Some("1.5.2".into()));
        assert_eq!(
            text,
            "Found in other harnesses on this machine:\n\nPi 1.5.2\n  mcp     ida                                works as is\n  ext     ollama                             cannot run: TypeScript extension: axio runs no JavaScript\n"
        );
        std::fs::remove_dir_all(h).unwrap();
    }
}
