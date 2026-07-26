//! A stub provider and the plumbing to drive the real binary against it.
//!
//! Shared by the end-to-end tests so there is one description of the wire
//! format to keep correct.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A stub provider speaking the chat-completions dialect over plain HTTP.
///
/// The first request gets one tool call; every later one gets `answer` as plain
/// text, so a turn that outlives whatever the test is doing still terminates
/// rather than hanging the suite.
pub fn stub_provider(
    tool: &str,
    input: serde_json::Value,
    answer: &str,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
    let port = listener.local_addr().expect("an address").port();
    // Built rather than written out: the arguments field is a JSON document
    // inside a JSON string, and hand-escaping that is how a stub ends up
    // silently returning nothing.
    let arguments = input.to_string();
    let tool_frame = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({
            "choices": [{"delta": {"tool_calls": [{
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": {"name": tool, "arguments": arguments},
            }]}}]
        }),
        serde_json::json!({ "choices": [{"delta": {}, "finish_reason": "tool_calls"}] }),
    );
    let text_frame = format!(
        "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        serde_json::json!({ "choices": [{"delta": {"content": answer}}] }),
        serde_json::json!({ "choices": [{"delta": {}, "finish_reason": "stop"}] }),
    );

    let handle = std::thread::spawn(move || {
        let mut first = true;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };

            // Read the request head, then the body its length announces.
            let mut reader = BufReader::new(stream.try_clone().expect("a clone"));
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
            if length > 0 {
                let mut body = vec![0u8; length];
                use std::io::Read;
                let _ = reader.read_exact(&mut body);
            }

            let frames = if first {
                first = false;
                tool_frame.clone()
            } else {
                text_frame.clone()
            };

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{frames}",
                frames.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (format!("http://127.0.0.1:{port}"), handle)
}

pub fn spawn_axio(base: &str, home: &std::path::Path, args: &[&str]) -> Child {
    let mut c = Command::new(env!("CARGO_BIN_EXE_axio"));
    for var in ["ANTHROPIC_API_KEY", "AXIO_MODEL", "AXIO_MAX_STEPS"] {
        c.env_remove(var);
    }
    c.env("AXIO_HOME", home.join("home"))
        .env("AXIO_STATE", home.join("state"))
        .env("AXIO_PROVIDER", "ollama")
        .env("AXIO_BASE_URL", base)
        .env("OLLAMA_API_KEY", "stub-key-not-a-real-credential")
        .env("NO_COLOR", "1")
        .current_dir(home)
        .args(args)
        .args(["-p", "do the thing"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    c.spawn().expect("axio starts")
}

pub fn wait_for<T>(limit: Duration, mut f: impl FnMut() -> Option<T>) -> Option<T> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(v) = f() {
            return Some(v);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}
