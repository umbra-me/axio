//! `axio agent` as a real process, against a local server that records what
//! it is sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};

/// Answers one chat completions request with "hi" and returns its headers.
fn serve_once(listener: TcpListener) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = String::new();
        let mut length = 0;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
            headers.push_str(&line);
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let sse = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let mut stream = stream;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{sse}"
        )
        .unwrap();
        headers
    })
}

#[test]
fn a_child_uses_keys_saved_in_auth_json() {
    let home = std::env::temp_dir().join(format!("axio-child-auth-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::fs::write(
        home.join("auth.json"),
        format!(
            r#"{{"filed": {{"baseUrl": "http://127.0.0.1:{port}/v1", "apiKey": "key-from-file"}}}}"#
        ),
    )
    .unwrap();
    let server = serve_once(listener);
    let mut child = Command::new(env!("CARGO_BIN_EXE_axio"))
        .arg("agent")
        .env("AXIO_HOME", &home)
        .env_remove("FILED_API_KEY")
        .env_remove("FILED_BASE_URL")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let spec = format!(
        r#"{{"task": "say hi", "tools": [], "model": "filed/m", "cwd": {:?}}}"#,
        home.display().to_string()
    );
    child
        .stdin
        .take()
        .unwrap()
        .write_all(spec.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: serde_json::Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "result", "{stdout}");
    assert_eq!(last["text"], "hi");
    assert!(out.status.success());
    let headers = server.join().unwrap().to_lowercase();
    assert!(headers.contains("bearer key-from-file"), "{headers}");
}
