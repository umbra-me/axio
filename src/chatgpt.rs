//! Sign in with ChatGPT: OpenAI's sign-in for third-party agents. The user
//! approves axio in the browser; the token it gives is used with the
//! Responses API at api.openai.com and is kept in `auth.json` under
//! `chatgpt`, refreshed before it expires.

use crate::crypto::{base64url, random, sha256};
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{SystemTime, UNIX_EPOCH};

pub const PROVIDER: &str = "chatgpt";
pub const API: &str = "https://api.openai.com/v1";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const CLIENT: &str = "dynamic_agent_client";
const PORT: u16 = 1455;
const CALLBACK: &str = "/auth/callback";
const DIRECT: &str = "chatgpt.tokens.use.direct";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// Refresh this long before the token expires.
const MARGIN: u64 = 180;

fn redirect() -> String {
    format!("http://127.0.0.1:{PORT}{CALLBACK}")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn stored() -> Value {
    Value::Object(crate::auth::path().map_or_else(Map::new, |p| crate::auth::section(p, PROVIDER)))
}

fn store(entry: Map<String, Value>) -> Result<(), String> {
    let path = crate::auth::path().ok_or("the credential store is not set up")?;
    crate::auth::set_section(path, PROVIDER, entry)
}

pub fn signed_in() -> bool {
    stored()["refresh"].is_string()
}

/// This installation's id, which OpenAI shows next to the approved agent.
fn device_id() -> Result<String, String> {
    if let Some(id) = stored()["deviceId"].as_str() {
        return Ok(id.to_string());
    }
    let mut b = random(16)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// A sign-in waiting for the browser to come back.
pub struct Pending {
    pub url: String,
    verifier: String,
    state: String,
    device: String,
    listener: TcpListener,
}

fn authorize_url(state: &str, challenge: &str, nonce: &str, device: &str) -> String {
    let host = format!("urn:uuid:{device}");
    let redirect = redirect();
    let params = [
        ("client_id", CLIENT),
        ("agent_name_hint", "axio"),
        ("ext_agent_host_id", host.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect.as_str()),
        ("resource", API),
        ("scope", SCOPE),
        ("state", state),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("nonce", nonce),
    ];
    format!("{AUTHORIZE}?{}", crate::oauth::form(&params))
}

/// Starts a sign-in: listens for the browser's return and builds the page
/// to open.
pub fn start() -> Result<Pending, String> {
    let listener = TcpListener::bind(("127.0.0.1", PORT)).map_err(|e| {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            format!(
                "port {PORT} is in use, probably by an unfinished sign-in in Codex or another tool; finish or cancel it and try again"
            )
        } else {
            format!("could not listen on port {PORT}: {e}")
        }
    })?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let verifier = base64url(&random(32)?);
    let state = base64url(&random(32)?);
    let nonce = base64url(&random(32)?);
    let device = device_id()?;
    // Kept from the first attempt on, so this installation keeps one id.
    if let Value::Object(mut entry) = stored()
        && !entry.contains_key("deviceId")
    {
        entry.insert("deviceId".into(), json!(device));
        store(entry)?;
    }
    let url = authorize_url(
        &state,
        &base64url(&sha256(verifier.as_bytes())),
        &nonce,
        &device,
    );
    Ok(Pending {
        url,
        verifier,
        state,
        device,
        listener,
    })
}

/// The code and issued client id from the query of the page the browser
/// returned to.
fn returned(query: &Map<String, Value>, state: &str) -> Result<(String, String), String> {
    let query = &Value::Object(query.clone());
    if let Some(e) = query["error"].as_str() {
        let why = query["error_description"].as_str().unwrap_or(e);
        return Err(format!("ChatGPT sign-in was not completed: {why}"));
    }
    if query["state"].as_str() != Some(state) {
        return Err("the sign-in reply did not match this sign-in; try again".into());
    }
    let code = query["code"]
        .as_str()
        .ok_or("the sign-in reply had no code")?;
    let client = query["client_id"]
        .as_str()
        .filter(|c| !c.trim().is_empty())
        .ok_or("the sign-in reply had no client id")?;
    Ok((code.to_string(), client.to_string()))
}

/// The browser's reply when it belongs to this sign-in; anything else,
/// such as a stale tab from an earlier attempt, is ignored.
fn for_this_sign_in(
    query: &Map<String, Value>,
    state: &str,
) -> Option<Result<(String, String), String>> {
    (query.get("state").and_then(Value::as_str) == Some(state)).then(|| returned(query, state))
}

impl Pending {
    /// Checks once for the browser's return. `Ok(None)` while waiting.
    pub fn poll(&self) -> Result<Option<(String, String)>, String> {
        let stream = match self.listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        stream.set_nonblocking(false).map_err(|e| e.to_string())?;
        let mut line = String::new();
        BufReader::new(&stream)
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        let target = line.split_whitespace().nth(1).unwrap_or("");
        let Some(q) = target
            .strip_prefix(CALLBACK)
            .and_then(|r| r.strip_prefix('?'))
        else {
            let _ = write!(
                &stream,
                "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
            );
            return Ok(None);
        };
        let result = for_this_sign_in(&crate::oauth::query(q), &self.state);
        let page = match &result {
            None => "This page is not from the sign-in axio is waiting for.",
            Some(Ok(_)) => "Signed in to ChatGPT. You can close this tab and return to axio.",
            Some(Err(e)) => e.as_str(),
        };
        let page = format!("<html><body><p>{page}</p></body></html>");
        let _ = write!(
            &stream,
            "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
            page.len()
        );
        result.transpose()
    }

    /// The same, from the address the browser ended on, pasted in when the
    /// browser runs on another machine.
    pub fn pasted(&self, url: &str) -> Result<(String, String), String> {
        let url = url.trim();
        let rest = url
            .strip_prefix(&redirect())
            .or_else(|| url.strip_prefix(&format!("http://localhost:{PORT}{CALLBACK}")))
            .ok_or_else(|| format!("paste the whole address, starting {}", redirect()))?;
        let q = rest
            .strip_prefix('?')
            .ok_or("that address has no sign-in reply")?;
        returned(&crate::oauth::query(q), &self.state)
    }

    /// Trades the code for tokens and saves them.
    pub fn finish(self, code: &str, client: &str) -> Result<(), String> {
        let redirect = redirect();
        let token = crate::oauth::token_request(
            TOKEN,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", client),
                ("code", code),
                ("code_verifier", &self.verifier),
                ("redirect_uri", &redirect),
                ("resource", API),
            ],
        )?;
        store(credential(&token, client, &self.device, now())?)
    }
}

/// What is saved from a token reply. The grant must allow direct use.
fn credential(
    token: &Value,
    client: &str,
    device: &str,
    at: u64,
) -> Result<Map<String, Value>, String> {
    let scopes = token["scope"].as_str().unwrap_or("");
    if !scopes.split_whitespace().any(|s| s == DIRECT) {
        return Err(format!("the ChatGPT sign-in did not grant {DIRECT}"));
    }
    let access = token["access_token"].as_str().ok_or("no access token")?;
    let refresh = token["refresh_token"].as_str().ok_or("no refresh token")?;
    let life = token["expires_in"].as_u64().unwrap_or(3600);
    let Value::Object(m) = json!({
        "access": access,
        "refresh": refresh,
        "expires": at + life.saturating_sub(MARGIN),
        "clientId": client,
        "deviceId": device,
    }) else {
        unreachable!()
    };
    Ok(m)
}

/// A current access token, refreshed first when it is about to expire.
pub fn bearer() -> Result<String, String> {
    let entry = stored();
    let expired = "the ChatGPT sign-in has expired; run /login chatgpt";
    let (Some(refresh), Some(client)) = (entry["refresh"].as_str(), entry["clientId"].as_str())
    else {
        return Err("not signed in to ChatGPT; run /login chatgpt".into());
    };
    if let Some(access) = entry["access"].as_str()
        && entry["expires"].as_u64().is_some_and(|e| e > now())
    {
        return Ok(access.to_string());
    }
    let token = crate::oauth::token_request(
        TOKEN,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", refresh),
            ("resource", API),
        ],
    )
    .map_err(|e| format!("{expired} ({e})"))?;
    let device = entry["deviceId"].as_str().unwrap_or("");
    let mut fresh = credential(&token, client, device, now())?;
    // A reply without a new refresh token keeps the old one.
    if fresh["refresh"].as_str().is_none_or(str::is_empty) {
        fresh.insert("refresh".into(), json!(refresh));
    }
    let access = fresh["access"].as_str().unwrap_or("").to_string();
    store(fresh)?;
    Ok(access)
}

/// The models the account can use, from its model list.
pub fn models() -> Result<Vec<String>, String> {
    let list = crate::provider::get_json(
        &format!("{API}/models"),
        &[("authorization", format!("Bearer {}", bearer()?))],
    )?;
    model_slugs(&list)
}

fn model_slugs(list: &Value) -> Result<Vec<String>, String> {
    let models = list["models"]
        .as_array()
        .ok_or("the model list had no models")?;
    Ok(models
        .iter()
        .filter(|m| m["visibility"].as_str().is_none_or(|v| v == "list"))
        .filter_map(|m| {
            let slug = m["slug"].as_str()?;
            if let Some(n) = m["context_window"].as_u64() {
                crate::provider::learn_window(&format!("{PROVIDER}/{slug}"), n);
            }
            Some(slug.to_string())
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(pairs: &[(&str, &str)]) -> Map<String, Value> {
        crate::oauth::query(&crate::oauth::form(pairs))
    }

    #[test]
    fn the_authorize_page_names_axio_and_asks_for_direct_use() {
        let url = authorize_url("s1", "c1", "n1", "0000-dev");
        let query = crate::oauth::query(url.split_once('?').unwrap().1);
        assert!(url.starts_with(AUTHORIZE));
        assert_eq!(query["client_id"], "dynamic_agent_client");
        assert_eq!(query["agent_name_hint"], "axio");
        assert_eq!(query["ext_agent_host_id"], "urn:uuid:0000-dev");
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(query["resource"], API);
        assert_eq!(query["code_challenge_method"], "S256");
        assert!(query["scope"].as_str().unwrap().contains(DIRECT));
    }

    #[test]
    fn the_reply_must_match_and_carry_a_client() {
        let ok = q(&[("code", "k"), ("state", "s1"), ("client_id", "cl")]);
        assert_eq!(returned(&ok, "s1").unwrap(), ("k".into(), "cl".into()));
        assert!(
            returned(&ok, "other")
                .unwrap_err()
                .contains("did not match")
        );
        let no_client = q(&[("code", "k"), ("state", "s1")]);
        assert!(
            returned(&no_client, "s1")
                .unwrap_err()
                .contains("no client id")
        );
        let refused = q(&[("error", "access_denied"), ("state", "s1")]);
        assert!(
            returned(&refused, "s1")
                .unwrap_err()
                .contains("access_denied")
        );
    }

    #[test]
    fn replies_for_another_sign_in_are_ignored() {
        let stale = q(&[("code", "k"), ("state", "old"), ("client_id", "cl")]);
        assert!(for_this_sign_in(&stale, "s1").is_none());
        let ours = q(&[("error", "access_denied"), ("state", "s1")]);
        assert!(for_this_sign_in(&ours, "s1").unwrap().is_err());
    }

    #[test]
    fn a_grant_without_direct_use_is_refused() {
        let token = json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600,
                           "scope": "openid profile"});
        assert!(
            credential(&token, "cl", "d", 100)
                .unwrap_err()
                .contains(DIRECT)
        );
        let token = json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600,
                           "scope": format!("openid {DIRECT}")});
        let c = credential(&token, "cl", "d", 100).unwrap();
        assert_eq!(c["expires"], 100 + 3600 - MARGIN);
        assert_eq!(c["clientId"], "cl");
    }

    #[test]
    fn model_slugs_skip_hidden_models() {
        let list = json!({"models": [
            {"slug": "gpt-a", "visibility": "list", "context_window": 500000},
            {"slug": "gpt-b"},
            {"slug": "gpt-c", "visibility": "hide"},
        ]});
        assert_eq!(model_slugs(&list).unwrap(), vec!["gpt-a", "gpt-b"]);
        let model = crate::provider::Model::parse("chatgpt/gpt-a");
        assert_eq!(model.context_window(), 500_000, "learned from the list");
        let other = crate::provider::Model::parse("chatgpt/gpt-b");
        assert_eq!(other.context_window(), 272_000);
    }

    #[test]
    fn device_ids_are_version_4_uuids() {
        let id = device_id().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }
}
