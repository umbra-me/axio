//! Sign in with ChatGPT: OpenAI's sign-in for third-party agents. The user
//! approves axio in the browser; the token it gives is used with the
//! Responses API at api.openai.com and is kept in `auth.json` under
//! `chatgpt`, refreshed before it expires.

use crate::crypto::{base64url, random, sha256};
use serde_json::{Map, Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
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

/// A client OpenAI issued to axio on an earlier sign-in. Signing in again
/// with it asks only which account to use, not to approve axio again.
struct Returning {
    client: String,
    id_token: Option<String>,
    email: Option<String>,
}

fn returning() -> Option<Returning> {
    let entry = stored();
    Some(Returning {
        client: entry["clientId"].as_str()?.to_string(),
        id_token: entry["idToken"].as_str().map(String::from),
        email: entry["email"].as_str().map(String::from),
    })
}

/// A sign-in waiting for the browser to come back.
pub struct Pending {
    pub url: String,
    verifier: String,
    state: String,
    device: String,
    /// The client from an earlier sign-in, when there was one.
    client: Option<String>,
    listener: TcpListener,
}

fn authorize_url(
    state: &str,
    challenge: &str,
    nonce: &str,
    device: &str,
    again: Option<&Returning>,
) -> String {
    let host = format!("urn:uuid:{device}");
    let redirect = redirect();
    let mut params = vec![
        ("client_id", again.map_or(CLIENT, |r| r.client.as_str())),
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
    match again {
        // The name is only offered when axio is added; a returning client keeps it.
        None => params.push(("agent_name_hint", "axio")),
        Some(r) => {
            if let Some(t) = &r.id_token {
                params.push(("id_token_hint", t));
            }
            if let Some(e) = &r.email {
                params.push(("login_hint", e));
            }
        }
    }
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
    let again = returning();
    let url = authorize_url(
        &state,
        &base64url(&sha256(verifier.as_bytes())),
        &nonce,
        &device,
        again.as_ref(),
    );
    Ok(Pending {
        url,
        verifier,
        state,
        device,
        client: again.map(|r| r.client),
        listener,
    })
}

/// The code and client from the query of the page the browser returned to.
/// A returning client may be left out of the reply, but never replaced.
fn returned(
    query: &Map<String, Value>,
    state: &str,
    saved: Option<&str>,
) -> Result<(String, String), String> {
    let query = &Value::Object(query.clone());
    if let Some(e) = query["error"].as_str() {
        let why = query["error_description"].as_str().unwrap_or(e);
        return Err(match e {
            "access_denied" => "axio was not allowed to use ChatGPT".to_string(),
            _ => format!("ChatGPT sign-in was not completed: {why}"),
        });
    }
    if query["state"].as_str() != Some(state) {
        return Err("the sign-in reply did not match this sign-in; try again".into());
    }
    let code = query["code"]
        .as_str()
        .ok_or("the sign-in reply had no code")?;
    let client = match (
        query["client_id"].as_str().filter(|c| !c.trim().is_empty()),
        saved,
    ) {
        (Some(c), Some(s)) if c != s => {
            return Err("the sign-in came back for a different client; try again".into());
        }
        (Some(c), _) | (None, Some(c)) => c,
        (None, None) => return Err("the sign-in reply had no client id".into()),
    };
    Ok((code.to_string(), client.to_string()))
}

/// The browser's reply when it belongs to this sign-in; anything else,
/// such as a stale tab from an earlier attempt, is ignored.
fn for_this_sign_in(
    query: &Map<String, Value>,
    state: &str,
    saved: Option<&str>,
) -> Option<Result<(String, String), String>> {
    (query.get("state").and_then(Value::as_str) == Some(state))
        .then(|| returned(query, state, saved))
}

/// The browser's return: the code and client, and the open request, which is
/// answered once the sign-in has finished so the page tells the outcome.
pub struct Reply {
    pub code: Result<(String, String), String>,
    browser: Option<TcpStream>,
}

impl Reply {
    /// Shows the outcome in the browser tab, when the browser is waiting.
    pub fn answer(self, outcome: &Result<Option<String>, String>) {
        if let Some(stream) = self.browser {
            respond(&stream, "200 OK", &page(outcome));
        }
    }
}

fn respond(mut stream: &TcpStream, status: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
}

impl Pending {
    /// Checks once for the browser's return. `None` while waiting.
    pub fn poll(&self) -> Result<Option<Reply>, String> {
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
            respond(&stream, "404 Not Found", "");
            return Ok(None);
        };
        match for_this_sign_in(&crate::oauth::query(q), &self.state, self.client.as_deref()) {
            None => {
                let stale = Err("This page is from an earlier sign-in. Use the newest tab, or run /login chatgpt again.".into());
                respond(&stream, "200 OK", &page(&stale));
                Ok(None)
            }
            Some(code) => Ok(Some(Reply {
                code,
                browser: Some(stream),
            })),
        }
    }

    /// The same, from the address the browser ended on, pasted in when the
    /// browser runs on another machine.
    pub fn pasted(&self, url: &str) -> Result<Reply, String> {
        let url = url.trim();
        let rest = url
            .strip_prefix(&redirect())
            .ok_or_else(|| format!("paste the whole address, starting {}", redirect()))?;
        let q = rest
            .strip_prefix('?')
            .ok_or("that address has no sign-in reply")?;
        Ok(Reply {
            code: returned(&crate::oauth::query(q), &self.state, self.client.as_deref()),
            browser: None,
        })
    }

    /// Trades the code for tokens and saves them. Returns the account's
    /// email, when OpenAI gives one, to show which account was used.
    pub fn finish(&self, code: &str, client: &str) -> Result<Option<String>, String> {
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
        let Value::Object(mut entry) = stored() else {
            unreachable!()
        };
        entry.extend(credential(&token, now())?);
        entry.insert("clientId".into(), json!(client));
        entry.insert("deviceId".into(), json!(self.device));
        let email = token["id_token"].as_str().and_then(email_in);
        if let Some(t) = token["id_token"].as_str() {
            entry.insert("idToken".into(), json!(t));
        }
        if let Some(e) = &email {
            entry.insert("email".into(), json!(e));
        }
        store(entry)?;
        Ok(email)
    }
}

/// The email in an ID token, shown as a label only: axio never decides
/// anything from it, so it does not check the token's signature.
fn email_in(id_token: &str) -> Option<String> {
    let payload = id_token.split('.').nth(1)?;
    let claims: Value = serde_json::from_slice(&base64url_decode(payload)?).ok()?;
    claims["email"].as_str().map(String::from)
}

fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.trim_end_matches('=').bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The page the browser lands on: whether axio can now use ChatGPT, and
/// where the plan's use by axio is managed.
fn page(outcome: &Result<Option<String>, String>) -> String {
    let (title, lead, body) = match outcome {
        Ok(email) => (
            "axio is signed in to ChatGPT",
            "You're signed in",
            format!(
                "<p>axio can now use your ChatGPT plan{}.</p><p class=\"next\">Go back to your terminal; this tab can be closed.</p><p class=\"small\">Limits for axio's use of your plan, and its access, are in <a href=\"https://chatgpt.com/settings/usage\">ChatGPT settings</a>.</p>",
                email.as_deref().map_or(String::new(), |e| format!(
                    " as <strong>{}</strong>",
                    escape(e)
                ))
            ),
        ),
        Err(e) => (
            "axio sign-in did not finish",
            "Sign-in didn't finish",
            format!(
                "<p>{}</p><p class=\"next\">Run <code>/login chatgpt</code> in axio to try again.</p>",
                escape(e)
            ),
        ),
    };
    let mark = if outcome.is_ok() { "✓" } else { "!" };
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}</title><style>
:root{{--bg:#fafaf9;--fg:#1c1917;--dim:#57534e;--line:#e7e5e4;--ok:#15803d;--bad:#b91c1c;color-scheme:light dark}}
@media (prefers-color-scheme:dark){{:root{{--bg:#0c0a09;--fg:#fafaf9;--dim:#a8a29e;--line:#292524;--ok:#4ade80;--bad:#f87171}}}}
*{{box-sizing:border-box}}body{{margin:0;min-height:100vh;display:grid;place-items:center;padding:24px;background:var(--bg);color:var(--fg);font:16px/1.6 ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif}}
main{{width:100%;max-width:440px}}
.brand{{font:600 15px ui-monospace,SFMono-Regular,Menlo,monospace;letter-spacing:.02em;color:var(--dim);margin-bottom:40px}}
.mark{{width:40px;height:40px;border-radius:50%;display:grid;place-items:center;font-weight:700;color:var(--bg);background:var(--{tone});margin-bottom:20px}}
h1{{font-size:26px;line-height:1.2;margin:0 0 12px;font-weight:650}}p{{margin:0 0 10px;color:var(--dim)}}strong{{color:var(--fg);font-weight:600}}
.next{{color:var(--fg)}}.small{{font-size:14px;margin-top:28px;padding-top:16px;border-top:1px solid var(--line)}}a{{color:inherit}}
code{{font:14px ui-monospace,SFMono-Regular,Menlo,monospace;background:var(--line);padding:1px 6px;border-radius:4px}}
</style></head><body><main><div class="brand">axio</div><div class="mark">{mark}</div><h1>{lead}</h1>{body}</main></body></html>"#,
        tone = if outcome.is_ok() { "ok" } else { "bad" },
    )
}

/// What is saved from a token reply. The grant must allow direct use; a
/// refresh may leave the refresh token out, which keeps the old one.
fn credential(token: &Value, at: u64) -> Result<Map<String, Value>, String> {
    let scopes = token["scope"].as_str().unwrap_or("");
    if !scopes.split_whitespace().any(|s| s == DIRECT) {
        return Err(format!(
            "this ChatGPT account did not allow axio to use its plan ({DIRECT} was not granted)"
        ));
    }
    let access = token["access_token"].as_str().ok_or("no access token")?;
    let life = token["expires_in"].as_u64().unwrap_or(3600);
    let mut m = Map::new();
    m.insert("access".into(), json!(access));
    m.insert("expires".into(), json!(at + life.saturating_sub(MARGIN)));
    if let Some(r) = token["refresh_token"].as_str().filter(|r| !r.is_empty()) {
        m.insert("refresh".into(), json!(r));
    }
    Ok(m)
}

/// Forgets the tokens but keeps the issued client, so signing in again
/// does not add axio to the account a second time.
pub fn sign_out() -> Result<bool, String> {
    let Value::Object(mut entry) = stored() else {
        unreachable!()
    };
    let had = entry.contains_key("refresh");
    for k in ["access", "refresh", "expires"] {
        entry.remove(k);
    }
    store(entry)?;
    Ok(had)
}

/// A current access token, refreshed first when it is about to expire.
pub fn bearer() -> Result<String, String> {
    let path = crate::auth::path().ok_or("the credential store is not set up")?;
    bearer_at(path, TOKEN, None)
}

/// A new access token after the API refused `rejected` before its expiry.
pub fn bearer_after_rejection(rejected: &str) -> Result<String, String> {
    let path = crate::auth::path().ok_or("the credential store is not set up")?;
    bearer_at(path, TOKEN, Some(rejected))
}

fn bearer_at(path: &Path, token_url: &str, rejected: Option<&str>) -> Result<String, String> {
    let read = || Value::Object(crate::auth::section(path, PROVIDER));
    let usable = |entry: &Value| {
        entry["access"]
            .as_str()
            .filter(|a| Some(*a) != rejected)
            .filter(|_| entry["expires"].as_u64().is_some_and(|e| e > now()))
            .map(String::from)
    };
    if let Some(access) = usable(&read()) {
        return Ok(access);
    }
    // Each refresh replaces the refresh token, so two axio sessions must not
    // refresh at once: the second waits here, then finds the first one's token.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|e| format!("could not lock the credential store: {e}"))?;
    lock.lock()
        .map_err(|e| format!("could not lock the credential store: {e}"))?;
    let entry = read();
    if let Some(access) = usable(&entry) {
        return Ok(access);
    }
    let (Some(refresh), Some(client)) = (entry["refresh"].as_str(), entry["clientId"].as_str())
    else {
        return Err("not signed in to ChatGPT; run /login chatgpt".into());
    };
    let token = crate::oauth::token_request(
        token_url,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", client),
            ("refresh_token", refresh),
            ("resource", API),
        ],
    )
    .map_err(|e| {
        format!(
            "the ChatGPT sign-in has ended (signed out, disconnected in ChatGPT settings, or unused for 30 days); run /login chatgpt ({e})"
        )
    })?;
    let Value::Object(mut saved) = entry else {
        unreachable!()
    };
    let fresh = credential(&token, now())?;
    let access = fresh["access"].as_str().unwrap_or("").to_string();
    saved.extend(fresh);
    crate::auth::set_section(path, PROVIDER, saved)?;
    Ok(access)
}

/// A plain reason for the errors a ChatGPT plan adds to the API's own.
pub fn explain(err: &str) -> Option<String> {
    if err.contains("subscription_sharing_usage_limit_exceeded") {
        return Some("axio has reached its usage limit on your ChatGPT plan; see https://chatgpt.com/settings/usage".into());
    }
    if err.contains("subscription_sharing_user_not_eligible") {
        return Some("this ChatGPT account's plan cannot be used by other apps".into());
    }
    None
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

    fn params(url: &str) -> Map<String, Value> {
        crate::oauth::query(url.split_once('?').unwrap().1)
    }

    #[test]
    fn a_first_sign_in_adds_axio_by_name() {
        let url = authorize_url("s1", "c1", "n1", "0000-dev", None);
        let query = params(&url);
        assert!(url.starts_with(AUTHORIZE));
        assert_eq!(query["client_id"], "dynamic_agent_client");
        assert_eq!(query["agent_name_hint"], "axio");
        assert_eq!(query["ext_agent_host_id"], "urn:uuid:0000-dev");
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(query["resource"], API);
        assert_eq!(query["code_challenge_method"], "S256");
        let scope = query["scope"].as_str().unwrap();
        for s in ["offline_access", "resource.invoke", DIRECT] {
            assert!(scope.contains(s), "{s}");
        }
    }

    #[test]
    fn signing_in_again_reuses_the_issued_client() {
        let again = Returning {
            client: "app_issued".into(),
            id_token: Some("old.id.token".into()),
            email: Some("me@example.com".into()),
        };
        let query = params(&authorize_url("s1", "c1", "n1", "0000-dev", Some(&again)));
        assert_eq!(query["client_id"], "app_issued");
        assert_eq!(query["id_token_hint"], "old.id.token");
        assert_eq!(query["login_hint"], "me@example.com");
        assert_eq!(query["ext_agent_host_id"], "urn:uuid:0000-dev");
        assert!(query.get("agent_name_hint").is_none(), "already added");
    }

    #[test]
    fn the_reply_must_match_and_carry_a_client() {
        let ok = q(&[("code", "k"), ("state", "s1"), ("client_id", "cl")]);
        assert_eq!(
            returned(&ok, "s1", None).unwrap(),
            ("k".into(), "cl".into())
        );
        assert!(
            returned(&ok, "other", None)
                .unwrap_err()
                .contains("did not match")
        );
        let no_client = q(&[("code", "k"), ("state", "s1")]);
        assert!(
            returned(&no_client, "s1", None)
                .unwrap_err()
                .contains("no client id")
        );
        assert_eq!(
            returned(&no_client, "s1", Some("saved")).unwrap().1,
            "saved",
            "a returning client may be left out"
        );
        assert!(
            returned(&ok, "s1", Some("saved"))
                .unwrap_err()
                .contains("different client")
        );
        let refused = q(&[("error", "access_denied"), ("state", "s1")]);
        assert_eq!(
            returned(&refused, "s1", None).unwrap_err(),
            "axio was not allowed to use ChatGPT"
        );
    }

    #[test]
    fn replies_for_another_sign_in_are_ignored() {
        let stale = q(&[("code", "k"), ("state", "old"), ("client_id", "cl")]);
        assert!(for_this_sign_in(&stale, "s1", None).is_none());
        let ours = q(&[("error", "access_denied"), ("state", "s1")]);
        assert!(for_this_sign_in(&ours, "s1", None).unwrap().is_err());
    }

    #[test]
    fn a_grant_without_direct_use_is_refused() {
        let token = json!({"access_token": "a", "refresh_token": "r", "expires_in": 3600,
                           "scope": "openid profile"});
        assert!(credential(&token, 100).unwrap_err().contains(DIRECT));
        let token = json!({"access_token": "a", "expires_in": 3600,
                           "scope": format!("openid {DIRECT}")});
        let c = credential(&token, 100).unwrap();
        assert_eq!(c["expires"], 100 + 3600 - MARGIN);
        assert!(
            c.get("refresh").is_none(),
            "a refresh without one keeps the old"
        );
    }

    #[test]
    fn the_email_comes_from_the_id_token() {
        let claims = base64url(br#"{"email":"me@example.com","sub":"u1"}"#);
        assert_eq!(
            email_in(&format!("eyJhbGciOiJSUzI1NiJ9.{claims}.sig")).as_deref(),
            Some("me@example.com")
        );
        assert_eq!(email_in("not-a-token"), None);
    }

    #[test]
    fn the_browser_page_tells_the_outcome() {
        let ok = page(&Ok(Some("a<b@example.com".into())));
        assert!(ok.contains("You're signed in"));
        assert!(ok.contains("a&lt;b@example.com"), "escaped");
        assert!(ok.contains("https://chatgpt.com/settings/usage"));
        let failed = page(&Err("axio was not allowed to use ChatGPT".into()));
        assert!(failed.contains("Sign-in didn't finish"));
        assert!(failed.contains("/login chatgpt"));
    }

    #[test]
    fn plan_errors_are_explained_and_not_retried() {
        let err = "429 from https://api.openai.com/v1/responses: subscription_sharing_usage_limit_exceeded: limit";
        let plain = explain(err).unwrap();
        assert!(plain.contains("chatgpt.com/settings/usage"));
        assert_eq!(crate::provider::retry_reason(&plain), None);
        assert_eq!(explain("429 from x: rate_limit_exceeded: slow"), None);
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

    /// A token endpoint that, like OpenAI's, replaces the refresh token on
    /// every use and refuses one that was already used.
    fn rotating_token_server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::Read;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let refreshes = std::sync::Arc::new(AtomicUsize::new(0));
        let count = refreshes.clone();
        std::thread::spawn(move || {
            let mut current = "r0".to_string();
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(&stream);
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
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let form = crate::oauth::query(&String::from_utf8(body).unwrap());
                let (status, reply) = if form["refresh_token"] == current.as_str() {
                    let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                    current = format!("r{n}");
                    // Slow enough for a second caller to arrive meanwhile.
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    (
                        "200 OK",
                        json!({"access_token": format!("a{n}"), "refresh_token": current,
                                      "expires_in": 3600, "scope": DIRECT})
                        .to_string(),
                    )
                } else {
                    (
                        "400 Bad Request",
                        json!({"error": "invalid_grant"}).to_string(),
                    )
                };
                let _ = write!(
                    &stream,
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
        });
        (url, refreshes)
    }

    fn signed_in_home(name: &str, expires: u64) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("axio-chatgpt-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        let Value::Object(entry) = json!({"access": "a0", "refresh": "r0", "expires": expires,
                                         "clientId": "cl", "email": "me@example.com"})
        else {
            unreachable!()
        };
        crate::auth::set_section(&path, PROVIDER, entry).unwrap();
        path
    }

    #[test]
    fn sessions_refreshing_at_once_share_one_refresh() {
        let (url, refreshes) = rotating_token_server();
        let path = signed_in_home("race", 0);
        let callers: Vec<_> = (0..3)
            .map(|_| {
                let (path, url) = (path.clone(), url.clone());
                std::thread::spawn(move || bearer_at(&path, &url, None))
            })
            .collect();
        let tokens: Vec<String> = callers
            .into_iter()
            .map(|c| c.join().unwrap().unwrap())
            .collect();
        assert_eq!(tokens, vec!["a1", "a1", "a1"]);
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let saved = crate::auth::section(&path, PROVIDER);
        assert_eq!(saved["refresh"], "r1", "the replaced refresh token is kept");
        assert_eq!(saved["email"], "me@example.com", "other fields survive");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_rejected_token_is_refreshed_once_even_before_it_expires() {
        let (url, refreshes) = rotating_token_server();
        let path = signed_in_home("rejected", now() + 3000);
        assert_eq!(bearer_at(&path, &url, None).unwrap(), "a0", "still valid");
        assert_eq!(bearer_at(&path, &url, Some("a0")).unwrap(), "a1");
        // Another session already replaced the token it was refused: use that.
        assert_eq!(bearer_at(&path, &url, Some("a0")).unwrap(), "a1");
        assert_eq!(refreshes.load(std::sync::atomic::Ordering::SeqCst), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
