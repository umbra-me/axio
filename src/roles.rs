//! Sub-agent roles: markdown files in Claude Code's agent format, so agents
//! written for Claude Code work unchanged. axio ships four (`agents/*.md` in
//! this repository); files in `~/.axio/agent/agents/`, plugins' `agents/`
//! folders and a trusted project's `.axio/agents/` add or replace them.

use crate::child::{Confine, Sees, Spec};
use crate::provider::Model;
use crate::settings::Subagents;
use std::path::{Path, PathBuf};

const BUILT_IN: [(&str, &str); 4] = [
    ("explore", include_str!("../agents/explore.md")),
    ("research", include_str!("../agents/research.md")),
    ("review", include_str!("../agents/review.md")),
    ("code", include_str!("../agents/code.md")),
];

/// Tools a sub-agent's process can offer.
pub const CHILD_TOOLS: [&str; 8] = [
    "Read",
    "Write",
    "Edit",
    "Bash",
    "Grep",
    "Glob",
    "TodoWrite",
    "WebFetch",
];

/// Where a model or effort comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum Pick {
    /// The main agent's current one.
    Inherit,
    /// The main agent picks per call.
    Choose,
    /// Fixed: a model (`provider/model`, or `fast`) or an effort level.
    Value(String),
}

impl Pick {
    fn parse(s: &str) -> Pick {
        match s.trim() {
            "" | "inherit" => Pick::Inherit,
            "choose" => Pick::Choose,
            v => Pick::Value(v.to_string()),
        }
    }

    /// Claude Code's model aliases: haiku is the fast model; sonnet and
    /// opus mean the main agent's model.
    fn model(s: &str) -> Pick {
        match s.trim() {
            "haiku" => Pick::Value("fast".into()),
            "sonnet" | "opus" => Pick::Inherit,
            v => Pick::parse(v),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Role {
    pub name: String,
    pub description: String,
    /// `None`: the main agent's tools.
    pub tools: Option<Vec<String>>,
    pub model: Pick,
    pub effort: Pick,
    pub max_turns: Option<usize>,
    pub context: Sees,
    /// `None`: the same as the main agent.
    pub sandbox: Option<Confine>,
    pub instructions: String,
    /// "built in" or the file it came from.
    pub source: String,
}

fn sees(s: &str) -> Result<Sees, String> {
    match s.trim() {
        "brief" => Ok(Sees::Brief),
        "rules" => Ok(Sees::Rules),
        "fork" => Ok(Sees::Fork),
        other => Err(format!("context must be brief, rules or fork, not {other}")),
    }
}

fn confine(s: &str) -> Result<Option<Confine>, String> {
    match s.trim() {
        "" | "inherit" => Ok(None),
        "read-only" => Ok(Some(Confine::ReadOnly)),
        "project" => Ok(Some(Confine::Project)),
        other => Err(format!(
            "sandbox must be read-only, project or inherit, not {other}"
        )),
    }
}

/// A role file. Claude Code fields axio does not use are ignored.
pub fn parse(text: &str, fallback_name: &str, source: String) -> Result<Role, String> {
    let (f, body) = crate::skills::frontmatter(text);
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");
    let tools = f.get("tools").map(|t| {
        t.trim_matches(['[', ']'])
            .split(',')
            .map(|n| n.trim().trim_matches(['"', '\'']).to_string())
            .filter(|n| !n.is_empty())
            .collect()
    });
    let max_turns = match f.get("maxTurns") {
        None => None,
        Some(v) => Some(
            v.trim()
                .parse()
                .map_err(|_| format!("maxTurns must be a number, not {v}"))?,
        ),
    };
    Ok(Role {
        name: match get("name").trim() {
            "" => fallback_name.to_string(),
            n => n.to_string(),
        },
        description: get("description").to_string(),
        tools,
        model: Pick::model(get("model")),
        effort: Pick::parse(get("effort")),
        max_turns,
        context: match get("context") {
            "" => Sees::Brief,
            c => sees(c)?,
        },
        sandbox: confine(get("sandbox"))?,
        instructions: body.trim().to_string(),
        source,
    })
}

/// The built-in roles, then each folder's `*.md` files in order; a role
/// replaces an earlier one of the same name. Returns the roles and the files
/// that could not be read.
pub fn load(dirs: &[PathBuf]) -> (Vec<Role>, Vec<String>) {
    let mut roles: Vec<Role> = BUILT_IN
        .iter()
        .map(|(name, text)| parse(text, name, "built in".into()).expect("built-in roles parse"))
        .collect();
    let mut errors = Vec::new();
    for dir in dirs {
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "md"))
            .collect();
        files.sort();
        for file in files {
            let stem = file.file_stem().unwrap_or_default().to_string_lossy();
            let parsed = std::fs::read_to_string(&file)
                .map_err(|e| e.to_string())
                .and_then(|t| parse(&t, &stem, file.display().to_string()));
            match parsed {
                Ok(r) => {
                    roles.retain(|old| old.name != r.name);
                    roles.push(r);
                }
                Err(e) => errors.push(format!("{}: {e}", file.display())),
            }
        }
    }
    (roles, errors)
}

/// Folders holding role files, lowest priority first: plugins' `agents/`,
/// then `~/.axio/agent/agents/`, then a trusted project's `.axio/agents/`.
pub fn dirs(home: &Path, project: Option<&Path>, plugin_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = plugin_roots.iter().map(|r| r.join("agents")).collect();
    v.push(home.join("agents"));
    if let Some(root) = project {
        v.push(root.join(".axio/agents"));
    }
    v
}

/// What the main agent brings to a sub-agent call.
pub struct Parent<'a> {
    pub model: &'a Model,
    pub effort: Option<&'a str>,
    pub cwd: &'a Path,
    /// The main agent's tools, for roles that list none.
    pub tools: &'a [String],
    /// Whether the main agent's commands run in the sandbox.
    pub sandboxed: bool,
    /// Whether the sub-agent may start sub-agents of its own.
    pub may_nest: bool,
    /// The main agent's MCP tools, which it runs for sub-agents.
    pub mcp: &'a [crate::tools::ToolSpec],
}

/// One request for a sub-agent.
#[derive(Clone, Debug, Default)]
pub struct Call {
    pub task: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub context: Option<Sees>,
}

/// The model `fast` means: the setting, else a small model from the main
/// agent's provider, else the main agent's model.
pub fn fast_model(settings: &Subagents, parent: &Model) -> String {
    if let Some(m) = &settings.fast_model {
        return m.clone();
    }
    match parent.provider.as_str() {
        "anthropic" => "anthropic/claude-haiku-4-5".into(),
        crate::chatgpt::PROVIDER => format!("{}/gpt-6-luna", crate::chatgpt::PROVIDER),
        _ => full_name(parent),
    }
}

fn full_name(m: &Model) -> String {
    format!("{}/{}", m.provider, m.id)
}

/// The child's task spec for `call` in `role`, and notes for the main agent
/// about anything it asked for that the role does not allow.
pub fn spec(
    role: &Role,
    call: &Call,
    parent: &Parent,
    settings: &Subagents,
) -> Result<(Spec, Vec<String>), String> {
    let mut notes = Vec::new();
    let over = settings.roles.get(&role.name).cloned().unwrap_or_default();
    let model_pick = over
        .model
        .as_deref()
        .map_or(role.model.clone(), Pick::model);
    let effort_pick = over
        .effort
        .as_deref()
        .map_or(role.effort.clone(), Pick::parse);
    let resolve_model = |m: &str| {
        if m == "fast" {
            fast_model(settings, parent.model)
        } else {
            m.to_string()
        }
    };
    let model = match (&model_pick, &call.model) {
        (Pick::Choose, Some(asked)) => {
            let m = resolve_model(asked);
            if let Some(choices) = &settings.choices
                && !choices.contains(&m)
            {
                return Err(format!(
                    "the {} role may use only these models: {}",
                    role.name,
                    choices.join(", ")
                ));
            }
            m
        }
        (Pick::Choose | Pick::Inherit, _) => full_name(parent.model),
        (Pick::Value(v), _) => resolve_model(v),
    };
    if call.model.is_some() && model_pick != Pick::Choose {
        notes.push(format!(
            "the {} role sets its own model, so it runs on {model}",
            role.name
        ));
    }
    let effort = match (&effort_pick, &call.effort) {
        (Pick::Choose, Some(asked)) => Some(asked.clone()),
        (Pick::Choose | Pick::Inherit, _) => parent.effort.map(String::from),
        (Pick::Value(v), _) => Some(v.clone()),
    };
    if call.effort.is_some() && effort_pick != Pick::Choose {
        notes.push(format!("the {} role sets its own effort", role.name));
    }
    let context = match (call.context, &over.context) {
        (Some(c), _) => c,
        (None, Some(c)) => sees(c)?,
        (None, None) => role.context,
    };
    let wanted: Vec<String> = match &role.tools {
        Some(list) => list.clone(),
        None => parent.tools.to_vec(),
    };
    // `mcp__server__*` stands for all of that server's tools.
    let wanted: Vec<String> = wanted
        .into_iter()
        .flat_map(|t| match t.strip_suffix('*') {
            Some(prefix) if t.starts_with("mcp__") => parent
                .mcp
                .iter()
                .filter(|m| m.name.starts_with(prefix))
                .map(|m| m.name.clone())
                .collect(),
            _ => vec![t],
        })
        .collect();
    let mut tools = Vec::new();
    for t in wanted {
        if t.starts_with("mcp__") {
            if parent.mcp.iter().any(|m| m.name == t) {
                if !tools.contains(&t) {
                    tools.push(t);
                }
            } else if role.tools.is_some() {
                notes.push(format!("no MCP tool named {t} is running; left out"));
            }
        } else if t == "Agent" && !parent.may_nest {
            if role.tools.is_some() {
                notes.push(
                    "sub-agents cannot start sub-agents here (subagents.nesting); Agent left out"
                        .into(),
                );
            }
        } else if CHILD_TOOLS.contains(&t.as_str()) || t == "Agent" {
            if !tools.contains(&t) {
                tools.push(t);
            }
        } else if role.tools.is_some() {
            notes.push(format!("a sub-agent cannot use {t}; left out"));
        }
    }
    let sandbox = match over.sandbox.as_deref() {
        Some(s) => confine(s)?,
        None => role.sandbox,
    };
    let sandbox = match (sandbox, parent.sandboxed) {
        (None, true) => Some(Confine::Project),
        (s, _) => s,
    };
    Ok((
        Spec {
            task: call.task.clone(),
            instructions: role.instructions.clone(),
            cwd: Some(parent.cwd.to_path_buf()),
            model: Some(model),
            effort,
            context,
            max_turns: over.max_turns.or(role.max_turns),
            result_limit: settings.result_limit,
            session_dir: None,
            sandbox,
            history: None,
            resume: None,
            fast: settings.fast.unwrap_or(false),
            permissions: None,
            can_ask: false,
            mcp: parent
                .mcp
                .iter()
                .filter(|m| tools.contains(&m.name))
                .cloned()
                .collect(),
            tools,
        },
        notes,
    ))
}

#[cfg(test)]
mod tests {
    use super::spec as resolve;
    use super::*;
    use crate::settings::RoleSettings;

    fn role(text: &str) -> Role {
        parse(text, "file", "test".into()).unwrap()
    }

    fn parent<'a>(model: &'a Model, tools: &'a [String]) -> Parent<'a> {
        Parent {
            model,
            effort: Some("high"),
            cwd: Path::new("/work"),
            tools,
            sandboxed: false,
            may_nest: false,
            mcp: &[],
        }
    }

    #[test]
    fn built_in_roles_parse() {
        let (roles, errors) = load(&[]);
        assert!(errors.is_empty(), "{errors:?}");
        let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["explore", "research", "review", "code"]);
        let explore = &roles[0];
        assert_eq!(explore.tools.as_deref().unwrap(), ["Read", "Grep", "Glob"]);
        assert_eq!(explore.model, Pick::Inherit);
        for r in &roles {
            assert_eq!(
                (&r.model, r.effort != Pick::Value("low".into())),
                (&Pick::Inherit, true),
                "built-in {} runs on the main model unless settings say otherwise",
                r.name
            );
        }
        assert_eq!(explore.sandbox, Some(Confine::ReadOnly));
        assert!(explore.instructions.starts_with("You search"));
        assert_eq!(roles[2].context, Sees::Rules);
        assert_eq!(roles[3].effort, Pick::Inherit);
        assert_eq!(roles[3].source, "built in");
    }

    #[test]
    fn claude_code_agents_work_unchanged() {
        let r = role(
            "---\nname: tester\ndescription: Writes tests\ntools: Read, Edit\nmodel: haiku\ncolor: blue\n---\nWrite tests.\n",
        );
        assert_eq!(r.name, "tester");
        assert_eq!(r.model, Pick::Value("fast".into()));
        assert_eq!(r.effort, Pick::Inherit);
        assert_eq!(r.context, Sees::Brief);
        assert_eq!(r.instructions, "Write tests.");
        assert_eq!(role("---\nmodel: opus\n---\nx").model, Pick::Inherit);
        let all = role("---\ndescription: d\n---\nx");
        assert_eq!((all.name.as_str(), all.tools), ("file", None));
        assert_eq!(
            role("---\ntools: [Read, Grep]\n---\n").tools.unwrap(),
            ["Read", "Grep"]
        );
        assert!(parse("---\ncontext: everything\n---\n", "f", "t".into()).is_err());
        assert!(parse("---\nmaxTurns: lots\n---\n", "f", "t".into()).is_err());
    }

    #[test]
    fn later_folders_replace_roles_by_name() {
        let dir = std::env::temp_dir().join(format!("axio-roles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["a", "b"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        std::fs::write(dir.join("a/explore.md"), "---\nmodel: inherit\n---\nmine").unwrap();
        std::fs::write(dir.join("a/doc.md"), "---\nname: doc\n---\nfirst").unwrap();
        std::fs::write(dir.join("b/doc.md"), "---\nname: doc\n---\nsecond").unwrap();
        std::fs::write(dir.join("b/bad.md"), "---\ncontext: x\n---\n").unwrap();
        std::fs::write(dir.join("b/notes.txt"), "not a role").unwrap();
        let (roles, errors) = load(&[dir.join("a"), dir.join("b"), dir.join("missing")]);
        let explore = roles.iter().find(|r| r.name == "explore").unwrap();
        assert_eq!(explore.instructions, "mine");
        assert_eq!(
            explore.source,
            dir.join("a").join("explore.md").display().to_string()
        );
        let doc = roles.iter().find(|r| r.name == "doc").unwrap();
        assert_eq!(doc.instructions, "second");
        assert_eq!(roles.iter().filter(|r| r.name == "doc").count(), 1);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("bad.md"), "{errors:?}");
    }

    #[test]
    fn fast_follows_the_setting_then_the_provider() {
        let mut s = Subagents::default();
        assert_eq!(
            fast_model(&s, &Model::parse("anthropic/claude-opus-5-5")),
            "anthropic/claude-haiku-4-5"
        );
        assert_eq!(
            fast_model(&s, &Model::parse("chatgpt/gpt-6")),
            "chatgpt/gpt-6-luna"
        );
        assert_eq!(fast_model(&s, &Model::parse("openai/o9")), "openai/o9");
        s.fast_model = Some("openrouter/x/y".into());
        assert_eq!(fast_model(&s, &Model::parse("openai/o9")), "openrouter/x/y");
    }

    #[test]
    fn spec_resolves_model_effort_tools_and_sandbox() {
        let model = Model::parse("anthropic/claude-opus-5-5");
        let tools: Vec<String> = ["Read", "Bash", "mcp__x__y", "TaskStop"]
            .map(String::from)
            .to_vec();
        let p = parent(&model, &tools);
        let call = Call {
            task: "find it".into(),
            ..Call::default()
        };
        let s = Subagents::default();
        let (roles, _) = load(&[]);
        let by = |n: &str| roles.iter().find(|r| r.name == n).unwrap();

        let (spec, notes) = resolve(by("explore"), &call, &p, &s).unwrap();
        assert_eq!(spec.task, "find it");
        assert_eq!(spec.model.as_deref(), Some("anthropic/claude-opus-5-5"));
        assert_eq!(spec.effort.as_deref(), Some("high"));
        assert_eq!(spec.tools, ["Read", "Grep", "Glob"]);
        assert_eq!(spec.sandbox, Some(Confine::ReadOnly));
        assert_eq!(spec.cwd.as_deref(), Some(Path::new("/work")));
        assert!(notes.is_empty());

        // inherit takes the main agent's model and effort.
        let (spec, _) = resolve(by("code"), &call, &p, &s).unwrap();
        assert_eq!(spec.model.as_deref(), Some("anthropic/claude-opus-5-5"));
        assert_eq!(spec.effort.as_deref(), Some("high"));
        assert_eq!(spec.sandbox, None);

        // A role that fixes its model ignores the main agent's pick.
        let asked = Call {
            model: Some("anthropic/claude-opus-5-5".into()),
            ..call.clone()
        };
        let fixed = role("---\nname: quick\nmodel: fast\n---\nx");
        let (spec, notes) = resolve(&fixed, &asked, &p, &s).unwrap();
        assert_eq!(spec.model.as_deref(), Some("anthropic/claude-haiku-4-5"));
        assert_eq!(notes.len(), 1, "{notes:?}");

        // A role with no tool list gets the main agent's tools it can run.
        let open = role("---\nname: open\n---\nx");
        let (spec, notes) = resolve(&open, &call, &p, &s).unwrap();
        assert_eq!(spec.tools, ["Read", "Bash"]);
        assert!(
            notes.is_empty(),
            "MCP and background tools are left out quietly"
        );

        // Tools the child cannot run are dropped with a note.
        let odd = role("---\ntools: Read, NotebookEdit\n---\nx");
        let (spec, notes) = resolve(&odd, &call, &p, &s).unwrap();
        assert_eq!(spec.tools, ["Read"]);
        assert!(notes[0].contains("NotebookEdit"), "{notes:?}");

        // Never looser than the main agent's sandbox.
        let boxed = Parent {
            sandboxed: true,
            ..parent(&model, &tools)
        };
        let (spec, _) = resolve(by("code"), &call, &boxed, &s).unwrap();
        assert_eq!(spec.sandbox, Some(Confine::Project));
        let (spec, _) = resolve(by("review"), &call, &boxed, &s).unwrap();
        assert_eq!(spec.sandbox, Some(Confine::ReadOnly));
    }

    #[test]
    fn agent_reaches_a_sub_agent_only_when_nesting_allows() {
        let model = Model::parse("anthropic/claude-opus-5-5");
        let tools: Vec<String> = vec!["Read".into(), "Agent".into()];
        let lead = role("---\ntools: Read, Agent\n---\nx");
        let call = Call {
            task: "t".into(),
            ..Call::default()
        };
        let s = Subagents::default();
        let (sp, notes) = resolve(&lead, &call, &parent(&model, &tools), &s).unwrap();
        assert_eq!(sp.tools, ["Read"]);
        assert!(notes[0].contains("nesting"), "{notes:?}");
        let nested = Parent {
            may_nest: true,
            ..parent(&model, &tools)
        };
        let (sp, _) = resolve(&lead, &call, &nested, &s).unwrap();
        assert_eq!(sp.tools, ["Read", "Agent"]);
        // A role with no list inherits Agent only when nesting allows, quietly.
        let open = role("---\nname: open\n---\nx");
        let (sp, notes) = resolve(&open, &call, &parent(&model, &tools), &s).unwrap();
        assert_eq!((sp.tools, notes.len()), (vec!["Read".to_string()], 0));
    }

    #[test]
    fn mcp_tools_come_from_the_main_agents_servers() {
        let model = Model::parse("anthropic/claude-opus-5-5");
        let spec_of = |n: &str| crate::tools::ToolSpec {
            name: n.into(),
            description: String::new(),
            schema: serde_json::json!({}),
        };
        let mcp = [
            spec_of("mcp__a__one"),
            spec_of("mcp__a__two"),
            spec_of("mcp__b__x"),
        ];
        let tools: Vec<String> = mcp.iter().map(|m| m.name.clone()).collect();
        let p = Parent {
            mcp: &mcp,
            ..parent(&model, &tools)
        };
        let call = Call {
            task: "t".into(),
            ..Call::default()
        };
        let s = Subagents::default();
        let r = role("---\ntools: Read, mcp__a__*, mcp__c__gone\n---\nx");
        let (sp, notes) = resolve(&r, &call, &p, &s).unwrap();
        assert_eq!(sp.tools, ["Read", "mcp__a__one", "mcp__a__two"]);
        let names: Vec<&str> = sp.mcp.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["mcp__a__one", "mcp__a__two"]);
        assert!(notes[0].contains("mcp__c__gone"), "{notes:?}");
        // A role with no list gets the main agent's MCP tools too.
        let (sp, _) = resolve(&role("---\nname: open\n---\nx"), &call, &p, &s).unwrap();
        assert_eq!(sp.mcp.len(), 3);
    }

    #[test]
    fn choose_lets_the_main_agent_pick_within_choices() {
        let model = Model::parse("anthropic/claude-opus-5-5");
        let tools: Vec<String> = vec!["Read".into()];
        let p = parent(&model, &tools);
        let picky = role("---\nname: picky\nmodel: choose\neffort: choose\n---\nx");
        let mut s = Subagents::default();
        let call = |m: Option<&str>, e: Option<&str>| Call {
            task: "t".into(),
            model: m.map(String::from),
            effort: e.map(String::from),
            context: None,
        };
        let (spec_, _) = resolve(&picky, &call(None, None), &p, &s).unwrap();
        assert_eq!(spec_.model.as_deref(), Some("anthropic/claude-opus-5-5"));
        assert_eq!(spec_.effort.as_deref(), Some("high"));
        let (spec_, _) = resolve(&picky, &call(Some("fast"), Some("low")), &p, &s).unwrap();
        assert_eq!(spec_.model.as_deref(), Some("anthropic/claude-haiku-4-5"));
        assert_eq!(spec_.effort.as_deref(), Some("low"));
        s.choices = Some(vec!["chatgpt/gpt-6-luna".into()]);
        assert!(resolve(&picky, &call(Some("openai/o9"), None), &p, &s).is_err());
        assert!(resolve(&picky, &call(Some("chatgpt/gpt-6-luna"), None), &p, &s).is_ok());
    }

    #[test]
    fn settings_override_the_role_file() {
        let model = Model::parse("anthropic/claude-opus-5-5");
        let tools: Vec<String> = vec!["Read".into()];
        let p = parent(&model, &tools);
        let (roles, _) = load(&[]);
        let explore = roles.iter().find(|r| r.name == "explore").unwrap();
        let mut s = Subagents {
            result_limit: Some(500),
            ..Subagents::default()
        };
        s.roles.insert(
            "explore".into(),
            RoleSettings {
                model: Some("chatgpt/gpt-6-luna".into()),
                effort: Some("medium".into()),
                context: Some("rules".into()),
                max_turns: Some(5),
                sandbox: None,
            },
        );
        let call = Call {
            task: "t".into(),
            ..Call::default()
        };
        let (spec, _) = resolve(explore, &call, &p, &s).unwrap();
        assert_eq!(spec.model.as_deref(), Some("chatgpt/gpt-6-luna"));
        assert_eq!(spec.effort.as_deref(), Some("medium"));
        assert_eq!(spec.context, Sees::Rules);
        assert_eq!((spec.max_turns, spec.result_limit), (Some(5), Some(500)));
        assert!(!spec.fast, "sub-agents are not fast unless asked");
        let s = Subagents {
            fast: Some(true),
            ..s
        };
        assert!(
            resolve(
                explore,
                &Call {
                    task: "t".into(),
                    ..Call::default()
                },
                &p,
                &s
            )
            .unwrap()
            .0
            .fast
        );
        // A per-call context wins over both.
        let call = Call {
            context: Some(Sees::Brief),
            ..call
        };
        assert_eq!(spec_context(explore, &call, &p, &s), Sees::Brief);
    }

    fn spec_context(r: &Role, c: &Call, p: &Parent, s: &Subagents) -> Sees {
        resolve(r, c, p, s).unwrap().0.context
    }
}
