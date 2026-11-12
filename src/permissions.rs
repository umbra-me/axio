//! When tools may run without asking: a mode, as in Claude Code, plus allow
//! and deny rules in Claude Code's syntax (`Bash(git log:*)`, `Edit(src/**)`,
//! `WebFetch(domain:docs.rs)`, `mcp__github`).

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// Asks before commands, file changes, fetches and MCP tools.
    #[default]
    Default,
    /// File changes run without asking; the rest still asks.
    AcceptEdits,
    /// Read-only: the model researches and proposes; changes are refused.
    Plan,
    /// Never asks.
    Bypass,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s {
            "default" => Mode::Default,
            "acceptEdits" => Mode::AcceptEdits,
            "plan" => Mode::Plan,
            "bypassPermissions" => Mode::Bypass,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::AcceptEdits => "acceptEdits",
            Mode::Plan => "plan",
            Mode::Bypass => "bypassPermissions",
        }
    }

    /// What the status line says, empty in the default mode.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Default => "",
            Mode::AcceptEdits => "accept edits",
            Mode::Plan => "plan mode",
            Mode::Bypass => "bypassing permissions",
        }
    }

    /// Shift+Tab: default → accept edits → plan → default. Bypass (only from
    /// a flag or setting) steps back to default.
    pub fn next(self) -> Mode {
        match self {
            Mode::Default => Mode::AcceptEdits,
            Mode::AcceptEdits => Mode::Plan,
            Mode::Plan | Mode::Bypass => Mode::Default,
        }
    }
}

/// `Tool` or `Tool(specifier)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rule {
    pub tool: String,
    pub spec: Option<String>,
}

impl Rule {
    pub fn parse(text: &str) -> Option<Rule> {
        let text = text.trim();
        let (tool, spec) = match text.split_once('(') {
            Some((tool, rest)) => (tool, Some(rest.strip_suffix(')')?.trim().to_string())),
            None => (text, None),
        };
        if tool.is_empty()
            || !tool
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '*' || c == '-')
        {
            return None;
        }
        Some(Rule {
            tool: tool.to_string(),
            spec: spec.filter(|s| !s.is_empty() && s != "*"),
        })
    }
}

impl std::fmt::Display for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.spec {
            Some(s) => write!(f, "{}({s})", self.tool),
            None => write!(f, "{}", self.tool),
        }
    }
}

/// Whether `rule` covers this call.
pub fn matches(rule: &Rule, tool: &str, input: &Value, cwd: &Path) -> bool {
    if let Some(server) = rule.tool.strip_prefix("mcp__") {
        let server = server.trim_end_matches("__*");
        return match server.split_once("__") {
            Some(_) => rule.tool == tool,
            None => tool
                .strip_prefix("mcp__")
                .and_then(|t| t.strip_prefix(server))
                .is_some_and(|rest| rest.starts_with("__")),
        };
    }
    // Edit rules cover every tool that changes files.
    let same_tool = rule.tool == tool || (rule.tool == "Edit" && tool == "Write");
    if !same_tool {
        return false;
    }
    let Some(spec) = &rule.spec else {
        return true;
    };
    match tool {
        "Bash" => {
            let command = input["command"].as_str().unwrap_or("");
            let parts = command_parts(command);
            !parts.is_empty()
                && !unsafe_to_match(command)
                && parts.iter().all(|p| bash_matches(spec, p))
        }
        "Read" | "Edit" | "Write" => input["file_path"]
            .as_str()
            .is_some_and(|f| path_matches(spec, f, cwd)),
        "WebFetch" => match (
            spec.strip_prefix("domain:"),
            host(input["url"].as_str().unwrap_or("")),
        ) {
            (Some(d), Some(h)) => h == d || h.ends_with(&format!(".{d}")),
            _ => false,
        },
        _ => false,
    }
}

/// A command's parts, split at `&&`, `||`, `;`, `|`, `&` and new lines.
fn command_parts(command: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '\n' | '|' | '&' => {
                if matches!((c, chars.peek()), ('|', Some('|')) | ('&', Some('&'))) {
                    chars.next();
                }
                parts.push(String::new());
            }
            c => parts.last_mut().expect("never empty").push(c),
        }
    }
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Commands a prefix rule must never wave through: substitutions run
/// other commands, and redirections write files.
fn unsafe_to_match(command: &str) -> bool {
    command.contains("$(") || command.contains('`') || command.contains('>')
}

/// `git log:*` is the prefix `git log` (as a whole word); anything else
/// must match exactly.
fn bash_matches(spec: &str, part: &str) -> bool {
    match spec.strip_suffix(":*") {
        Some(prefix) => {
            part == prefix
                || part
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with(char::is_whitespace))
        }
        None => part == spec,
    }
}

fn host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next()?;
    (!host.is_empty()).then(|| host.to_lowercase())
}

/// `path` made absolute against `cwd`, with `.` and `..` worked out.
fn normalize(path: &str, cwd: &Path) -> PathBuf {
    let joined = cwd.join(path);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// `.gitignore`-style: `/x` is from the working directory, a pattern with a
/// slash is relative to it, and one without a slash matches at any depth.
fn path_matches(spec: &str, file: &str, cwd: &Path) -> bool {
    let full = normalize(file, cwd);
    let Ok(rel) = full.strip_prefix(cwd) else {
        return false;
    };
    let rel: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let anchored = spec.starts_with('/') || spec.trim_start_matches('/').contains('/');
    let pat: Vec<&str> = spec.trim_start_matches('/').split('/').collect();
    if anchored {
        glob(&pat, &rel)
    } else {
        rel.last().is_some_and(|name| wild(pat[0], name))
    }
}

/// Path segments against pattern segments, where `**` is any number of them.
fn glob(pat: &[&str], path: &[String]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|i| glob(rest, &path[i..])),
        Some((p, rest)) => path
            .split_first()
            .is_some_and(|(s, tail)| wild(p, s) && glob(rest, tail)),
    }
}

/// One segment: `*` is any run of characters, `?` any one.
fn wild(pat: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(&p, &s)
}

#[derive(Debug, PartialEq)]
pub enum Verdict {
    Allow,
    Deny(String),
    /// Ask the user `question`; `rule` is what "don't ask again" would save.
    Ask {
        question: String,
        rule: Option<Rule>,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Permissions {
    pub mode: Mode,
    pub allow: Vec<Rule>,
    pub deny: Vec<Rule>,
}

impl Permissions {
    pub fn decide(&self, tool: &str, input: &Value, cwd: &Path) -> Verdict {
        if let Some(r) = self.deny.iter().find(|r| denies(r, tool, input, cwd)) {
            return Verdict::Deny(format!("denied by the rule {r}"));
        }
        if matches!(
            tool,
            "Read" | "Grep" | "Glob" | "TodoWrite" | "TaskStop" | "Agent"
        ) {
            return Verdict::Allow;
        }
        let changes = matches!(tool, "Write" | "Edit" | "Bash");
        match self.mode {
            Mode::Plan if changes => {
                return Verdict::Deny(
                    "plan mode: files and commands are read-only; propose a plan instead, and the user will switch modes to carry it out"
                        .into(),
                );
            }
            Mode::Bypass => return Verdict::Allow,
            Mode::AcceptEdits if matches!(tool, "Write" | "Edit") => return Verdict::Allow,
            _ => {}
        }
        if self.allows(tool, input, cwd) {
            return Verdict::Allow;
        }
        let s = |k: &str| input[k].as_str().unwrap_or("").to_string();
        let first_line = |t: &str| -> String {
            let line = t.lines().next().unwrap_or("");
            if line.chars().count() > 120 {
                format!("{}…", line.chars().take(119).collect::<String>())
            } else if t.lines().count() > 1 {
                format!("{line} …")
            } else {
                line.to_string()
            }
        };
        let (question, rule) = match tool {
            "Bash" => {
                let command = s("command");
                let parts = command_parts(&command);
                let rule = (parts.len() == 1 && !unsafe_to_match(&command))
                    .then(|| {
                        let mut words = parts[0].split_whitespace();
                        let first = words.next()?;
                        let prefix = match words.next() {
                            Some(w)
                                if w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                                    && !w.starts_with('-') =>
                            {
                                format!("{first} {w}")
                            }
                            _ => first.to_string(),
                        };
                        Some(Rule {
                            tool: "Bash".into(),
                            spec: Some(format!("{prefix}:*")),
                        })
                    })
                    .flatten();
                (format!("Run `{}`?", first_line(&command)), rule)
            }
            "Write" | "Edit" => (
                format!("{} {}?", tool, s("file_path")),
                Some(Rule {
                    tool: "Edit".into(),
                    spec: None,
                }),
            ),
            "WebFetch" => {
                let url = s("url");
                let rule = host(&url).map(|h| Rule {
                    tool: "WebFetch".into(),
                    spec: Some(format!("domain:{h}")),
                });
                (format!("Fetch {url}?"), rule)
            }
            t => {
                let shown = t
                    .strip_prefix("mcp__")
                    .and_then(|r| r.split_once("__"))
                    .map_or(t.to_string(), |(server, name)| format!("{server} · {name}"));
                (
                    format!("Use {shown}?"),
                    Some(Rule {
                        tool: t.into(),
                        spec: None,
                    }),
                )
            }
        };
        Verdict::Ask { question, rule }
    }
}

impl Permissions {
    /// Every part of a command must be covered, each by any allow rule.
    fn allows(&self, tool: &str, input: &Value, cwd: &Path) -> bool {
        if tool != "Bash" {
            return self.allow.iter().any(|r| matches(r, tool, input, cwd));
        }
        let command = input["command"].as_str().unwrap_or("");
        if self
            .allow
            .iter()
            .any(|r| r.tool == "Bash" && r.spec.is_none())
        {
            return true;
        }
        let parts = command_parts(command);
        !parts.is_empty()
            && !unsafe_to_match(command)
            && parts.iter().all(|p| {
                self.allow.iter().any(|r| {
                    r.tool == "Bash" && r.spec.as_deref().is_some_and(|spec| bash_matches(spec, p))
                })
            })
    }
}

/// A deny rule refuses a command if it covers any part of it.
fn denies(rule: &Rule, tool: &str, input: &Value, cwd: &Path) -> bool {
    if tool == "Bash"
        && rule.tool == "Bash"
        && let Some(spec) = &rule.spec
    {
        let command = input["command"].as_str().unwrap_or("");
        return command_parts(command).iter().any(|p| bash_matches(spec, p));
    }
    matches(rule, tool, input, cwd)
}

/// Rules saved with "Yes, always in this project", kept in the user's own
/// `permissions.json` rather than the repository, so a cloned project cannot
/// grant itself permissions.
pub fn project_rules(home: &Path, root: &Path) -> Vec<Rule> {
    let doc: Value = std::fs::read_to_string(home.join("permissions.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    doc["projects"][root.display().to_string()]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| Rule::parse(v.as_str()?))
        .collect()
}

pub fn save_project_rule(home: &Path, root: &Path, rule: &Rule) -> Result<(), String> {
    let path = home.join("permissions.json");
    let mut doc: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({"projects": {}}));
    if !doc["projects"].is_object() {
        doc["projects"] = serde_json::json!({});
    }
    let key = root.display().to_string();
    let list = doc["projects"][&key]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let text = rule.to_string();
    if !list.iter().any(|v| v.as_str() == Some(&text)) {
        let mut list = list;
        list.push(Value::String(text));
        doc["projects"][&key] = Value::Array(list);
    }
    crate::private::dir(home).map_err(|e| e.to_string())?;
    crate::private::write(&path, format!("{doc:#}\n").as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(s: &str) -> Rule {
        Rule::parse(s).unwrap()
    }

    fn cwd() -> PathBuf {
        PathBuf::from("/work/app")
    }

    fn bash(c: &str) -> Value {
        json!({"command": c})
    }

    #[test]
    fn modes_parse_name_and_cycle() {
        for (s, m) in [
            ("default", Mode::Default),
            ("acceptEdits", Mode::AcceptEdits),
            ("plan", Mode::Plan),
            ("bypassPermissions", Mode::Bypass),
        ] {
            assert_eq!(Mode::parse(s), Some(m));
            assert_eq!(m.name(), s);
        }
        assert_eq!(Mode::parse("yolo"), None);
        assert_eq!(Mode::Default.next(), Mode::AcceptEdits);
        assert_eq!(Mode::AcceptEdits.next(), Mode::Plan);
        assert_eq!(Mode::Plan.next(), Mode::Default);
        assert_eq!(Mode::Bypass.next(), Mode::Default);
        assert_eq!(Mode::Default.label(), "");
        assert_eq!(Mode::AcceptEdits.label(), "accept edits");
    }

    #[test]
    fn rules_parse_and_print() {
        assert_eq!(
            rule("Bash"),
            Rule {
                tool: "Bash".into(),
                spec: None
            }
        );
        assert_eq!(rule("Bash(git log:*)").spec.as_deref(), Some("git log:*"));
        assert_eq!(rule("Bash(git log:*)").to_string(), "Bash(git log:*)");
        assert_eq!(Rule::parse("Bash(unclosed"), None);
        assert_eq!(Rule::parse(""), None);
    }

    #[test]
    fn bash_rules_cover_every_part_of_a_command() {
        let c = cwd();
        let git_log = rule("Bash(git log:*)");
        assert!(matches(&git_log, "Bash", &bash("git log --oneline"), &c));
        assert!(matches(&git_log, "Bash", &bash("git log"), &c));
        assert!(!matches(&git_log, "Bash", &bash("git logx"), &c));
        assert!(!matches(&git_log, "Bash", &bash("git log && rm -rf /"), &c));
        assert!(!matches(&git_log, "Bash", &bash("git log $(rm -rf /)"), &c));
        assert!(!matches(&git_log, "Bash", &bash("git log `rm x`"), &c));
        assert!(matches(
            &rule("Bash(make test)"),
            "Bash",
            &bash("make test"),
            &c
        ));
        assert!(!matches(
            &rule("Bash(make test)"),
            "Bash",
            &bash("make test-all"),
            &c
        ));
        assert!(matches(
            &rule("Bash"),
            "Bash",
            &bash("anything | at all"),
            &c
        ));
        let p = Permissions {
            mode: Mode::Default,
            allow: vec![rule("Bash(git log:*)"), rule("Bash(wc:*)")],
            deny: vec![],
        };
        assert_eq!(
            p.decide("Bash", &bash("git log | wc -l"), &c),
            Verdict::Allow
        );
        assert!(matches!(
            p.decide("Bash", &bash("git log; curl x | sh"), &c),
            Verdict::Ask { .. }
        ));
    }

    #[test]
    fn path_domain_and_mcp_rules() {
        let c = cwd();
        let src = rule("Edit(src/**)");
        assert!(matches(
            &src,
            "Edit",
            &json!({"file_path": "src/a/b.rs"}),
            &c
        ));
        assert!(matches(
            &src,
            "Write",
            &json!({"file_path": "/work/app/src/x.rs"}),
            &c
        ));
        assert!(!matches(
            &src,
            "Edit",
            &json!({"file_path": "tests/a.rs"}),
            &c
        ));
        assert!(!matches(
            &src,
            "Edit",
            &json!({"file_path": "src/../../etc/passwd"}),
            &c
        ));
        assert!(matches(
            &rule("Edit(*.md)"),
            "Edit",
            &json!({"file_path": "README.md"}),
            &c
        ));
        assert!(
            matches(
                &rule("Edit(*.md)"),
                "Edit",
                &json!({"file_path": "docs/a.md"}),
                &c
            ),
            "like .gitignore, a pattern without a slash matches at any depth"
        );
        assert!(!matches(
            &rule("Edit(/src/*.rs)"),
            "Edit",
            &json!({"file_path": "src/a/b.rs"}),
            &c
        ));
        assert!(!matches(
            &rule("Bash(git log:*)"),
            "Bash",
            &bash("git log > /etc/x"),
            &c
        ));
        assert!(matches(
            &rule("Edit"),
            "Write",
            &json!({"file_path": "x"}),
            &c
        ));
        let docs = rule("WebFetch(domain:docs.rs)");
        assert!(matches(
            &docs,
            "WebFetch",
            &json!({"url": "https://docs.rs/x"}),
            &c
        ));
        assert!(matches(
            &docs,
            "WebFetch",
            &json!({"url": "https://www.docs.rs/"}),
            &c
        ));
        assert!(!matches(
            &docs,
            "WebFetch",
            &json!({"url": "https://evildocs.rs/"}),
            &c
        ));
        assert!(matches(
            &rule("mcp__mobbin"),
            "mcp__mobbin__search",
            &json!({}),
            &c
        ));
        assert!(matches(
            &rule("mcp__mobbin__*"),
            "mcp__mobbin__search",
            &json!({}),
            &c
        ));
        assert!(matches(
            &rule("mcp__mobbin__search"),
            "mcp__mobbin__search",
            &json!({}),
            &c
        ));
        assert!(!matches(
            &rule("mcp__mobbin"),
            "mcp__mobbinx__search",
            &json!({}),
            &c
        ));
    }

    #[test]
    fn modes_and_rules_decide_together() {
        let c = cwd();
        let edit = json!({"file_path": "a.rs", "old_string": "a", "new_string": "b"});
        let mut p = Permissions::default();
        for read_only in ["Read", "Grep", "Glob", "TodoWrite", "Agent"] {
            assert_eq!(
                p.decide(read_only, &json!({}), &c),
                Verdict::Allow,
                "{read_only}"
            );
        }
        let Verdict::Ask {
            question,
            rule: suggested,
        } = p.decide("Bash", &bash("git push origin main"), &c)
        else {
            panic!("default mode asks")
        };
        assert!(question.contains("git push origin main"), "{question}");
        assert_eq!(suggested.unwrap().to_string(), "Bash(git push:*)");
        assert!(matches!(p.decide("Edit", &edit, &c), Verdict::Ask { .. }));
        p.mode = Mode::AcceptEdits;
        assert_eq!(p.decide("Edit", &edit, &c), Verdict::Allow);
        assert!(matches!(
            p.decide("Bash", &bash("ls"), &c),
            Verdict::Ask { .. }
        ));
        p.mode = Mode::Plan;
        assert!(
            matches!(p.decide("Edit", &edit, &c), Verdict::Deny(why) if why.contains("plan mode"))
        );
        assert!(matches!(
            p.decide("Bash", &bash("ls"), &c),
            Verdict::Deny(_)
        ));
        assert_eq!(p.decide("Read", &json!({}), &c), Verdict::Allow);
        p.mode = Mode::Bypass;
        assert_eq!(p.decide("Bash", &bash("rm -rf build"), &c), Verdict::Allow);
        p.deny = vec![rule("Bash(rm:*)")];
        assert!(
            matches!(
                p.decide("Bash", &bash("ls && rm -rf build"), &c),
                Verdict::Deny(_)
            ),
            "deny rules hold even when bypassing"
        );
        let Verdict::Ask {
            rule: suggested, ..
        } = Permissions::default().decide("WebFetch", &json!({"url": "https://docs.rs/serde"}), &c)
        else {
            panic!("asks")
        };
        assert_eq!(suggested.unwrap().to_string(), "WebFetch(domain:docs.rs)");
    }

    #[test]
    fn project_rules_live_in_the_users_home() {
        let home = std::env::temp_dir().join(format!("axio-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let root = Path::new("/work/app");
        assert!(project_rules(&home, root).is_empty());
        save_project_rule(&home, root, &rule("Bash(cargo test:*)")).unwrap();
        save_project_rule(&home, root, &rule("Bash(cargo test:*)")).unwrap();
        assert_eq!(project_rules(&home, root), [rule("Bash(cargo test:*)")]);
        assert!(project_rules(&home, Path::new("/other")).is_empty());
    }
}
