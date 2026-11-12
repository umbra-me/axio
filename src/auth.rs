//! `~/.axio/agent/auth.json`: provider keys and MCP sign-in tokens, readable
//! only by the user. Environment variables still win over the file.
//!
//! ```json
//! {"anthropic": {"apiKey": "sk-ant-…"},
//!  "openrouter": {"apiKey": "…"},
//!  "local": {"baseUrl": "http://localhost:11434/v1"},
//!  "mcp": {"https://server/mcp": {"access_token": "…"}}}
//! ```

use serde_json::{Map, Value};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Uses `home/auth.json`, folding in an older `mcp-auth.json` once.
pub fn set_home(home: &Path) {
    migrate(home);
    let _ = PATH.set(home.join("auth.json"));
}

fn migrate(home: &Path) {
    let path = home.join("auth.json");
    let old = home.join("mcp-auth.json");
    if let Ok(text) = std::fs::read_to_string(&old)
        && let Ok(Value::Object(tokens)) = serde_json::from_str(&text)
    {
        let mut mcp = section(&path, "mcp");
        for (k, v) in tokens {
            mcp.entry(k).or_insert(v);
        }
        if set_section(&path, "mcp", mcp).is_ok() {
            let _ = std::fs::remove_file(&old);
        }
    }
}

pub fn path() -> Option<&'static PathBuf> {
    PATH.get()
}

pub fn read(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Writes the whole file with 0600 permissions, in a 0700 folder when new.
pub fn write(path: &Path, all: &Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent()
        && !dir.exists()
    {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut f = options
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    writeln!(f, "{:#}", Value::Object(all.clone())).map_err(|e| e.to_string())
}

pub fn section(path: &Path, key: &str) -> Map<String, Value> {
    read(path)
        .get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

pub fn set_section(path: &Path, key: &str, value: Map<String, Value>) -> Result<(), String> {
    let mut all = read(path);
    all.insert(key.to_string(), Value::Object(value));
    write(path, &all)
}

/// The environment variable if set, else `provider.field` from the file.
pub fn get(env: &str, provider: &str, field: &str) -> Option<String> {
    get_from(path().map(PathBuf::as_path), env, provider, field)
}

fn get_from(path: Option<&Path>, env: &str, provider: &str, field: &str) -> Option<String> {
    std::env::var(env)
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| {
            let all = read(path?);
            all.get(provider)?[field]
                .as_str()
                .filter(|v| !v.is_empty())
                .map(String::from)
        })
}

/// Saves `provider.field`, keeping the provider's other fields.
pub fn save(path: &Path, provider: &str, field: &str, value: &str) -> Result<(), String> {
    let mut all = read(path);
    let entry = all
        .entry(provider.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry[field] = Value::String(value.to_string());
    write(path, &all)
}

/// Removes a provider's saved entry; `false` when there was none.
pub fn forget(path: &Path, provider: &str) -> Result<bool, String> {
    let mut all = read(path);
    if all.remove(provider).is_none() {
        return Ok(false);
    }
    write(path, &all)?;
    Ok(true)
}

/// Providers with something saved, leaving out MCP sign-ins.
pub fn saved(path: &Path) -> Vec<String> {
    read(path)
        .into_iter()
        .map(|(k, _)| k)
        .filter(|k| k != "mcp")
        .collect()
}

/// Whether `provider` lacks the key it needs. Providers axio has no
/// built-in endpoint for treat their key as optional.
pub fn missing_key(provider: &str) -> bool {
    match provider {
        "anthropic" => {
            get("ANTHROPIC_API_KEY", provider, "apiKey").is_none()
                && get("ANTHROPIC_AUTH_TOKEN", provider, "authToken").is_none()
        }
        "openai" | "openrouter" => {
            let env = format!("{}_API_KEY", provider.to_uppercase());
            get(&env, provider, "apiKey").is_none()
        }
        _ => false,
    }
}

/// First run without a key: asks for one and saves it to auth.json.
pub fn first_run(provider: &str, home: &Path) -> Result<(), String> {
    let path = home.join("auth.json");
    let env = format!("{}_API_KEY", provider.to_uppercase());
    eprintln!("axio needs an API key for {provider}.");
    eprintln!(
        "Paste it below; it is saved to {} (readable only by you).\nOr press Enter, then set {env}.",
        path.display()
    );
    let value = read_secret("API key: ", true)?;
    if value.is_empty() {
        return Err(format!(
            "no key entered; set {env} or run `axio auth set {provider}`"
        ));
    }
    let mut all = read(&path);
    let entry = all
        .entry(provider.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry["apiKey"] = Value::String(value);
    write(&path, &all)?;
    eprintln!("saved; `axio auth list` shows it and `axio auth remove {provider}` deletes it\n");
    Ok(())
}

fn mask(v: &str) -> String {
    let tail: String = v
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("…{tail}")
}

const USAGE: &str = "Usage: axio auth list
       axio auth set <provider> [apiKey|authToken|baseUrl]   (reads the value from stdin)
       axio auth remove <provider>";

/// `axio auth list | set | remove`. Values are read from stdin (hidden on a
/// terminal) and never printed in full.
pub fn main(args: &[String], home: &Path) -> Result<(), String> {
    let path = home.join("auth.json");
    let all = read(&path);
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["list"] | [] => {
            if all.is_empty() {
                println!("{} has no entries", path.display());
            }
            for (provider, v) in &all {
                if provider == "mcp" {
                    let n = v.as_object().map_or(0, Map::len);
                    println!("mcp  {n} signed-in server(s)");
                    continue;
                }
                let fields: Vec<String> = v
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, val)| {
                        let s = val.as_str().unwrap_or("");
                        if k == "baseUrl" {
                            format!("{k} {s}")
                        } else {
                            format!("{k} {}", mask(s))
                        }
                    })
                    .collect();
                println!("{provider}  {}", fields.join(", "));
            }
            Ok(())
        }
        ["set", provider, rest @ ..] => {
            if *provider == "mcp" {
                return Err("use `axio mcp login <name>` for MCP servers".into());
            }
            let field = match rest {
                [] => "apiKey",
                ["apiKey" | "authToken" | "baseUrl"] => rest[0],
                _ => return Err(USAGE.into()),
            };
            let value = read_secret(&format!("{field} for {provider}: "), field != "baseUrl")?;
            if value.is_empty() {
                return Err("nothing entered; nothing changed".into());
            }
            save(&path, provider, field, &value)?;
            println!("saved {provider} {field} to {}", path.display());
            Ok(())
        }
        ["remove", provider] => {
            if !forget(&path, provider)? {
                return Err(format!("{provider} is not in {}", path.display()));
            }
            println!("removed {provider}");
            Ok(())
        }
        _ => Err(USAGE.into()),
    }
}

/// One line from stdin; on a terminal, typed without echo.
fn read_secret(prompt: &str, hidden: bool) -> Result<String, String> {
    let tty = std::io::stdin().is_terminal();
    let stty = |arg: &str| {
        let _ = std::process::Command::new("stty")
            .arg(arg)
            .stdin(std::process::Stdio::inherit())
            .status();
    };
    if tty {
        // Echo goes off before the prompt shows, so nothing typed or pasted
        // right away is displayed.
        if hidden {
            stty("-echo");
        }
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
    }
    let mut line = String::new();
    let read = std::io::stdin().read_line(&mut line);
    if tty && hidden {
        stty("echo");
        eprintln!();
    }
    read.map_err(|e| e.to_string())?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_forget_and_list_providers() {
        let dir = std::env::temp_dir().join(format!("axio-auth-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        save(&path, "local", "baseUrl", "http://127.0.0.1:1234/v1").unwrap();
        save(&path, "local", "apiKey", "k1").unwrap();
        set_section(&path, "mcp", Map::new()).unwrap();
        assert_eq!(
            get_from(Some(&path), "AXIO_TEST_UNSET", "local", "baseUrl").as_deref(),
            Some("http://127.0.0.1:1234/v1")
        );
        assert_eq!(saved(&path), vec!["local".to_string()]);
        assert!(forget(&path, "local").unwrap());
        assert!(!forget(&path, "local").unwrap());
        assert!(saved(&path).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folds_in_the_old_token_file() {
        let home = std::env::temp_dir().join(format!("axio-auth-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("mcp-auth.json"),
            r#"{"https://s/mcp": {"access_token": "old"}}"#,
        )
        .unwrap();
        write(
            &home.join("auth.json"),
            &serde_json::from_str(r#"{"anthropic": {"apiKey": "k"}}"#).unwrap(),
        )
        .unwrap();
        migrate(&home);
        assert!(!home.join("mcp-auth.json").exists());
        let all = read(&home.join("auth.json"));
        assert_eq!(all["mcp"]["https://s/mcp"]["access_token"], "old");
        assert_eq!(all["anthropic"]["apiKey"], "k");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn writes_privately_and_keeps_sections_apart() {
        let dir = std::env::temp_dir().join(format!("axio-auth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("auth.json");
        let mut all = Map::new();
        all.insert(
            "openai".into(),
            serde_json::json!({"apiKey": "sk-1234567890"}),
        );
        write(&path, &all).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let mut mcp = Map::new();
        mcp.insert(
            "https://x/mcp".into(),
            serde_json::json!({"access_token": "t"}),
        );
        set_section(&path, "mcp", mcp).unwrap();
        let back = read(&path);
        assert_eq!(
            back["openai"]["apiKey"], "sk-1234567890",
            "other sections survive"
        );
        assert_eq!(section(&path, "mcp")["https://x/mcp"]["access_token"], "t");
        assert_eq!(mask("sk-1234567890"), "…7890");
        let from_file = get_from(Some(&path), "AXIO_TEST_UNSET_KEY", "openai", "apiKey");
        assert_eq!(from_file.as_deref(), Some("sk-1234567890"));
        // SAFETY: test-only variable that no other test reads.
        unsafe { std::env::set_var("AXIO_TEST_SET_KEY", "from-env") };
        let from_env = get_from(Some(&path), "AXIO_TEST_SET_KEY", "openai", "apiKey");
        assert_eq!(
            from_env.as_deref(),
            Some("from-env"),
            "the environment wins"
        );
        assert_eq!(
            get_from(Some(&path), "AXIO_TEST_UNSET_KEY", "anthropic", "apiKey"),
            None
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
