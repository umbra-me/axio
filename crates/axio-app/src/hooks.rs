//! Where a hosted agent's own hooks report to.
//!
//! Claude Code and Codex can run a command on the events that matter —
//! a prompt submitted, a turn ended, a permission wanted — and this is the
//! command's other end: one loopback HTTP listener, one bearer token, one
//! shell script that POSTs what it was handed. The hooks are passed to each
//! hosted process on its command line, so a session this window did not
//! start never runs them and nothing is written into the person's own
//! configuration. A status learned this way is the agent's own word; the
//! quiet-timer guess is only for tools that have no hooks.
//!
//! Plain `TcpListener` and a hand-rolled request parse, because the whole
//! protocol is "POST a JSON body with a bearer, get 204", and a web
//! framework for that would be the second largest thing in this crate.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// What a hosted process is told so its hooks can find this window.
#[derive(Debug, Clone)]
pub struct HookEndpoint {
    pub url: String,
    pub token: String,
    pub script: PathBuf,
}

impl HookEndpoint {
    /// The environment a hosted process carries, for the script to read.
    pub fn env(&self, terminal: &str) -> Vec<(String, String)> {
        vec![
            ("AXIO_HOOK_URL".to_owned(), self.url.clone()),
            ("AXIO_HOOK_TOKEN".to_owned(), self.token.clone()),
            ("AXIO_HOSTED_ID".to_owned(), terminal.to_owned()),
            ("AXIO_HOSTED".to_owned(), "1".to_owned()),
        ]
    }
}

/// A report from a hook: which terminal, which event, what the tool said.
pub type Sink = Arc<dyn Fn(String, String, serde_json::Value) + Send + Sync>;

/// Bind, write the script, and serve until the process ends.
pub async fn serve(home: &Path, sink: Sink) -> std::io::Result<HookEndpoint> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let token = format!("{:032x}", ulid::Ulid::generate().random());
    let script = write_script(home)?;
    let endpoint = HookEndpoint {
        url: format!("http://127.0.0.1:{port}/hook"),
        token: token.clone(),
        script,
    };
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let sink = Arc::clone(&sink);
            let token = token.clone();
            tokio::spawn(async move {
                let _ = handle(stream, &token, sink).await;
            });
        }
    });
    Ok(endpoint)
}

async fn handle(mut stream: tokio::net::TcpStream, token: &str, sink: Sink) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let head_end = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut chunk))
            .await??;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 64 * 1024 {
            return reply(&mut stream, "431 Request Header Fields Too Large").await;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.lines();
    let request = lines.next().unwrap_or_default();
    if !request.starts_with("POST ") {
        return reply(&mut stream, "405 Method Not Allowed").await;
    }
    let header = |name: &str| {
        lines
            .clone()
            .find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            })
            .map(|(_, v)| v.trim().to_owned())
    };
    if header("authorization").as_deref() != Some(&format!("Bearer {token}")) {
        return reply(&mut stream, "401 Unauthorized").await;
    }
    let length: usize = header("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return reply(&mut stream, "413 Content Too Large").await;
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut chunk))
            .await??;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let terminal = header("x-axio-terminal").unwrap_or_default();
    let event = header("x-axio-event").unwrap_or_default();
    let payload = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    if !terminal.is_empty() && !event.is_empty() {
        sink(terminal, event, payload);
    }
    reply(&mut stream, "204 No Content").await
}

async fn reply(stream: &mut tokio::net::TcpStream, status: &str) -> std::io::Result<()> {
    stream
        .write_all(
            format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    stream.shutdown().await
}

/// The script the hooks run. Rewritten on every start so it always matches
/// this build; executable on unix; a no-op anywhere the environment it
/// needs is missing, so a copy of it invoked by hand does nothing.
fn write_script(home: &Path) -> std::io::Result<PathBuf> {
    let dir = home.join("hooks");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("report.sh");
    let text = r#"#!/bin/sh
# Written by axio. Reports a hosted agent's hook event to the window hosting it.
# Called as: report.sh <event> [payload]. Claude Code hands the payload on
# stdin; Codex passes it as an argument. Without the window's environment it
# reads and discards its input so the tool never waits on it.
if [ -z "$AXIO_HOOK_URL" ] || [ -z "$AXIO_HOOK_TOKEN" ]; then
  cat >/dev/null 2>&1
  exit 0
fi
event="$1"
if [ -n "$2" ]; then body="$2"; else body=$(cat); fi
printf '%s' "$body" | curl -s -m 2 -X POST "$AXIO_HOOK_URL" \
  -H "Authorization: Bearer $AXIO_HOOK_TOKEN" \
  -H "X-Axio-Terminal: $AXIO_HOSTED_ID" \
  -H "X-Axio-Event: $event" \
  -H 'Content-Type: application/json' \
  --data-binary @- >/dev/null 2>&1 || true
exit 0
"#;
    std::fs::write(&path, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_report_with_the_token_reaches_the_sink_and_one_without_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink: Sink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |t, e, p| seen.lock().unwrap().push((t, e, p)))
        };
        let endpoint = serve(dir.path(), sink).await.unwrap();
        assert!(endpoint.script.is_file());

        let post = |auth: String| {
            let url = endpoint.url.clone();
            async move {
                let addr = url.trim_start_matches("http://").trim_end_matches("/hook");
                let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
                let body = r#"{"session_id":"abc"}"#;
                let req = format!(
                    "POST /hook HTTP/1.1\r\nHost: x\r\n{auth}X-Axio-Terminal: h1\r\nX-Axio-Event: Stop\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                s.write_all(req.as_bytes()).await.unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                out
            }
        };
        let ok = post(format!("Authorization: Bearer {}\r\n", endpoint.token)).await;
        assert!(ok.starts_with("HTTP/1.1 204"), "{ok}");
        let bad = post("Authorization: Bearer nope\r\n".to_owned()).await;
        assert!(bad.starts_with("HTTP/1.1 401"), "{bad}");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "h1");
        assert_eq!(seen[0].1, "Stop");
        assert_eq!(seen[0].2["session_id"], "abc");
    }
}
