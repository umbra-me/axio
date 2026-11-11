//! The `web` module's WebFetch tool: an HTTP GET that turns HTML into
//! readable text. The model reads the result itself; no second model call.

use serde_json::{Value, json};
use std::io::Read;
use std::time::Duration;

const MAX_BYTES: u64 = 5 * 1024 * 1024;

pub fn spec() -> crate::tools::ToolSpec {
    crate::tools::ToolSpec {
        name: "WebFetch".into(),
        description: "Fetch a web page or text file over http(s) and return it as text. HTML is reduced to readable text with links kept.".into(),
        schema: json!({
            "type": "object",
            "properties": {"url": {"type": "string", "description": "An http or https URL"}},
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
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let resp = agent
        .get(url)
        .header("user-agent", concat!("axio/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("could not fetch {url}: {e}"))?;
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
