//! `axio plugin install | list | remove | update`. Installing copies or
//! clones a plugin folder and runs nothing; its hooks and servers run only
//! when a session starts, and `install` shows what they are.

use crate::plugins::{self, Plugin};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Command;

const SOURCE_FILE: &str = ".axio-source.json";

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Git {
        url: String,
        reference: Option<String>,
    },
    Local(PathBuf),
}

/// `git:host/owner/repo[@ref]`, a git URL (`https://…`, `ssh://…`,
/// `git@host:owner/repo`) with an optional `@ref` after the last `/`, an
/// existing folder, or `owner/repo` for GitHub.
pub fn parse_source(s: &str, cwd: &Path) -> Source {
    let local = cwd.join(s);
    if !s.starts_with("git:") && local.is_dir() {
        return Source::Local(local);
    }
    let split = |u: &str| -> (String, Option<String>) {
        let slash = u.rfind('/').unwrap_or(0);
        match u[slash..].rfind('@') {
            Some(at) => (
                u[..slash + at].to_string(),
                Some(u[slash + at + 1..].to_string()),
            ),
            None => (u.to_string(), None),
        }
    };
    let (url, reference) = if let Some(rest) = s.strip_prefix("git:") {
        let (u, r) = split(rest);
        (
            if u.contains("://") {
                u
            } else {
                format!("https://{u}")
            },
            r,
        )
    } else if s.contains("://") || s.starts_with("git@") {
        split(s)
    } else {
        let (u, r) = split(s);
        (format!("https://github.com/{u}"), r)
    };
    Source::Git { url, reference }
}

fn git(args: &[&str], dir: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new("git");
    if let Some(d) = dir {
        cmd.arg("-C").arg(d);
    }
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_name() == ".git" {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_dir(&src, &dst)?;
        } else if kind.is_symlink() {
            let target = std::fs::read_link(&src).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(target, &dst).map_err(|e| e.to_string())?;
            #[cfg(not(unix))]
            {
                // Symbolic links need extra rights on Windows: copy the target.
                let target = if target.is_relative() {
                    src.parent().unwrap_or(Path::new(".")).join(target)
                } else {
                    target
                };
                std::fs::copy(&target, &dst).map_err(|e| format!("{}: {e}", target.display()))?;
            }
        } else {
            // fs::copy keeps permissions, so hook scripts stay executable.
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

/// What a plugin will do, for `install` and `list`.
pub fn describe(root: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let read = |p: &str| {
        std::fs::read_to_string(root.join(p))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    };
    if let Some(h) = read("hooks/hooks.json") {
        for (event, groups) in h["hooks"].as_object().into_iter().flatten() {
            for g in groups.as_array().into_iter().flatten() {
                for hook in g["hooks"].as_array().into_iter().flatten() {
                    let matcher = g["matcher"].as_str().filter(|m| !m.is_empty());
                    lines.push(format!(
                        "hook {event}{}: {}",
                        matcher.map_or(String::new(), |m| format!(" [{m}]")),
                        hook["command"].as_str().unwrap_or("?")
                    ));
                }
            }
        }
    }
    if let Some(m) = read(".mcp.json") {
        for (name, s) in m["mcpServers"].as_object().into_iter().flatten() {
            let cmd = s["command"].as_str().map_or_else(
                || s["url"].as_str().unwrap_or("?").to_string(),
                |c| {
                    let args: Vec<&str> = s["args"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect();
                    [c].into_iter().chain(args).collect::<Vec<_>>().join(" ")
                },
            );
            lines.push(format!("mcp server {name}: {cmd}"));
        }
    }
    let count = |dir: &str, file: Option<&str>| {
        std::fs::read_dir(root.join(dir))
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| match file {
                Some(f) => e.path().join(f).is_file(),
                None => e.path().extension().is_some_and(|x| x == "md"),
            })
            .count()
    };
    let (skills, commands) = (count("skills", Some("SKILL.md")), count("commands", None));
    if skills > 0 {
        lines.push(format!("{skills} skill(s)"));
    }
    if commands > 0 {
        lines.push(format!("{commands} command(s)"));
    }
    lines
}

fn read_source(root: &Path) -> Option<Value> {
    std::fs::read_to_string(root.join(SOURCE_FILE))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
}

/// Fetches `source` into a temporary folder inside `dir`, checks it is a
/// plugin, and moves it to `dir/<name>`.
pub fn install(dir: &Path, source: &Source, original: &str) -> Result<Plugin, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Source::Local(path) = source {
        let (from, into) = (path.canonicalize(), dir.canonicalize());
        if let (Ok(from), Ok(into)) = (from, into)
            && into.starts_with(&from)
        {
            return Err("cannot install a folder into itself".into());
        }
    }
    let staging = dir.join(format!(".installing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    let fetched = (|| {
        let record = match source {
            Source::Local(path) => {
                copy_dir(path, &staging)?;
                json!({"source": original, "kind": "local", "path": path})
            }
            Source::Git { url, reference } => {
                let target = staging.display().to_string();
                let mut args = vec!["clone", "--depth", "1", "--quiet"];
                if let Some(r) = reference {
                    args.extend(["--branch", r.as_str()]);
                }
                args.extend([url.as_str(), target.as_str()]);
                git(&args, None)?;
                let commit = git(&["rev-parse", "HEAD"], Some(&staging))?;
                json!({"source": original, "kind": "git", "url": url, "ref": reference, "commit": commit})
            }
        };
        let looks_like_plugin = [
            ".claude-plugin/plugin.json",
            "hooks/hooks.json",
            ".mcp.json",
            "skills",
            "commands",
        ]
        .iter()
        .any(|p| staging.join(p).exists());
        if !looks_like_plugin {
            return Err("this is not a plugin: it has no .claude-plugin/plugin.json, hooks/hooks.json, .mcp.json, skills/ or commands/".to_string());
        }
        std::fs::write(staging.join(SOURCE_FILE), format!("{record:#}\n"))
            .map_err(|e| e.to_string())?;
        Ok(())
    })();
    if let Err(e) = fetched {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let name = plugins::list(dir, None)
        .into_iter()
        .find(|p| p.root == staging)
        .map(|p| p.name)
        .ok_or("could not read the plugin")?;
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("unusable plugin name {name:?}"));
    }
    let target = dir.join(&name);
    if target.exists() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "{name} is already installed at {}; use `axio plugin update {name}`",
            target.display()
        ));
    }
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    plugins::list(dir, None)
        .into_iter()
        .find(|p| p.root == target)
        .ok_or_else(|| "installed, but the plugin could not be read back".into())
}

/// Re-fetches a plugin from where it was installed. A git plugin with local
/// edits is left alone.
pub fn update(dir: &Path, name: &str) -> Result<String, String> {
    let plugin = plugins::list(dir, None)
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("{name} is not installed in {}", dir.display()))?;
    let record = read_source(&plugin.root).ok_or_else(|| {
        format!("{name} was not installed with `axio plugin install`, so axio does not know where to update it from")
    })?;
    match record["kind"].as_str() {
        Some("git") => {
            let dirty = git(
                &["status", "--porcelain", "--untracked-files=no"],
                Some(&plugin.root),
            )?;
            if !dirty.is_empty() {
                return Err(format!(
                    "{name} has local changes; commit or discard them first"
                ));
            }
            let before = record["commit"].as_str().unwrap_or("").to_string();
            let reference = record["ref"].as_str().unwrap_or("HEAD");
            git(
                &["fetch", "--depth", "1", "--quiet", "origin", reference],
                Some(&plugin.root),
            )?;
            git(
                &["reset", "--hard", "--quiet", "FETCH_HEAD"],
                Some(&plugin.root),
            )?;
            let after = git(&["rev-parse", "HEAD"], Some(&plugin.root))?;
            let mut record = record.clone();
            record["commit"] = json!(after);
            std::fs::write(plugin.root.join(SOURCE_FILE), format!("{record:#}\n"))
                .map_err(|e| e.to_string())?;
            Ok(if before == after {
                format!("{name} is up to date ({})", &after[..after.len().min(12)])
            } else {
                format!(
                    "updated {name} {} -> {}",
                    &before[..before.len().min(12)],
                    &after[..after.len().min(12)]
                )
            })
        }
        Some("local") => {
            let path = PathBuf::from(record["path"].as_str().unwrap_or(""));
            if !path.is_dir() {
                return Err(format!(
                    "{name}'s source {} no longer exists",
                    path.display()
                ));
            }
            let staging = dir.join(format!(".updating-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&staging);
            copy_dir(&path, &staging)?;
            std::fs::write(staging.join(SOURCE_FILE), format!("{record:#}\n"))
                .map_err(|e| e.to_string())?;
            std::fs::remove_dir_all(&plugin.root).map_err(|e| e.to_string())?;
            std::fs::rename(&staging, &plugin.root).map_err(|e| e.to_string())?;
            Ok(format!("updated {name} from {}", path.display()))
        }
        _ => Err(format!("{name} has an unreadable {SOURCE_FILE}")),
    }
}

pub fn remove(dir: &Path, name: &str) -> Result<String, String> {
    let plugin = plugins::list(dir, None)
        .into_iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("{name} is not installed in {}", dir.display()))?;
    std::fs::remove_dir_all(&plugin.root).map_err(|e| format!("{}: {e}", plugin.root.display()))?;
    Ok(format!("removed {name} ({})", plugin.root.display()))
}

pub fn list(dirs: &[(&str, PathBuf)], imported: &[crate::import::Item]) -> String {
    let mut out = Vec::new();
    for (scope, dir) in dirs {
        for p in plugins::list(dir, None) {
            let source = read_source(&p.root)
                .and_then(|r| r["source"].as_str().map(String::from))
                .unwrap_or_else(|| "copied by hand".into());
            out.push(format!(
                "{} {} ({scope}, from {source})",
                p.name,
                p.version.as_deref().unwrap_or("unversioned")
            ));
            out.extend(describe(&p.root).into_iter().map(|l| format!("  {l}")));
        }
    }
    for i in imported {
        let crate::import::Kind::Plugin { root, codex } = &i.kind else {
            continue;
        };
        out.push(format!(
            "{} {} (imported from {}, used in place)",
            i.name,
            i.version.as_deref().unwrap_or("unversioned"),
            i.harness
        ));
        // Codex plugins keep these files under `.codex-plugin/`.
        if !codex {
            out.extend(describe(root).into_iter().map(|l| format!("  {l}")));
        }
    }
    if out.is_empty() {
        "no plugins installed".into()
    } else {
        out.join("\n")
    }
}

const USAGE: &str = "Usage: axio plugin install <git-url | owner/repo | git:host/owner/repo[@ref] | path> [--project]
       axio plugin list
       axio plugin update [name] [--project]
       axio plugin remove <name> [--project]";

/// Entry point for `axio plugin …`.
/// `imported` holds the accepted items from other harnesses, when importing is on.
pub fn main(
    args: &[String],
    home: &Path,
    cwd: &Path,
    imported: Vec<crate::import::Item>,
) -> Result<(), String> {
    let project = args.iter().any(|a| a == "--project");
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--project")
        .collect();
    let root = crate::settings::project_root(cwd);
    let dir = if project {
        root.join(".axio/plugins")
    } else {
        home.join("plugins")
    };
    match rest.as_slice() {
        ["install", source] => {
            let p = install(&dir, &parse_source(source, cwd), source)?;
            println!(
                "installed {} {} into {}",
                p.name,
                p.version.as_deref().unwrap_or(""),
                p.root.display()
            );
            let what = describe(&p.root);
            if what.is_empty() {
                println!("it declares nothing axio uses yet");
            }
            for line in what {
                println!("  {line}");
            }
            println!(
                "Hooks and MCP servers run with your permissions from the next session; `axio plugin remove {}` undoes this.",
                p.name
            );
            if project {
                println!("Project plugins load only in trusted projects.");
            }
            Ok(())
        }
        ["list"] => {
            println!(
                "{}",
                list(
                    &[
                        ("global", home.join("plugins")),
                        ("project", root.join(".axio/plugins"))
                    ],
                    &imported
                )
            );
            Ok(())
        }
        ["update"] => {
            for p in plugins::list(&dir, None) {
                match update(&dir, &p.name) {
                    Ok(m) => println!("{m}"),
                    Err(e) => println!("{}: {e}", p.name),
                }
            }
            Ok(())
        }
        ["update", name] => update(&dir, name).map(|m| println!("{m}")),
        ["remove", name] => remove(&dir, name).map(|m| println!("{m}")),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("axio-plugin-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn parses_sources() {
        let cwd = tmp("parse");
        std::fs::create_dir_all(cwd.join("local-plugin")).unwrap();
        assert_eq!(
            parse_source("local-plugin", &cwd),
            Source::Local(cwd.join("local-plugin"))
        );
        let git = |u: &str, r: Option<&str>| Source::Git {
            url: u.into(),
            reference: r.map(String::from),
        };
        assert_eq!(
            parse_source("git:github.com/a/b@v1", &cwd),
            git("https://github.com/a/b", Some("v1"))
        );
        assert_eq!(
            parse_source("a/b", &cwd),
            git("https://github.com/a/b", None)
        );
        assert_eq!(
            parse_source("git@github.com:a/b.git", &cwd),
            git("git@github.com:a/b.git", None)
        );
        assert_eq!(
            parse_source("https://x.dev/a/b.git@main", &cwd),
            git("https://x.dev/a/b.git", Some("main"))
        );
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn installs_lists_updates_and_removes_a_local_plugin() {
        let work = tmp("local");
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/guard");
        let copy = work.join("guard-src");
        copy_dir(&source, &copy).unwrap();
        let dir = work.join("plugins");
        let p = install(&dir, &Source::Local(copy.clone()), "guard-src").unwrap();
        assert_eq!(
            (p.name.as_str(), p.version.as_deref()),
            ("guard", Some("0.1.0"))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(p.root.join("bin/guard"))
                .unwrap()
                .permissions()
                .mode();
            assert!(mode & 0o111 != 0, "hook scripts stay executable");
        }
        assert_eq!(
            describe(&p.root),
            ["hook PreToolUse [Bash]: ${CLAUDE_PLUGIN_ROOT}/bin/guard"]
        );
        assert!(
            install(&dir, &Source::Local(copy.clone()), "again")
                .unwrap_err()
                .contains("already installed")
        );
        std::fs::write(copy.join("bin/extra"), "new").unwrap();
        assert!(
            update(&dir, "guard")
                .unwrap()
                .starts_with("updated guard from")
        );
        assert!(p.root.join("bin/extra").exists());
        let listed = list(&[("global", dir.clone())], &[]);
        assert!(
            listed.starts_with("guard 0.1.0 (global, from guard-src)\n  hook PreToolUse [Bash]")
        );
        assert!(remove(&dir, "guard").unwrap().starts_with("removed guard"));
        assert_eq!(
            list(&[("global", dir.clone())], &[]),
            "no plugins installed"
        );
        let figma = crate::import::Item {
            harness: "Claude Code",
            kind: crate::import::Kind::Plugin {
                root: work.join("guard-src"),
                codex: false,
            },
            label: "plugin",
            name: "figma".into(),
            version: Some("2.2.157".into()),
            support: crate::import::Support::Works,
            fingerprint: String::new(),
        };
        assert!(list(&[("global", dir.clone())], &[figma]).starts_with(
            "figma 2.2.157 (imported from Claude Code, used in place)\n  hook PreToolUse [Bash]"
        ));
        std::fs::create_dir_all(work.join("empty")).unwrap();
        assert!(
            install(&dir, &Source::Local(work.join("empty")), "empty")
                .unwrap_err()
                .contains("not a plugin")
        );
        assert_eq!(
            install(&dir, &Source::Local(work.clone()), "work").unwrap_err(),
            "cannot install a folder into itself"
        );
        std::fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn installs_and_updates_from_git() {
        let work = tmp("git");
        let repo = work.join("repo");
        copy_dir(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins/guard"),
            &repo,
        )
        .unwrap();
        let g = |args: &[&str]| git(args, Some(&repo)).unwrap();
        g(&["init", "--quiet", "-b", "main"]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "add",
            ".",
        ]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "one",
        ]);
        let url = format!("file://{}", repo.display());
        let dir = work.join("plugins");
        let p = install(&dir, &parse_source(&url, &work), &url).unwrap();
        assert_eq!(p.name, "guard");
        assert!(
            update(&dir, "guard")
                .unwrap()
                .starts_with("guard is up to date")
        );
        std::fs::write(repo.join("NOTES.md"), "v2").unwrap();
        g(&["add", "."]);
        g(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "two",
        ]);
        assert!(update(&dir, "guard").unwrap().starts_with("updated guard"));
        assert!(p.root.join("NOTES.md").exists());
        std::fs::write(p.root.join("bin/guard"), "edited").unwrap();
        assert!(update(&dir, "guard").unwrap_err().contains("local changes"));
        std::fs::remove_dir_all(work).unwrap();
    }
}
