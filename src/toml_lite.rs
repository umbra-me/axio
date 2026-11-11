//! Reads the TOML subset Codex and Grok Build configs use, into JSON values:
//! `[table.path]` and `[[array.of.tables]]` headers with bare or quoted keys,
//! `key = value` with strings, numbers, booleans, arrays and inline tables,
//! and comments. Dates and multi-line strings are not needed and not read.

use serde_json::{Map, Value};

pub fn parse(text: &str) -> Result<Value, String> {
    let mut root = Map::new();
    let mut path: Vec<String> = Vec::new();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((n, raw)) = lines.next() {
        let mut line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        let err = |m: &str| format!("line {}: {m}", n + 1);
        if let Some(inner) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
            path = keys(inner).map_err(|e| err(&e))?;
            let (last, parents) = path.split_last().ok_or_else(|| err("empty table name"))?;
            let table = table_at(&mut root, parents).map_err(|e| err(&e))?;
            let list = table
                .entry(last.clone())
                .or_insert_with(|| Value::Array(Vec::new()));
            let Value::Array(items) = list else {
                return Err(err("not an array of tables"));
            };
            items.push(Value::Object(Map::new()));
            continue;
        }
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            path = keys(inner).map_err(|e| err(&e))?;
            table_at(&mut root, &path).map_err(|e| err(&e))?;
            continue;
        }
        // Arrays and inline tables may continue over several lines.
        while open_brackets(&line) > 0 {
            let Some((_, next)) = lines.next() else {
                return Err(err("unclosed array"));
            };
            line.push(' ');
            line.push_str(strip_comment(next).trim());
        }
        let (key, value) = split_key_value(&line).ok_or_else(|| err("expected key = value"))?;
        let mut key_path = keys(key).map_err(|e| err(&e))?;
        let last = key_path.pop().ok_or_else(|| err("empty key"))?;
        let mut full = path.clone();
        full.extend(key_path);
        let table = table_at(&mut root, &full).map_err(|e| err(&e))?;
        let (v, rest) = value_of(value.trim()).map_err(|e| err(&e))?;
        if !rest.trim().is_empty() {
            return Err(err(&format!("unexpected text after value: {rest}")));
        }
        table.insert(last, v);
    }
    Ok(Value::Object(root))
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '#') => return &line[..i],
            _ => {}
        }
        escaped = false;
    }
    line
}

fn open_brackets(line: &str) -> i32 {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        match (quote, c) {
            (Some('"'), '\\') if !escaped => {
                escaped = true;
                continue;
            }
            (Some(q), c) if c == q && !escaped => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '{') => depth += 1,
            (None, ']' | '}') => depth -= 1,
            _ => {}
        }
        escaped = false;
    }
    depth
}

fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"' | '\'') => quote = Some(c),
            (None, '=') => return Some((&line[..i], &line[i + 1..])),
            _ => {}
        }
    }
    None
}

/// `a."b c".d` into its parts.
fn keys(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let (key, after) = if rest.starts_with(['"', '\'']) {
            let (v, after) = value_of(rest)?;
            (v.as_str().unwrap_or_default().to_string(), after)
        } else {
            let end = rest.find('.').unwrap_or(rest.len());
            (rest[..end].trim().to_string(), &rest[end..])
        };
        if key.is_empty() {
            return Err(format!("bad key {s:?}"));
        }
        out.push(key);
        rest = after.trim_start();
        rest = rest.strip_prefix('.').unwrap_or(rest).trim_start();
    }
    Ok(out)
}

fn table_at<'a>(
    root: &'a mut Map<String, Value>,
    path: &[String],
) -> Result<&'a mut Map<String, Value>, String> {
    let mut t = root;
    for k in path {
        let entry = t
            .entry(k.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        t = match entry {
            Value::Object(m) => m,
            // [[x]] then [x.y] addresses the latest table in the array.
            Value::Array(items) => match items.last_mut() {
                Some(Value::Object(m)) => m,
                _ => return Err(format!("{k} is not a table")),
            },
            _ => return Err(format!("{k} is already a value")),
        };
    }
    Ok(t)
}

/// One value from the start of `s`, and the text after it.
fn value_of(s: &str) -> Result<(Value, &str), String> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Ok((Value::String(out), &rest[i + 1..])),
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('u') => {
                        let hex: String = chars.by_ref().take(4).map(|(_, c)| c).collect();
                        let code = u32::from_str_radix(&hex, 16).map_err(|_| "bad \\u escape")?;
                        out.push(char::from_u32(code).ok_or("bad \\u escape")?);
                    }
                    other => return Err(format!("unknown escape \\{}", other.unwrap_or(' '))),
                },
                c => out.push(c),
            }
        }
        return Err("unclosed string".into());
    }
    if let Some(rest) = s.strip_prefix('\'') {
        let end = rest.find('\'').ok_or("unclosed string")?;
        return Ok((Value::String(rest[..end].into()), &rest[end + 1..]));
    }
    if let Some(mut rest) = s.strip_prefix('[') {
        let mut items = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                return Ok((Value::Array(items), after));
            }
            let (v, after) = value_of(rest)?;
            items.push(v);
            rest = after.trim_start();
            rest = rest.strip_prefix(',').unwrap_or(rest);
        }
    }
    if let Some(mut rest) = s.strip_prefix('{') {
        let mut map = Map::new();
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix('}') {
                return Ok((Value::Object(map), after));
            }
            let eq = split_key_value(rest)
                .ok_or("expected key = value in inline table")?
                .0
                .len();
            let mut key = keys(&rest[..eq])?;
            let last = key.pop().ok_or("empty key")?;
            let (v, after) = value_of(&rest[eq + 1..])?;
            table_at(&mut map, &key)?.insert(last, v);
            rest = after.trim_start();
            rest = rest.strip_prefix(',').unwrap_or(rest);
        }
    }
    let end = s.find([',', ']', '}']).unwrap_or(s.len());
    let word = s[..end].trim();
    let v = match word {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        w => {
            let clean = w.replace('_', "");
            if let Ok(i) = clean.parse::<i64>() {
                Value::from(i)
            } else if let Ok(f) = clean.parse::<f64>() {
                Value::from(f)
            } else {
                return Err(format!("unsupported value {w:?}"));
            }
        }
    };
    Ok((v, &s[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_codex_and_grok_shapes() {
        let text = r#"
model = "gpt-6" # trailing comment
[mcp_servers.node_repl]
args = []
command = "/usr/local/bin/node"
startup_timeout_sec = 120.0

[mcp_servers.node_repl.env]
NODE_PATH = "/opt/a:/opt/b"   # with a # inside? no

[mcp_servers.ghidra]
enabled = false
args = [
  "--port", "8080",  # comment inside an array
  'raw\path',
]

[plugins."ida-mcp@HexRaysSA"]
enabled = true

[[marketplace.sources]]
url = "https://example.invalid/a"
[[marketplace.sources]]
url = "https://example.invalid/b"
pinned = { ref = "v1", depth = 1 }
"#;
        let v = parse(text).unwrap();
        assert_eq!(v["model"], "gpt-6");
        assert_eq!(
            v["mcp_servers"]["node_repl"]["command"],
            "/usr/local/bin/node"
        );
        assert_eq!(
            v["mcp_servers"]["node_repl"]["env"]["NODE_PATH"],
            "/opt/a:/opt/b"
        );
        assert_eq!(v["mcp_servers"]["node_repl"]["startup_timeout_sec"], 120.0);
        assert_eq!(
            v["mcp_servers"]["ghidra"]["args"],
            json!(["--port", "8080", "raw\\path"])
        );
        assert_eq!(v["plugins"]["ida-mcp@HexRaysSA"]["enabled"], true);
        assert_eq!(
            v["marketplace"]["sources"][1]["pinned"],
            json!({"ref": "v1", "depth": 1})
        );
    }

    #[test]
    fn reports_what_it_cannot_read() {
        assert!(parse("a = 1979-05-27").unwrap_err().starts_with("line 1"));
        assert!(parse("a = \"open").is_err());
        assert!(parse("[a]\nb = [1,").is_err());
        assert_eq!(
            parse("s = \"tab\\tq\\\"\\u00e9\"").unwrap()["s"],
            "tab\tq\"é"
        );
    }
}

#[cfg(test)]
mod real_files {
    /// `AXIO_TOML_CHECK=a.toml:b.toml cargo test real_configs -- --ignored`
    #[test]
    #[ignore]
    fn real_configs() {
        for p in std::env::var("AXIO_TOML_CHECK").unwrap().split(':') {
            let text = std::fs::read_to_string(p).unwrap();
            let v = super::parse(&text).unwrap_or_else(|e| panic!("{p}: {e}"));
            let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
            println!("{p}: ok, top-level keys {keys:?}");
        }
    }
}
