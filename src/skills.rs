//! Skills and slash commands: markdown files in the Claude Code layout.
//!
//! A skill is `skills/<name>/SKILL.md`; the system prompt lists its name,
//! description and path, and the model reads the file when a task matches. A
//! command is `commands/<name>.md`; `/name args` sends its body as the prompt.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The `---` block at the top of a markdown file and the text after it.
/// Reads the YAML these files use in practice: `key: value`, quoted values,
/// and `>`, `>-`, `|`, `|-` block values. Nested keys are skipped.
pub fn frontmatter(text: &str) -> (HashMap<String, String>, &str) {
    let mut fields = HashMap::new();
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (fields, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (fields, text);
    };
    let body = rest[end + 4..].trim_start_matches(['-', '\r']);
    let body = body.strip_prefix('\n').unwrap_or(body);
    let lines: Vec<&str> = rest[..end].lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.starts_with([' ', '\t']) || line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let value = if matches!(value, ">" | ">-" | "|" | "|-" | ">+" | "|+") {
            let mut block = Vec::new();
            while i < lines.len()
                && (lines[i].starts_with([' ', '\t']) || lines[i].trim().is_empty())
            {
                block.push(lines[i].trim());
                i += 1;
            }
            let sep = if value.starts_with('>') { " " } else { "\n" };
            block.join(sep).trim().to_string()
        } else {
            let unquoted = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            unquoted.to_string()
        };
        fields.insert(key.trim().to_string(), value);
    }
    (fields, body)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// The user can run it as `/name`.
    pub invocable: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    pub description: String,
    pub template: String,
}

/// Skills and commands from several folders; the first folder to define a
/// name wins, and later duplicates are reported.
#[derive(Debug, Default)]
pub struct Library {
    pub skills: Vec<Skill>,
    pub commands: Vec<Command>,
    pub shadowed: Vec<String>,
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    v.sort();
    v
}

impl Library {
    /// `dir/skills/*/SKILL.md` and `dir/commands/*.md`. Plugin commands are
    /// also reachable as `/plugin:name`.
    pub fn add_dir(&mut self, dir: &Path, plugin: Option<&str>) {
        for skill_dir in sorted_entries(&dir.join("skills")) {
            let file = skill_dir.join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let (f, _) = frontmatter(&text);
            let dir_name = skill_dir.file_name().unwrap_or_default().to_string_lossy();
            let name = f
                .get("name")
                .cloned()
                .unwrap_or_else(|| dir_name.to_string());
            if self.skills.iter().any(|s| s.name == name) {
                self.shadowed
                    .push(format!("skill {name} at {}", file.display()));
                continue;
            }
            self.skills.push(Skill {
                name,
                description: f.get("description").cloned().unwrap_or_default(),
                invocable: f.get("user-invocable").is_some_and(|v| v == "true"),
                path: file,
            });
        }
        for file in sorted_entries(&dir.join("commands")) {
            if file.extension().is_none_or(|e| e != "md") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let (f, body) = frontmatter(&text);
            let stem = file
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let mut names = vec![stem.clone()];
            if let Some(p) = plugin {
                names.push(format!("{p}:{stem}"));
            }
            for name in names {
                if self.commands.iter().any(|c| c.name == name) {
                    self.shadowed
                        .push(format!("command /{name} at {}", file.display()));
                    continue;
                }
                self.commands.push(Command {
                    name,
                    description: command_description(&f, body),
                    template: body.to_string(),
                });
            }
        }
    }

    /// The prompt for `/name args`, or `None` when no command or invocable
    /// skill has that name.
    pub fn expand(&self, name: &str, args: &str) -> Option<String> {
        if let Some(c) = self.commands.iter().find(|c| c.name == name) {
            return Some(fill(&c.template, args));
        }
        let s = self.skills.iter().find(|s| s.invocable && s.name == name)?;
        let mut prompt = format!(
            "Use the {} skill: read {} first and follow it.",
            s.name,
            s.path.display()
        );
        if !args.is_empty() {
            prompt.push_str(&format!("\n\nArguments: {args}"));
        }
        Some(prompt)
    }

    /// The system prompt section listing skills, one line each.
    pub fn prompt_section(&self) -> Option<String> {
        if self.skills.is_empty() {
            return None;
        }
        let mut s = String::from(
            "Skills: instructions for particular tasks. When a task matches a skill's description, read its file with Read before you start, and follow it.",
        );
        for k in &self.skills {
            let description: String = k.description.chars().take(300).collect();
            s.push_str(&format!(
                "\n- {}: {} ({})",
                k.name,
                description,
                k.path.display()
            ));
        }
        Some(s)
    }
}

/// `$ARGUMENTS` takes all of `args`, `$1`..`$9` take single words. A template
/// with neither gets the arguments appended.
fn fill(template: &str, args: &str) -> String {
    let words: Vec<&str> = args.split_whitespace().collect();
    let has =
        template.contains("$ARGUMENTS") || (1..=9).any(|i| template.contains(&format!("${i}")));
    let mut out = template.replace("$ARGUMENTS", args);
    for i in (1..=9).rev() {
        out = out.replace(&format!("${i}"), words.get(i - 1).copied().unwrap_or(""));
    }
    if !has && !args.is_empty() {
        out = format!("{}\n\n{args}", out.trim_end());
    }
    out.trim().to_string()
}

/// The frontmatter description, or else the body's first line, so `/help`
/// says something about every command.
fn command_description(f: &HashMap<String, String>, body: &str) -> String {
    if let Some(d) = f.get("description") {
        return d.clone();
    }
    let first = body
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("");
    match first.char_indices().nth(77) {
        Some((i, _)) => format!("{}…", first[..i].trim_end()),
        None => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_frontmatter_real_skills_use() {
        let text = "---\nname: shipit\ndescription: >-\n  Carries work to a\n  merged change.\nversion: 4.3.1\nuser-invocable: true\nargument-hint: \"[a · b]\"\nmetadata:\n  rex-stub: \"2\"\n---\n\n# Body\n";
        let (f, body) = frontmatter(text);
        assert_eq!(f["description"], "Carries work to a merged change.");
        assert_eq!(f["argument-hint"], "[a · b]");
        assert_eq!(f["user-invocable"], "true");
        assert_eq!(f["metadata"], "", "nested keys are skipped");
        assert!(!f.contains_key("rex-stub"));
        assert_eq!(body, "\n# Body\n");
        let (f, body) = frontmatter("no frontmatter");
        assert!(f.is_empty() && body == "no frontmatter");
        let (f, _) = frontmatter("---\ndescription: |\n  line one\n  line two\n---\n");
        assert_eq!(f["description"], "line one\nline two");
    }

    #[test]
    fn commands_without_a_description_show_their_first_line() {
        let (f, body) =
            frontmatter("Count the lines in $ARGUMENTS with `wc -l`.\nReply with the number.");
        assert_eq!(
            command_description(&f, body),
            "Count the lines in $ARGUMENTS with `wc -l`."
        );
        let (f, body) = frontmatter("---\ndescription: Review a file\n---\nReview it.");
        assert_eq!(command_description(&f, body), "Review a file");
        let long = format!("# {}", "word ".repeat(40));
        let (f, body) = frontmatter(&long);
        assert!(command_description(&f, body).chars().count() <= 80);
    }

    #[test]
    fn fills_arguments() {
        assert_eq!(
            fill("Review $ARGUMENTS now", "src/a.rs b.rs"),
            "Review src/a.rs b.rs now"
        );
        assert_eq!(fill("Move $1 to $2", "a b c"), "Move a to b");
        assert_eq!(
            fill("Summarize the diff.\n", "focus on tests"),
            "Summarize the diff.\n\nfocus on tests"
        );
        assert_eq!(fill("Summarize the diff.", ""), "Summarize the diff.");
    }

    #[test]
    fn discovers_skills_commands_and_plugin_names() {
        let root = std::env::temp_dir().join(format!("axio-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |p: &str, t: &str| {
            let p = root.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, t).unwrap();
        };
        write(
            "project/skills/pdf/SKILL.md",
            "---\nname: pdf\ndescription: Fill PDF forms\n---\nSteps",
        );
        write(
            "project/commands/review.md",
            "---\ndescription: Review a file\n---\nReview $ARGUMENTS carefully.",
        );
        write("home/commands/review.md", "global review, shadowed");
        write("home/skills/pdf/SKILL.md", "---\nname: pdf\n---\nshadowed");
        write(
            "home/skills/ship/SKILL.md",
            "---\nname: ship\ndescription: Ship a change\nuser-invocable: true\n---\n",
        );
        write(
            "plugins/tools/.claude-plugin/plugin.json",
            r#"{"name": "toolbox"}"#,
        );
        write("plugins/tools/commands/fmt.md", "Format the code.");

        let mut lib = Library::default();
        lib.add_dir(&root.join("project"), None);
        lib.add_dir(&root.join("home"), None);
        for p in crate::plugins::list(&root.join("plugins"), None) {
            lib.add_dir(&p.root, Some(&p.name));
        }

        let skills: Vec<&str> = lib.skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(skills, ["pdf", "ship"]);
        assert_eq!(lib.shadowed.len(), 2);
        assert_eq!(
            lib.expand("review", "a.rs").as_deref(),
            Some("Review a.rs carefully.")
        );
        assert_eq!(lib.expand("fmt", "").as_deref(), Some("Format the code."));
        assert_eq!(
            lib.expand("toolbox:fmt", "").as_deref(),
            Some("Format the code.")
        );
        assert!(
            lib.expand("ship", "now")
                .unwrap()
                .ends_with("Arguments: now")
        );
        assert_eq!(
            lib.expand("pdf", ""),
            None,
            "skills are commands only when user-invocable"
        );
        let section = lib.prompt_section().unwrap();
        assert!(section.contains("- pdf: Fill PDF forms ("));
        std::fs::remove_dir_all(root).unwrap();
    }
}
