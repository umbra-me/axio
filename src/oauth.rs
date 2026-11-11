//! OAuth sign-in for remote MCP servers, as the MCP authorization spec
//! describes: discover the authorization server from the 401, register as a
//! client, sign in through the browser with PKCE, and keep the token fresh.
//! Tokens live in `~/.axio/agent/mcp-auth.json`, readable only by the user.

use crate::crypto::{base64url, random, sha256};
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PROTOCOL: &str = "2025-06-18";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

static STORE: OnceLock<PathBuf> = OnceLock::new();

/// Where tokens are kept; set once at startup.
pub fn set_home(home: &Path) {
    let _ = STORE.set(home.join("mcp-auth.json"));
}

fn store_path() -> Option<&'static PathBuf> {
    STORE.get()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into()
}

fn load(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(path: &Path, all: &Map<String, Value>) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
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

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let hex = (b[i] == b'%' && i + 2 < b.len())
            .then(|| std::str::from_utf8(&b[i + 1..i + 3]).ok())
            .flatten()
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (b[i], hex) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 2;
            }
            (b'+', _) => out.push(b' '),
            (c, _) => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", encode(k), encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn query(s: &str) -> Map<String, Value> {
    s.split('&')
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (decode(k), Value::String(decode(v))))
        .collect()
}

/// `scheme://host[:port]` and the path of a URL.
fn split_url(url: &str) -> (String, String) {
    let after = url.find("://").map_or(0, |i| i + 3);
    let path_at = url[after..].find('/').map_or(url.len(), |i| after + i);
    let path = url[path_at..].split(['?', '#']).next().unwrap_or("");
    (
        url[..path_at].to_string(),
        path.trim_end_matches('/').to_string(),
    )
}

/// A parameter from a `WWW-Authenticate: Bearer k="v", …` header.
fn header_param(header: &str, key: &str) -> Option<String> {
    let at = header.find(&format!("{key}="))?;
    let rest = &header[at + key.len() + 1..];
    Some(match rest.strip_prefix('"') {
        Some(q) => q.split('"').next().unwrap_or("").to_string(),
        None => rest.split([',', ' ']).next().unwrap_or("").to_string(),
    })
}

fn get_json(url: &str) -> Option<Value> {
    let resp = agent()
        .get(url)
        .header("accept", "application/json")
        .call()
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    serde_json::from_str(&resp.into_body().read_to_string().ok()?).ok()
}

/// Posts `initialize` without credentials; a 401 carries the hints sign-in needs.
fn probe(url: &str) -> Result<String, String> {
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "axio", "version": env!("CARGO_PKG_VERSION")}}});
    let resp = agent()
        .post(url)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .send(body.to_string())
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    match resp.status().as_u16() {
        401 => Ok(resp
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()),
        200..=299 => Err("this server does not ask for a sign-in".into()),
        code => Err(format!("{url} returned {code}")),
    }
}

#[derive(Debug)]
struct AuthServer {
    authorize: String,
    token: String,
    register: Option<String>,
    scopes: Option<String>,
}

fn discover(url: &str, challenge: &str) -> Result<AuthServer, String> {
    let (origin, path) = split_url(url);
    // RFC 9728: the protected resource names its authorization servers.
    let resource = header_param(challenge, "resource_metadata")
        .and_then(|u| get_json(&u))
        .or_else(|| {
            get_json(&format!(
                "{origin}/.well-known/oauth-protected-resource{path}"
            ))
        })
        .or_else(|| get_json(&format!("{origin}/.well-known/oauth-protected-resource")));
    let issuer = resource
        .as_ref()
        .and_then(|r| r["authorization_servers"][0].as_str().map(String::from))
        .unwrap_or_else(|| origin.clone());
    let (iss_origin, iss_path) = split_url(&issuer);
    let meta = [
        format!("{iss_origin}/.well-known/oauth-authorization-server{iss_path}"),
        format!("{iss_origin}/.well-known/openid-configuration{iss_path}"),
        format!("{issuer}/.well-known/openid-configuration"),
    ]
    .iter()
    .find_map(|u| get_json(u));
    let field = |k: &str| meta.as_ref().and_then(|m| m[k].as_str().map(String::from));
    let scopes = header_param(challenge, "scope").or_else(|| {
        resource.as_ref().and_then(|r| {
            let list: Vec<&str> = r["scopes_supported"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            (!list.is_empty()).then(|| list.join(" "))
        })
    });
    // Servers from before RFC 9728 put the endpoints at fixed paths.
    Ok(AuthServer {
        authorize: field("authorization_endpoint").unwrap_or(format!("{iss_origin}/authorize")),
        token: field("token_endpoint").unwrap_or(format!("{iss_origin}/token")),
        register: field("registration_endpoint")
            .or_else(|| meta.is_none().then(|| format!("{iss_origin}/register"))),
        scopes,
    })
}

fn token_request(endpoint: &str, pairs: &[(&str, &str)]) -> Result<Value, String> {
    let resp = agent()
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("accept", "application/json")
        .send(form(pairs))
        .map_err(|e| format!("could not reach {endpoint}: {e}"))?;
    let status = resp.status().as_u16();
    let body = resp.into_body().read_to_string().unwrap_or_default();
    let v: Value =
        serde_json::from_str(&body).map_err(|_| format!("{endpoint} returned {status}: {body}"))?;
    if !(200..300).contains(&status) || v.get("access_token").is_none() {
        let why = v["error_description"]
            .as_str()
            .or(v["error"].as_str())
            .unwrap_or(&body)
            .to_string();
        return Err(format!("the sign-in was refused: {why}"));
    }
    Ok(v)
}

fn credential(
    token: &Value,
    client: &Value,
    endpoint: &str,
    resource: &str,
    old_refresh: Option<&str>,
) -> Value {
    json!({
        "access_token": token["access_token"],
        "refresh_token": token["refresh_token"].as_str().or(old_refresh),
        "expires_at": token["expires_in"].as_u64().map(|s| now() + s),
        "token_endpoint": endpoint,
        "client_id": client["client_id"],
        "client_secret": client["client_secret"],
        "resource": resource,
    })
}

/// Waits for the browser's redirect to `/callback` and returns its query.
fn wait_for_callback(listener: &TcpListener) -> Result<Map<String, Value>, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + CALLBACK_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).map_err(|e| e.to_string())?;
                let mut line = String::new();
                BufReader::new(&stream)
                    .read_line(&mut line)
                    .map_err(|e| e.to_string())?;
                let target = line.split_whitespace().nth(1).unwrap_or("");
                let Some(q) = target.strip_prefix("/callback?") else {
                    let _ = write!(
                        &stream,
                        "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                    );
                    continue;
                };
                let page = "<html><body><p>Signed in. You can close this tab and return to axio.</p></body></html>";
                let _ = write!(
                    &stream,
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
                    page.len()
                );
                return Ok(query(q));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() > deadline {
                    return Err("timed out waiting for the browser sign-in".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Signs in to the server at `url` through the browser, which `open` shows
/// the sign-in page in, and stores the token.
pub fn login(url: &str, open: &dyn Fn(&str)) -> Result<(), String> {
    let store = store_path().ok_or("the token store is not set up")?;
    let challenge = probe(url)?;
    let server = discover(url, &challenge)?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let redirect = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr().map_err(|e| e.to_string())?.port()
    );
    let register = server
        .register
        .as_deref()
        .ok_or("the server's authorization server does not offer client registration, so axio cannot sign in to it")?;
    let resp = agent()
        .post(register)
        .header("content-type", "application/json")
        .send(
            json!({"client_name": "axio", "redirect_uris": [redirect], "grant_types": ["authorization_code", "refresh_token"],
                   "response_types": ["code"], "token_endpoint_auth_method": "none"})
            .to_string(),
        )
        .map_err(|e| format!("could not register with {register}: {e}"))?;
    let client: Value =
        serde_json::from_str(&resp.into_body().read_to_string().unwrap_or_default())
            .map_err(|_| "the client registration reply was not JSON")?;
    let client_id = client["client_id"]
        .as_str()
        .ok_or("the client registration gave no client_id")?;
    let verifier = base64url(&random(32)?);
    let state = base64url(&random(16)?);
    let code_challenge = base64url(&sha256(verifier.as_bytes()));
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect.as_str()),
        ("code_challenge", code_challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", state.as_str()),
        ("resource", url),
    ];
    if let Some(s) = &server.scopes {
        params.push(("scope", s));
    }
    let sep = if server.authorize.contains('?') {
        '&'
    } else {
        '?'
    };
    open(&format!("{}{sep}{}", server.authorize, form(&params)));
    let back = wait_for_callback(&listener)?;
    if let Some(e) = back.get("error").and_then(Value::as_str) {
        return Err(format!("the sign-in was refused: {e}"));
    }
    if back.get("state").and_then(Value::as_str) != Some(state.as_str()) {
        return Err("the sign-in reply did not match this request; try again".into());
    }
    let code = back
        .get("code")
        .and_then(Value::as_str)
        .ok_or("the sign-in reply had no code")?;
    let mut pairs = vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect.as_str()),
        ("client_id", client_id),
        ("code_verifier", verifier.as_str()),
        ("resource", url),
    ];
    if let Some(secret) = client["client_secret"].as_str() {
        pairs.push(("client_secret", secret));
    }
    let token = token_request(&server.token, &pairs)?;
    let mut all = load(store);
    all.insert(
        url.to_string(),
        credential(&token, &client, &server.token, url, None),
    );
    save(store, &all)
}

pub fn logout(url: &str) -> Result<bool, String> {
    let store = store_path().ok_or("the token store is not set up")?;
    let mut all = load(store);
    let had = all.remove(url).is_some();
    save(store, &all)?;
    Ok(had)
}

pub fn signed_in(url: &str) -> bool {
    store_path().is_some_and(|s| load(s).contains_key(url))
}

/// The access token for `url`, refreshed first when it has expired or when
/// `force` is set because the server just refused it.
pub fn bearer(url: &str, force: bool) -> Option<String> {
    let store = store_path()?;
    let mut all = load(store);
    let cred = all.get(url)?.clone();
    let expired = cred["expires_at"].as_u64().is_some_and(|t| t <= now() + 30);
    if !(expired || force) {
        return cred["access_token"].as_str().map(String::from);
    }
    let refresh = cred["refresh_token"].as_str()?;
    let endpoint = cred["token_endpoint"].as_str()?;
    let client_id = cred["client_id"].as_str()?;
    let mut pairs = vec![
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh),
        ("client_id", client_id),
        ("resource", url),
    ];
    if let Some(secret) = cred["client_secret"].as_str() {
        pairs.push(("client_secret", secret));
    }
    let token = token_request(endpoint, &pairs).ok()?;
    let client = json!({"client_id": cred["client_id"], "client_secret": cred["client_secret"]});
    let fresh = credential(&token, &client, endpoint, url, Some(refresh));
    let access = fresh["access_token"].as_str().map(String::from);
    all.insert(url.to_string(), fresh);
    let _ = save(store, &all);
    access
}

/// Opens `url` in the user's browser and prints it in case that fails.
pub fn open_browser(url: &str) {
    eprintln!("Opening your browser to sign in. If it does not open, visit:\n{url}");
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(
            split_url("https://h.dev:8/a/mcp/?x=1"),
            ("https://h.dev:8".into(), "/a/mcp".into())
        );
        assert_eq!(
            split_url("https://h.dev"),
            ("https://h.dev".into(), "".into())
        );
        let h = r#"Bearer error="invalid_token", resource_metadata="https://h/.well-known/oauth-protected-resource", scope=read"#;
        assert_eq!(
            header_param(h, "resource_metadata").as_deref(),
            Some("https://h/.well-known/oauth-protected-resource")
        );
        assert_eq!(header_param(h, "scope").as_deref(), Some("read"));
        assert_eq!(form(&[("a b", "c&d=é")]), "a%20b=c%26d%3D%C3%A9");
        assert_eq!(
            query("code=a%2Fb+c&state=s").get("code"),
            Some(&json!("a/b c"))
        );
        assert_eq!(decode("100%"), "100%");
    }

    /// An MCP server behind a fake authorization server, all on one port.
    /// Only a token obtained by refreshing is accepted, so the test proves
    /// sign-in, PKCE and refresh together.
    fn server() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let challenge = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let b = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (base, challenge) = (b.clone(), challenge.clone());
                std::thread::spawn(move || {
                    use std::io::Read;
                    let mut r = BufReader::new(stream.try_clone().unwrap());
                    let mut first = String::new();
                    r.read_line(&mut first).unwrap();
                    let mut headers = std::collections::HashMap::new();
                    loop {
                        let mut l = String::new();
                        r.read_line(&mut l).unwrap();
                        let Some((k, v)) = l.trim_end().split_once(": ") else {
                            break;
                        };
                        headers.insert(k.to_ascii_lowercase(), v.to_string());
                    }
                    let mut body = vec![
                        0;
                        headers
                            .get("content-length")
                            .map_or(0, |l| l.parse().unwrap())
                    ];
                    r.read_exact(&mut body).unwrap();
                    let body = String::from_utf8(body).unwrap();
                    let target = first.split_whitespace().nth(1).unwrap().to_string();
                    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
                    let mut out = stream;
                    let mut send = |status: &str, extra: &str, body: &str| {
                        write!(out, "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n{extra}content-length: {}\r\nconnection: close\r\n\r\n{body}", body.len()).unwrap();
                    };
                    match path {
                        "/mcp" => {
                            if headers.get("authorization").map(String::as_str) != Some("Bearer at2") {
                                let h = format!("www-authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\"\r\n");
                                return send("401 Unauthorized", &h, "{}");
                            }
                            let m: Value = serde_json::from_str(&body).unwrap();
                            let result = match m["method"].as_str() {
                                Some("initialize") => json!({"protocolVersion": PROTOCOL, "capabilities": {}}),
                                Some("tools/list") => json!({"tools": [{"name": "whoami", "inputSchema": {"type": "object"}}]}),
                                Some("tools/call") => json!({"content": [{"type": "text", "text": "signed in"}]}),
                                _ => return send("202 Accepted", "", ""),
                            };
                            send("200 OK", "", &json!({"jsonrpc": "2.0", "id": m["id"], "result": result}).to_string());
                        }
                        "/.well-known/oauth-protected-resource/mcp" => send("200 OK", "", &json!({
                            "resource": format!("{base}/mcp"), "authorization_servers": [format!("{base}/auth")]}).to_string()),
                        "/.well-known/oauth-authorization-server/auth" => send("200 OK", "", &json!({
                            "authorization_endpoint": format!("{base}/auth/authorize"),
                            "token_endpoint": format!("{base}/auth/token"),
                            "registration_endpoint": format!("{base}/auth/register")}).to_string()),
                        "/auth/register" => {
                            let reg: Value = serde_json::from_str(&body).unwrap();
                            assert!(reg["redirect_uris"][0].as_str().unwrap().starts_with("http://127.0.0.1:"));
                            send("201 Created", "", r#"{"client_id": "c1"}"#);
                        }
                        "/auth/authorize" => {
                            let p = query(q);
                            assert_eq!(p["client_id"], "c1");
                            assert_eq!(p["code_challenge_method"], "S256");
                            assert_eq!(p["resource"], json!(format!("{base}/mcp")));
                            *challenge.lock().unwrap() = p["code_challenge"].as_str().unwrap().to_string();
                            let loc = format!("location: {}?code=code1&state={}\r\n", p["redirect_uri"].as_str().unwrap(), encode(p["state"].as_str().unwrap()));
                            send("302 Found", &loc, "");
                        }
                        "/auth/token" => {
                            let p = query(&body);
                            let reply = match p["grant_type"].as_str() {
                                Some("authorization_code") => {
                                    let verifier = p["code_verifier"].as_str().unwrap();
                                    assert_eq!(base64url(&sha256(verifier.as_bytes())), *challenge.lock().unwrap(), "PKCE");
                                    assert_eq!(p["code"], "code1");
                                    json!({"access_token": "at1", "refresh_token": "rt1", "expires_in": 0})
                                }
                                Some("refresh_token") if p["refresh_token"] == "rt1" => json!({"access_token": "at2", "expires_in": 3600}),
                                _ => return send("400 Bad Request", "", r#"{"error": "invalid_grant"}"#),
                            };
                            send("200 OK", "", &reply.to_string());
                        }
                        _ => send("404 Not Found", "", "{}"),
                    }
                });
            }
        });
        base
    }

    #[test]
    fn signs_in_with_pkce_and_refreshes() {
        let home = std::env::temp_dir().join(format!("axio-oauth-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        set_home(&home);
        let url = format!("{}/mcp", server());
        let mut config = crate::mcp::Config::default();
        config.add(
            &json!({"mcpServers": {"remote": {"type": "http", "url": url}}}),
            None,
            Path::new("mcp.json"),
        );

        let before = crate::mcp::Mcp::start(&config, &home);
        assert!(
            before.failed[0].contains("axio mcp login remote"),
            "{:?}",
            before.failed
        );

        // The "browser" follows the sign-in page's redirect to the callback.
        login(&url, &|page: &str| {
            let page = page.to_string();
            std::thread::spawn(move || {
                let _ = ureq::get(&page).call();
            });
        })
        .unwrap();
        assert!(signed_in(&url));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.join("mcp-auth.json"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let after = crate::mcp::Mcp::start(&config, &home);
        assert!(after.failed.is_empty(), "{:?}", after.failed);
        assert_eq!(
            after.call("mcp__remote__whoami", &json!({}), &mut || false),
            Ok("signed in".into())
        );
        let stored = load(&home.join("mcp-auth.json"));
        assert_eq!(
            stored[&url]["access_token"], "at2",
            "the expired token was refreshed"
        );
        assert_eq!(
            stored[&url]["refresh_token"], "rt1",
            "the refresh token is kept"
        );

        assert!(logout(&url).unwrap());
        assert!(!signed_in(&url));
        std::fs::remove_dir_all(home).unwrap();
    }
}

#[cfg(test)]
mod real_server {
    /// `AXIO_OAUTH_CHECK=<mcp url> cargo test discover_real -- --ignored`:
    /// discovery only, with no registration and no browser.
    #[test]
    #[ignore]
    fn discover_real() {
        let url = std::env::var("AXIO_OAUTH_CHECK").unwrap();
        let challenge = super::probe(&url).unwrap();
        let s = super::discover(&url, &challenge).unwrap();
        let host = |u: &str| super::split_url(u).0;
        println!(
            "401 hint: {} | authorize at {} | token at {} | registration: {} | scopes: {:?}",
            !challenge.is_empty(),
            host(&s.authorize),
            host(&s.token),
            s.register.is_some(),
            s.scopes
        );
    }
}
