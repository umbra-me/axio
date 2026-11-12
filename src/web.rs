//! The `web` module's WebFetch tool: an HTTP GET that turns HTML into
//! readable text. With a `prompt`, the agent has a small model answer it from
//! the page (see `Agent::digest`).

use serde_json::{Value, json};
use std::io::Read;
use std::time::Duration;

const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn spec() -> crate::tools::ToolSpec {
    crate::tools::ToolSpec {
        name: "WebFetch".into(),
        description: "Fetch a web page or text file over http(s). HTML is reduced to readable text with links kept. With a prompt, a small model answers it from the page and only the answer comes back, which keeps long pages out of the conversation; without one you get the whole text.".into(),
        schema: json!({
            "type": "object",
            "properties": {
                "url": {"type": "string", "description": "An http or https URL"},
                "prompt": {"type": "string", "description": "What to find on the page; leave out for the whole text"}
            },
            "required": ["url"],
            "additionalProperties": false
        }),
    }
}

pub fn fetch(input: &Value) -> Result<String, String> {
    let url = input["url"]
        .as_str()
        .ok_or("missing string argument: url")?;
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http and https URLs can be fetched".into());
    }
    let agent = crate::provider::http(Some(Duration::from_secs(30)));
    let mut url = url.to_string();
    let mut hops = 0;
    let resp = loop {
        let resp = agent
            .get(&url)
            .header("user-agent", concat!("axio/", env!("CARGO_PKG_VERSION")))
            .call()
            .map_err(|e| format!("could not fetch {url}: {e}"))?;
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .filter(|_| resp.status().is_redirection());
        let Some(location) = location else {
            break resp;
        };
        let next = join(&url, location);
        // Permission covered the address asked for; another site needs a
        // call of its own, which is checked again.
        if !same_site(&url, &next) {
            return Ok(format!(
                "URL: {url}\n\n{url} redirects to {next}, another site. Fetch that URL to follow it."
            ));
        }
        hops += 1;
        if hops > 5 {
            return Err(format!("{url}: too many redirects"));
        }
        url = next;
    };
    let url = url.as_str();
    let status = resp.status().as_u16();
    let kind = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mut bytes = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("reading {url}: {e}"))?;
    let raw = String::from_utf8_lossy(&bytes);
    let text = if kind.contains("html") || (kind.is_empty() && raw.trim_start().starts_with('<')) {
        html_to_text(&raw)
    } else if kind.starts_with("text/")
        || kind.contains("json")
        || kind.contains("xml")
        || kind.is_empty()
    {
        raw.into_owned()
    } else {
        return Err(format!("{url} returned {kind}, which is not text"));
    };
    if !(200..300).contains(&status) {
        let start: String = text.chars().take(500).collect();
        return Err(format!("{url} returned {status}: {start}"));
    }
    Ok(crate::tools::shorten(&format!(
        "URL: {url}\n\n{}",
        text.trim()
    )))
}

/// Scheme, host and port of an http(s) URL.
fn origin(url: &str) -> Option<(&str, &str)> {
    let (scheme, rest) = url.split_once("://")?;
    Some((scheme, rest.split(['/', '?', '#']).next()?))
}

/// The same scheme, host and port, or the same host moving to https.
fn same_site(from: &str, to: &str) -> bool {
    match (origin(from), origin(to)) {
        (Some(a), Some(b)) => {
            let host = |h: &str| {
                let h = h
                    .rsplit_once('@')
                    .map_or(h, |(_, h)| h)
                    .to_ascii_lowercase();
                h.strip_suffix(":80").map(str::to_string).unwrap_or(h)
            };
            (a.0 == b.0 && host(a.1) == host(b.1))
                || (a.0 == "http"
                    && b.0 == "https"
                    && host(a.1) == host(b.1).trim_end_matches(":443"))
        }
        _ => false,
    }
}

/// A redirect's `location` made absolute against the page it came from.
fn join(base: &str, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let Some((scheme, rest)) = base.split_once("://") else {
        return location.to_string();
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    if let Some(l) = location.strip_prefix("//") {
        return format!("{scheme}://{l}");
    }
    if location.starts_with('/') {
        return format!("{scheme}://{host}{location}");
    }
    let path = rest[host.len()..].split(['?', '#']).next().unwrap_or("");
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    format!("{scheme}://{host}{dir}/{location}")
}

fn entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" => ' ',
        n if n.starts_with("#x") || n.starts_with("#X") => {
            char::from_u32(u32::from_str_radix(&n[2..], 16).ok()?)?
        }
        n if n.starts_with('#') => char::from_u32(n[1..].parse().ok()?)?,
        _ => return None,
    })
}

fn decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        match rest[1..]
            .find(';')
            .filter(|&n| n <= 10)
            .and_then(|n| entity(&rest[1..=n]).map(|c| (c, n)))
        {
            Some((c, n)) => {
                out.push(c);
                rest = &rest[n + 2..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Readable text from HTML: scripts, styles and comments dropped, block
/// tags as line breaks, list items as dashes, links as `text (href)`.
pub fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::new();
    let mut i = 0;
    let mut href: Option<String> = None;
    while i < html.len() {
        if lower[i..].starts_with("<!--") {
            i = lower[i..].find("-->").map_or(html.len(), |e| i + e + 3);
            continue;
        }
        if html.as_bytes()[i] != b'<' {
            let end = html[i..].find('<').map_or(html.len(), |e| i + e);
            out.push_str(&decode(&html[i..end]));
            i = end;
            continue;
        }
        let end = html[i..].find('>').map_or(html.len(), |e| i + e + 1);
        let tag = &lower[i..end];
        let name: String = tag
            .trim_start_matches(['<', '/'])
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let closing = tag.starts_with("</");
        if !closing
            && matches!(
                name.as_str(),
                "script" | "style" | "noscript" | "svg" | "head"
            )
        {
            let close = format!("</{name}");
            i = lower[end..].find(&close).map_or(html.len(), |e| {
                let after = end + e;
                lower[after..]
                    .find('>')
                    .map_or(html.len(), |g| after + g + 1)
            });
            continue;
        }
        match (name.as_str(), closing) {
            (
                "br" | "p" | "div" | "tr" | "section" | "article" | "header" | "footer" | "pre"
                | "table" | "ul" | "ol",
                _,
            ) => out.push('\n'),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => out.push_str("\n\n# "),
            ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => out.push('\n'),
            ("li", false) => out.push_str("\n- "),
            ("td" | "th", false) => out.push(' '),
            ("a", false) => {
                href = html[i..end]
                    .split("href=")
                    .nth(1)
                    .map(|v| {
                        v.trim_start_matches(['"', '\''])
                            .split(['"', '\'', ' ', '>'])
                            .next()
                            .unwrap_or("")
                            .to_string()
                    })
                    .filter(|h| {
                        !h.is_empty() && !h.starts_with('#') && !h.starts_with("javascript:")
                    });
            }
            ("a", true) => {
                if let Some(h) = href.take() {
                    out.push_str(&format!(" ({})", decode(&h)));
                }
            }
            _ => {}
        }
        i = end;
    }
    if let Some(start) = lower.find("<title")
        && let Some(open) = lower[start..].find('>')
        && let Some(len) = lower[start + open..].find("</title")
    {
        let title = decode(html[start + open + 1..start + open + len].trim());
        if !title.is_empty() {
            out = format!("Title: {title}\n\n{out}");
        }
    }
    let mut lines: Vec<String> = Vec::new();
    for line in out.lines() {
        let squeezed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if squeezed.is_empty() && lines.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        lines.push(squeezed);
    }
    lines.join("\n").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets_and_sites() {
        assert_eq!(join("https://a.dev/x/y?q", "/z"), "https://a.dev/z");
        assert_eq!(join("https://a.dev/x/y", "z"), "https://a.dev/x/z");
        assert_eq!(join("https://a.dev/x", "//b.dev/p"), "https://b.dev/p");
        assert_eq!(join("https://a.dev/x", "http://c.dev/"), "http://c.dev/");
        assert!(same_site("https://a.dev/x", "https://A.dev/y"));
        assert!(same_site("http://a.dev/x", "https://a.dev/x"));
        assert!(
            !same_site("https://a.dev/x", "http://a.dev/x"),
            "no downgrade"
        );
        assert!(!same_site("https://a.dev/", "https://b.a.dev/"));
        assert!(!same_site("http://127.0.0.1:1/", "http://127.0.0.1:2/"));
        assert!(!same_site("https://a.dev/", "https://evil@a.dev.evil/"));
    }

    #[test]
    fn redirects_are_followed_only_on_the_same_site() {
        use crate::testserve::{ok, redirect, serve};
        let other = serve(|_| ok("internal service"));
        let away = format!("{}/admin", other.base);
        let site = serve(move |path| match path {
            "/moved" => redirect("/page"),
            "/page" => ok("the page"),
            _ => redirect(&away),
        });
        let text = fetch(&json!({"url": format!("{}/moved", site.base)})).unwrap();
        assert!(text.contains("the page"), "{text}");
        let text = fetch(&json!({"url": format!("{}/leave", site.base)})).unwrap();
        assert!(
            text.contains(&format!("redirects to {}/admin", other.base)),
            "{text}"
        );
        assert!(!text.contains("internal service"), "{text}");
        assert!(
            other.seen.lock().unwrap().is_empty(),
            "the other site was not contacted"
        );
    }

    #[test]
    fn turns_html_into_text() {
        let html = r#"<html><head><title>T</title><style>p{}</style></head><body>
<h1>Title &amp; more</h1><script>alert(1)</script><!-- hidden -->
<p>Read <a href="https://example.com/doc?a=1&amp;b=2">the docs</a> now.</p>
<ul><li>one</li><li>two&nbsp;&#x2713;</li></ul></body></html>"#;
        assert_eq!(
            html_to_text(html),
            "Title: T\n\n# Title & more\n\nRead the docs (https://example.com/doc?a=1&b=2) now.\n\n- one\n- two ✓"
        );
        assert_eq!(decode("a &bogus; b &lt;c&gt; &#65;"), "a &bogus; b <c> A");
    }

    #[test]
    fn refuses_other_schemes() {
        assert!(fetch(&json!({"url": "file:///etc/passwd"})).is_err());
    }
}
