//! The one redirect a browser sign-in sends back.
//!
//! A hand-written listener rather than a server: this accepts connections until
//! one is the callback, answers it with a page saying the tab can be closed, and
//! stops. Pulling in an HTTP server to read one query string would be a
//! dependency the whole crate does not otherwise need.
//!
//! Split from `codex` because that file reached the width limit, and because
//! none of this is specific to one provider.

use axio_core::provider::ProviderError;
use axio_core::redact::Redacted;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Accept connections until one carries the callback we are waiting for.
///
/// A browser opens more than one connection to a host it is redirected to —
/// a favicon request is the usual second — so the first connection is not
/// necessarily the callback, and answering it and stopping loses the code.
pub(super) async fn catch(
    listener: &TcpListener,
    expected_state: &str,
) -> Result<String, ProviderError> {
    loop {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|e| ProviderError::Transport(Redacted::new(e.to_string())))?;

        let mut buf = vec![0u8; 8192];
        let read = socket.read(&mut buf).await.unwrap_or(0);
        let request = String::from_utf8_lossy(&buf[..read]).to_string();
        let Some(target) = request_target(&request) else {
            let _ = socket.write_all(page(false).as_bytes()).await;
            continue;
        };
        if !target.starts_with("/auth/callback") {
            let _ = socket.write_all(page(false).as_bytes()).await;
            continue;
        }

        let query = query_pairs(target);
        let found = |name: &str| {
            query
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };

        // Checked before the code is touched. A callback whose state is not
        // ours belongs to somebody else's flow, and redeeming its code would
        // be redeeming a code we were never issued.
        if found("state").as_deref() != Some(expected_state) {
            let _ = socket.write_all(page(false).as_bytes()).await;
            return Err(ProviderError::Configuration(
                "the sign-in came back with a state this attempt did not send".to_owned(),
            ));
        }

        if let Some(code) = found("code") {
            let _ = socket.write_all(page(true).as_bytes()).await;
            let _ = socket.shutdown().await;
            return Ok(code);
        }

        let _ = socket.write_all(page(false).as_bytes()).await;
        return Err(ProviderError::Configuration(match found("error") {
            Some(why) => format!("the sign-in was refused: {why}"),
            None => "the sign-in came back without a code".to_owned(),
        }));
    }
}

/// The path and query of an HTTP request, from its first line.
pub(crate) fn request_target(request: &str) -> Option<&str> {
    let first = request.lines().next()?;
    let mut parts = first.split(' ');
    let method = parts.next()?;
    if method != "GET" {
        return None;
    }
    parts.next()
}

/// The query pairs of a request target, percent-decoded.
pub(crate) fn query_pairs(target: &str) -> Vec<(String, String)> {
    let Some((_, query)) = target.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// What the browser tab is left showing.
fn page(ok: bool) -> String {
    let (status, title, body) = if ok {
        (
            "200 OK",
            "Signed in",
            "You can close this tab and go back to axio.",
        )
    } else {
        (
            "400 Bad Request",
            "Sign-in failed",
            "Go back to axio; it will say what happened.",
        )
    };
    let html = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <body style=\"font:16px system-ui;margin:4rem auto;max-width:32rem\">\
         <h1 style=\"font-size:1.2rem\">{title}</h1><p>{body}</p>"
    );
    format!(
        "HTTP/1.1 {status}\r\n\
         content-type: text/html; charset=utf-8\r\n\
         content-length: {}\r\n\
         connection: close\r\n\r\n{html}",
        html.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_target_is_read_from_the_first_line() {
        let request = "GET /auth/callback?code=abc HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(request_target(request), Some("/auth/callback?code=abc"));
        // A browser preflight or a probe is not a callback.
        assert_eq!(request_target("POST /auth/callback HTTP/1.1\r\n"), None);
        assert_eq!(request_target(""), None);
    }

    #[test]
    fn query_pairs_are_decoded() {
        let pairs = query_pairs("/auth/callback?code=a%2Bb&state=x%20y&empty=");
        assert_eq!(pairs[0], ("code".into(), "a+b".into()));
        assert_eq!(pairs[1], ("state".into(), "x y".into()));
        assert_eq!(pairs[2], ("empty".into(), String::new()));
        assert!(query_pairs("/auth/callback").is_empty());
    }
}
