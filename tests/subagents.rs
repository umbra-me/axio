//! Sub-agents end to end: a real axio as the main agent, real `axio agent`
//! children, and a local OpenAI-compatible server standing in for the model.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
struct Seen {
    at: Instant,
    body: Value,
}

type Reply = (Duration, String);

/// Serves every connection on its own thread; `reply` decides each answer
/// (a delay and an SSE body) from the request.
fn serve(reply: fn(&Value) -> Reply) -> (u16, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                let body: Value = serde_json::from_slice(&body).unwrap();
                log.lock().unwrap().push(Seen {
                    at: Instant::now(),
                    body: body.clone(),
                });
                let (delay, sse) = reply(&body);
                std::thread::sleep(delay);
                let mut stream = stream;
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{sse}"
                );
            });
        }
    });
    (port, seen)
}

fn sse(delta: Value, finish: &str) -> String {
    format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        json!({"choices": [{"index": 0, "delta": delta, "finish_reason": null}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": finish}]})
    )
}

fn text(t: &str) -> String {
    sse(json!({"content": t}), "stop")
}

fn calls(list: &[(&str, &str, Value)]) -> String {
    let tool_calls: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, (id, name, args))| {
            json!({"index": i, "id": id, "type": "function",
                   "function": {"name": name, "arguments": args.to_string()}})
        })
        .collect();
    sse(json!({"tool_calls": tool_calls}), "tool_calls")
}

fn system(body: &Value) -> &str {
    body["messages"][0]["content"].as_str().unwrap_or("")
}

fn is_child(body: &Value) -> bool {
    system(body).starts_with("You are a sub-agent")
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| t["function"]["name"].as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn first_user(body: &Value) -> String {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "user")
        .map(|m| m["content"].to_string())
        .unwrap_or_default()
}

/// A home with the stub server's address and key in auth.json.
fn home(name: &str, port: u16) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("axio-sub-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project")).unwrap();
    std::fs::write(
        dir.join("auth.json"),
        format!(r#"{{"stub": {{"baseUrl": "http://127.0.0.1:{port}/v1", "apiKey": "k"}}}}"#),
    )
    .unwrap();
    dir
}

fn axio(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_axio"))
        .args(args)
        .current_dir(home.join("project"))
        .env("AXIO_HOME", home)
        .env_remove("AXIO_MODEL")
        .env_remove("AXIO_PROFILE")
        .env_remove("AXIO_DEPTH")
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn two_explorers(body: &Value) -> Reply {
    if is_child(body) {
        let task = first_user(body);
        let which = if task.contains("find A") { "A" } else { "B" };
        return (
            Duration::from_millis(400),
            text(&format!("child found {which}")),
        );
    }
    let answered = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["role"] == "tool");
    if answered {
        return (Duration::ZERO, text("both done"));
    }
    let agent = |what: &str| {
        json!({"description": format!("look {what}"), "prompt": format!("find {what}"),
               "subagent_type": "explore"})
    };
    (
        Duration::ZERO,
        calls(&[("a", "Agent", agent("A")), ("b", "Agent", agent("B"))]),
    )
}

#[test]
fn two_agent_calls_run_at_once_and_answer_the_main_agent() {
    let (port, seen) = serve(two_explorers);
    let home = home("two", port);
    let out = axio(&home, &["-p", "--no-session", "-m", "stub/main", "go"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("both done"), "{stdout}\n{stderr}");
    let seen = seen.lock().unwrap().clone();
    let main: Vec<&Seen> = seen.iter().filter(|s| !is_child(&s.body)).collect();
    let children: Vec<&Seen> = seen.iter().filter(|s| is_child(&s.body)).collect();
    assert!(tool_names(&main[0].body).contains(&"Agent".to_string()));
    assert_eq!(children.len(), 2);
    for c in &children {
        assert_eq!(tool_names(&c.body), ["Read", "Grep", "Glob"]);
    }
    let gap = if children[0].at > children[1].at {
        children[0].at - children[1].at
    } else {
        children[1].at - children[0].at
    };
    assert!(
        gap < Duration::from_millis(300),
        "both children asked within the other's 400ms reply: {gap:?}"
    );
    let results: Vec<String> = main[1].body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(results.len(), 2);
    assert!(results[0].starts_with("child found A"), "{results:?}");
    assert!(results[1].starts_with("child found B"), "{results:?}");
    assert!(
        results[0].contains("[sub-agent transcript: "),
        "{results:?}"
    );
    assert!(stderr.contains("✓ explore · look A"), "{stderr}");
}

fn minimal_check(_: &Value) -> Reply {
    (Duration::ZERO, text("ok"))
}

#[test]
fn the_minimal_profile_has_no_agent_tool() {
    let (port, seen) = serve(minimal_check);
    let home = home("minimal", port);
    let out = axio(
        &home,
        &[
            "-p",
            "--no-session",
            "--profile",
            "minimal",
            "-m",
            "stub/m",
            "hi",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap();
    assert!(!tool_names(&seen[0].body).contains(&"Agent".to_string()));
}

fn forked(body: &Value) -> Reply {
    if is_child(body) {
        let task = first_user(body);
        let saw = task.contains("the secret word is plum");
        return (Duration::ZERO, text(&format!("saw history: {saw}")));
    }
    let messages = body["messages"].as_array().unwrap();
    if messages.iter().any(|m| m["role"] == "tool") {
        return (Duration::ZERO, text("done"));
    }
    let users = messages.iter().filter(|m| m["role"] == "user").count();
    if users == 1 {
        return (Duration::ZERO, text("noted"));
    }
    (
        Duration::ZERO,
        calls(&[(
            "f",
            "Agent",
            json!({"description": "recall", "prompt": "what was the word?",
                   "subagent_type": "explore", "context": "fork"}),
        )]),
    )
}

#[test]
fn a_forked_sub_agent_sees_the_conversation() {
    let (port, seen) = serve(forked);
    let home = home("fork", port);
    let first = axio(&home, &["-p", "-m", "stub/m", "the secret word is plum"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let out = axio(&home, &["-p", "-c", "-m", "stub/m", "ask a sub-agent"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap();
    let last = seen.iter().rev().find(|s| !is_child(&s.body)).unwrap();
    let result = last.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()["content"]
        .to_string();
    assert!(result.contains("saw history: true"), "{result}");
}

fn slow(_: &Value) -> Reply {
    (Duration::from_secs(20), text("too late"))
}

#[test]
fn closing_an_attached_childs_input_stops_it() {
    let (port, seen) = serve(slow);
    let home = home("stop", port);
    let mut child = Command::new(env!("CARGO_BIN_EXE_axio"))
        .args(["agent", "--attached"])
        .env("AXIO_HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"task": "wait", "tools": [], "model": "stub/m", "cwd": {:?}}}"#,
        home.display().to_string()
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while seen.lock().unwrap().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !seen.lock().unwrap().is_empty(),
        "the child asked the model"
    );
    let closed = Instant::now();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    assert!(
        closed.elapsed() < Duration::from_secs(3),
        "{:?}",
        closed.elapsed()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "error", "{stdout}");
    assert!(!out.status.success());
}

#[test]
fn the_parallel_setting_limits_running_sub_agents() {
    let (port, seen) = serve(two_explorers);
    let home = home("serial", port);
    std::fs::write(
        home.join("settings.json"),
        r#"{"subagents": {"parallel": 1}}"#,
    )
    .unwrap();
    let out = axio(&home, &["-p", "--no-session", "-m", "stub/main", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap();
    let children: Vec<&Seen> = seen.iter().filter(|s| is_child(&s.body)).collect();
    assert_eq!(children.len(), 2);
    assert!(
        children[1].at - children[0].at >= Duration::from_millis(350),
        "the second child waited for the first"
    );
}

/// A one-tool MCP server: this test binary re-run with AXIO_TEST_MCP set.
#[test]
fn mcp_stub_server() {
    if std::env::var_os("AXIO_TEST_MCP").is_none() {
        return;
    }
    let mut out = std::io::stdout();
    for line in std::io::stdin().lock().lines() {
        let Ok(msg) = serde_json::from_str::<Value>(&line.unwrap()) else {
            continue;
        };
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let result = match msg["method"].as_str().unwrap_or("") {
            "initialize" => json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                                   "serverInfo": {"name": "echo"}}),
            "tools/list" => json!({"tools": [{"name": "say", "description": "Says the text back",
                "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}}]}),
            "tools/call" => json!({"content": [{"type": "text",
                "text": format!("said: {} (pid {})", msg["params"]["arguments"]["text"].as_str().unwrap_or(""),
                                std::process::id())}]}),
            _ => continue,
        };
        writeln!(
            out,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        )
        .unwrap();
        out.flush().unwrap();
    }
}

fn mcp_through_parent(body: &Value) -> Reply {
    let messages = body["messages"].as_array().unwrap();
    let tool_said = messages
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap_or("").to_string())
        .next();
    if is_child(body) {
        return match tool_said {
            Some(said) => (Duration::ZERO, text(&format!("child got [{said}]"))),
            None => (
                Duration::ZERO,
                calls(&[("m", "mcp__echo__say", json!({"text": "hi"}))]),
            ),
        };
    }
    match tool_said {
        Some(_) => (Duration::ZERO, text("done")),
        None => (
            Duration::ZERO,
            calls(&[(
                "a",
                "Agent",
                json!({"description": "echo", "prompt": "say hi", "subagent_type": "echoer"}),
            )]),
        ),
    }
}

#[test]
fn sub_agents_use_the_main_agents_mcp_servers() {
    let (port, seen) = serve(mcp_through_parent);
    let home = home("mcp", port);
    let exe = std::env::current_exe().unwrap();
    std::fs::write(
        home.join("mcp.json"),
        json!({"mcpServers": {"echo": {"command": exe.display().to_string(),
            "args": ["--exact", "mcp_stub_server", "--nocapture", "--quiet", "--test-threads=1"],
            "env": {"AXIO_TEST_MCP": "1"}}}})
        .to_string(),
    )
    .unwrap();
    std::fs::create_dir_all(home.join("agents")).unwrap();
    std::fs::write(
        home.join("agents/echoer.md"),
        "---\nname: echoer\ndescription: Says things\ntools: mcp__echo__*\n---\nUse the echo tool.",
    )
    .unwrap();
    let out = axio(&home, &["-p", "--no-session", "-m", "stub/main", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap();
    let child = seen
        .iter()
        .find(|s| is_child(&s.body))
        .expect("a child ran");
    assert_eq!(tool_names(&child.body), ["mcp__echo__say"]);
    let main_last = seen.iter().rev().find(|s| !is_child(&s.body)).unwrap();
    let result = main_last.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(result.contains("child got [said: hi"), "{result}");
}

fn hooked(body: &Value) -> Reply {
    let messages = body["messages"].as_array().unwrap();
    if is_child(body) {
        let all = body["messages"].to_string();
        let saw = all.contains("START-CONTEXT");
        return if all.contains("check again") {
            (
                Duration::ZERO,
                text(&format!("second answer, context {saw}")),
            )
        } else {
            (Duration::ZERO, text("first answer"))
        };
    }
    if messages.iter().any(|m| m["role"] == "tool") {
        return (Duration::ZERO, text("done"));
    }
    (
        Duration::ZERO,
        calls(&[(
            "a",
            "Agent",
            json!({"description": "d", "prompt": "look", "subagent_type": "explore"}),
        )]),
    )
}

#[test]
fn subagent_hooks_add_context_and_can_send_a_sub_agent_back() {
    let (port, seen) = serve(hooked);
    let home = home("hooks", port);
    let mark = home.join("stopped-once");
    let log = home.join("hook-input.jsonl");
    let start = format!(
        r#"cat >> {log}; echo >> {log}; echo '{{"hookSpecificOutput": {{"hookEventName": "SubagentStart", "additionalContext": "START-CONTEXT"}}}}'"#,
        log = log.display()
    );
    let stop = format!(
        r#"cat >> {log}; echo >> {log}; if [ -f {m} ]; then exit 0; fi; touch {m}; echo '{{"decision": "block", "reason": "check again"}}'"#,
        log = log.display(),
        m = mark.display()
    );
    std::fs::write(
        home.join("settings.json"),
        json!({"hooks": {
            "SubagentStart": [{"matcher": "explore", "hooks": [{"type": "command", "command": start}]}],
            "SubagentStop": [{"matcher": "explore", "hooks": [{"type": "command", "command": stop}]}]
        }})
        .to_string(),
    )
    .unwrap();
    let out = axio(&home, &["-p", "--no-session", "-m", "stub/main", "go"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter().filter(|s| is_child(&s.body)).count(),
        2,
        "resumed once"
    );
    let main_last = seen.iter().rev().find(|s| !is_child(&s.body)).unwrap();
    let result = main_last.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        result.starts_with("second answer, context true"),
        "{result}"
    );
    let inputs: Vec<Value> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(inputs[0]["hook_event_name"], "SubagentStart");
    assert_eq!(inputs[0]["agent_type"], "explore");
    assert_eq!(inputs[1]["hook_event_name"], "SubagentStop");
    assert_eq!(inputs[1]["stop_hook_active"], false);
    assert!(
        inputs[1]["agent_transcript_path"]
            .as_str()
            .unwrap()
            .ends_with(".jsonl")
    );
    assert_eq!(inputs[2]["stop_hook_active"], true);
}
