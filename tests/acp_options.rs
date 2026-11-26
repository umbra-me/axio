//! `_axio/options`: what a new session would offer, asked before any session.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

#[test]
fn options_come_before_a_session_and_start_nothing() {
    let home = std::env::temp_dir().join(format!("axio-acp-options-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let project = home.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(
        home.join("settings.json"),
        json!({"model": "anthropic/claude-opus-5-5", "effort": "low",
               "permissions": {"defaultMode": "plan"}})
        .to_string(),
    )
    .unwrap();
    // An MCP server that leaves a mark when something starts it.
    let mark = home.join("mcp-started");
    std::fs::write(
        home.join("mcp.json"),
        json!({"mcpServers": {"mark": {"command": "sh",
            "args": ["-c", format!("touch '{}'; cat", mark.display())]}}})
        .to_string(),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_axio"))
        .arg("acp")
        .current_dir(&project)
        .env("AXIO_HOME", &home)
        .env_remove("AXIO_MODEL")
        .env_remove("AXIO_EFFORT")
        .env_remove("AXIO_PERMISSION_MODE")
        .env_remove("AXIO_PROFILE")
        .env_remove("AXIO_DEPTH")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut ask = |id: u64, method: &str, params: Value| -> Value {
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(out.read_line(&mut line).unwrap() > 0, "axio acp ended");
            let v: Value = serde_json::from_str(&line).unwrap();
            if v["id"] == id && v.get("method").is_none() {
                return v;
            }
        }
    };
    let init = ask(1, "initialize", json!({"protocolVersion": 1}));
    assert_eq!(
        init["result"]["agentCapabilities"]["_meta"]["axio"]["options"],
        true
    );
    let r = ask(2, "_axio/options", json!({"cwd": project}));
    let _ = child.kill();
    let _ = child.wait();

    let options = r["result"]["configOptions"]
        .as_array()
        .unwrap_or_else(|| panic!("{r:#}"));
    let current = |id: &str| {
        options
            .iter()
            .find(|o| o["id"] == id)
            .map(|o| o["currentValue"].clone())
    };
    assert_eq!(current("model"), Some(json!("anthropic/claude-opus-5-5")));
    assert_eq!(current("effort"), Some(json!("low")));
    assert_eq!(current("mode"), Some(json!("plan")));
    assert!(
        !home.join("sessions").exists(),
        "a session file was written"
    );
    assert!(!mark.exists(), "an MCP server was started");

    let relative = {
        let mut c = Command::new(env!("CARGO_BIN_EXE_axio"))
            .arg("acp")
            .env("AXIO_HOME", &home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut i = c.stdin.take().unwrap();
        writeln!(
            i,
            "{}",
            json!({"jsonrpc": "2.0", "id": 1, "method": "_axio/options", "params": {"cwd": "here"}})
        )
        .unwrap();
        drop(i);
        let mut line = String::new();
        BufReader::new(c.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let _ = c.wait();
        serde_json::from_str::<Value>(&line).unwrap()
    };
    assert!(relative["error"].is_object(), "{relative:#}");
    std::fs::remove_dir_all(home).unwrap();
}
