use crate::message::{Block, Message};
use crate::tools::ToolSpec;
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub mod anthropic;
pub mod openai;
pub mod responses;

pub const DEFAULT_MODEL: &str = "anthropic/claude-opus-5-5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Anthropic,
    OpenAi,
    /// The Responses API, for a ChatGPT sign-in.
    Responses,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Model {
    /// Name used for credentials and display: anthropic, openai, openrouter, or
    /// any other OpenAI-compatible endpoint configured through the environment.
    pub provider: String,
    pub id: String,
    pub api: Api,
}

impl Model {
    /// Accepts `provider/model` or a bare model id. Bare `claude-*` ids go to
    /// Anthropic; other bare ids go to the OpenAI-compatible endpoint.
    pub fn parse(spec: &str) -> Model {
        let (provider, id) = match spec.split_once('/') {
            Some((p, id)) if !p.is_empty() && !id.is_empty() => (p.to_string(), id.to_string()),
            _ if spec.starts_with("claude") => ("anthropic".into(), spec.to_string()),
            _ => ("openai".into(), spec.to_string()),
        };
        let api = match provider.as_str() {
            "anthropic" => Api::Anthropic,
            crate::chatgpt::PROVIDER => Api::Responses,
            _ => Api::OpenAi,
        };
        Model { provider, id, api }
    }
}

impl Model {
    /// The model's context window in tokens; AXIO_CONTEXT_WINDOW overrides it.
    pub fn context_window(&self) -> u64 {
        if let Some(n) = env("AXIO_CONTEXT_WINDOW").and_then(|v| v.parse().ok()) {
            return n;
        }
        if let Some(n) = info(&self.to_string()).and_then(|i| i.window) {
            return n;
        }
        if self.provider == crate::chatgpt::PROVIDER {
            // The smallest window the ChatGPT model list reports.
            return 272_000;
        }
        let id = self.id.as_str();
        // Claude 4.6 and later have a 1M window; earlier Claude models 200k.
        let million = ["opus-4-6", "opus-4-7", "opus-4-8", "sonnet-4-6"];
        match id {
            _ if !id.starts_with("claude-") => 128_000,
            _ if id.starts_with("claude-3") => 200_000,
            _ if million.iter().any(|m| id.contains(m)) => 1_000_000,
            _ if id.contains("-4") => 200_000,
            _ => 1_000_000,
        }
    }
}

impl Model {
    /// The effort levels this model takes, lowest first; empty when it takes none.
    pub fn efforts(&self) -> Vec<String> {
        if let Some(i) = info(&self.to_string()).filter(|i| !i.efforts.is_empty()) {
            return i.efforts;
        }
        let levels: &[&str] = match self.api {
            Api::Anthropic => anthropic::effort_levels(&self.id),
            Api::Responses => &["low", "medium", "high", "xhigh"],
            Api::OpenAi => &["low", "medium", "high"],
        };
        levels.iter().map(|l| l.to_string()).collect()
    }

    /// What each level does, when the model list says.
    pub fn effort_notes(&self) -> Vec<String> {
        info(&self.to_string()).map(|i| i.notes).unwrap_or_default()
    }

    /// Fast mode's price per million tokens (input / output) and whether
    /// it is a research preview; `None` when the model has no fast mode.
    pub fn fast_mode(&self) -> Option<anthropic::Fast> {
        match self.api {
            Api::Anthropic => anthropic::fast_mode(&self.id),
            _ => None,
        }
    }

    /// The level the provider uses when none is sent.
    pub fn default_effort(&self) -> Option<String> {
        if let Some(d) = info(&self.to_string()).and_then(|i| i.default_effort) {
            return Some(d);
        }
        match self.api {
            _ if self.efforts().is_empty() => None,
            Api::Anthropic => Some("high".into()),
            Api::Responses => Some("medium".into()),
            Api::OpenAi => None,
        }
    }
}

impl std::fmt::Display for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.provider, self.id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    ToolUse,
    MaxTokens,
    Refusal,
    Other(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Usage {
    /// Tokens the next request starts from: everything sent plus this reply.
    pub fn context(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }
}

#[derive(Debug)]
pub struct Reply {
    pub content: Vec<Block>,
    pub stop: Stop,
    pub usage: Usage,
}

impl Reply {
    pub fn into_message(self) -> (Message, Stop, Usage) {
        (
            Message::Assistant {
                content: self.content,
            },
            self.stop,
            self.usage,
        )
    }
}

/// Streaming progress for the interface; the finished reply is returned separately.
pub enum Delta<'a> {
    Text(&'a str),
    Thinking,
    /// Readable summary text of the model's thinking, as it streams.
    ThinkingText(&'a str),
    /// A tool call started; its name.
    ToolCall(&'a str),
    /// A piece of the tool call's JSON input as it streams.
    ToolInput(&'a str),
}

pub struct Request<'a> {
    pub model: &'a Model,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub effort: Option<&'a str>,
    /// Fast mode, sent only to models that have it.
    pub fast: bool,
    /// Set when the user stops the turn; streams check it between events.
    pub cancel: &'a AtomicBool,
}

impl Request<'_> {
    pub(crate) fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(crate::agent::ABORTED.into())
        } else {
            Ok(())
        }
    }
}

pub fn complete(req: &Request, on: &mut dyn FnMut(Delta)) -> Result<Reply, String> {
    match req.model.api {
        Api::Anthropic => anthropic::complete(req, on),
        Api::OpenAi => openai::complete(req, on),
        Api::Responses => responses::complete(req, on),
    }
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .into()
}

pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Sends a JSON body and returns a reader over the event stream, or the
/// provider's error body when the status is not 2xx.
pub(crate) fn post_stream(
    url: &str,
    headers: &[(&str, String)],
    body: &serde_json::Value,
) -> Result<BufReader<impl Read + use<>>, String> {
    let mut req = agent().post(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req
        .send_json(body)
        .map_err(|e| format!("request to {url} failed: {e}"))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(|s| format!(" (retry after {s}s)"))
        .unwrap_or_default();
    let mut body = resp.into_body();
    if !(200..300).contains(&status) {
        let text = body.read_to_string().unwrap_or_default();
        return Err(format!(
            "{status} from {url}{retry_after}: {}",
            error_body(&text)
        ));
    }
    Ok(BufReader::new(body.into_reader()))
}

/// An API base URL and the headers its requests need.
pub(crate) type Connection = (String, Vec<(&'static str, String)>);

/// Fetches a JSON document, with the provider's error when the status is not 2xx.
pub(crate) fn get_json(url: &str, headers: &[(&str, String)]) -> Result<serde_json::Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(15)))
        .build()
        .into();
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.header(*k, v);
    }
    let resp = req
        .call()
        .map_err(|e| format!("could not reach {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.into_body().read_to_string().unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(format!("{status} from {url}: {}", error_body(&text)));
    }
    serde_json::from_str(&text).map_err(|_| format!("{url} did not return JSON"))
}

/// The ids in a `{"data": [{"id": …}]}` model list, sorted.
pub(crate) fn model_ids(list: &serde_json::Value) -> Result<Vec<String>, String> {
    let data = list["data"]
        .as_array()
        .ok_or("the model list had no data")?;
    let mut ids: Vec<String> = data
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    ids.sort();
    Ok(ids)
}

/// What a provider's model list says about a model.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub window: Option<u64>,
    pub efforts: Vec<String>,
    /// What each effort level does, in the same order.
    pub notes: Vec<String>,
    pub default_effort: Option<String>,
    pub about: Option<String>,
}

static INFO: std::sync::Mutex<Vec<(String, Info)>> = std::sync::Mutex::new(Vec::new());

/// Records what a model list said about `model` (`provider/id`).
pub fn learn(model: &str, info: Info) {
    if let Ok(mut all) = INFO.lock() {
        all.retain(|(m, _)| m != model);
        all.push((model.to_string(), info));
    }
}

pub fn info(model: &str) -> Option<Info> {
    let all = INFO.lock().ok()?;
    all.iter().find(|(m, _)| m == model).map(|(_, i)| i.clone())
}

/// Providers with a built-in endpoint.
pub const KNOWN: [&str; 4] = ["anthropic", "chatgpt", "openai", "openrouter"];

/// Providers set up on this machine: known ones with a key, then any other
/// endpoint saved with a base URL.
pub fn signed_in() -> Vec<String> {
    let mut out: Vec<String> = KNOWN
        .iter()
        .filter(|p| !crate::auth::missing_key(p))
        .map(|p| p.to_string())
        .collect();
    if let Some(path) = crate::auth::path() {
        for (name, entry) in crate::auth::read(path) {
            if !matches!(name.as_str(), "mcp" | "chatgpt")
                && !out.contains(&name)
                && entry["baseUrl"].is_string()
            {
                out.push(name);
            }
        }
    }
    out
}

/// The models `provider` offers, as `provider/id`.
pub fn models(provider: &str) -> Result<Vec<String>, String> {
    let ids = match provider {
        "anthropic" => anthropic::models()?,
        crate::chatgpt::PROVIDER => crate::chatgpt::models()?,
        p => openai::models(p)?,
    };
    Ok(ids
        .into_iter()
        .map(|id| format!("{provider}/{id}"))
        .collect())
}

/// The API's `type: message` from a JSON error body, else the body itself.
fn error_body(text: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or_default();
    let e = &v["error"];
    // OpenAI's `code` names the error more exactly than its `type`.
    let kind = e["code"].as_str().or(e["type"].as_str());
    match (kind, e["message"].as_str()) {
        (Some(t), Some(m)) => format!("{t}: {m}"),
        (None, Some(m)) => m.to_string(),
        _ => text.trim().to_string(),
    }
}

/// Why a failed call is worth trying again, or `None` when it is not:
/// overload, rate limits, server errors and dropped connections.
pub fn retry_reason(err: &str) -> Option<&'static str> {
    let status = err
        .split_once(" from ")
        .and_then(|(s, _)| s.parse::<u16>().ok());
    Some(match status {
        Some(529) => "the API is overloaded",
        Some(429) => "rate limited",
        Some(408 | 409 | 500 | 502 | 503 | 504) => "the API had an error",
        Some(_) => return None,
        None if err.starts_with("overloaded_error") => "the API is overloaded",
        None if err.starts_with("rate_limit_error") => "rate limited",
        None if err.starts_with("api_error") => "the API had an error",
        None if err.starts_with("request to ") => "the connection failed",
        None if err == "stream ended before the model finished" => "the connection dropped",
        None => return None,
    })
}

/// How long to wait before retry `attempt` (from 0): the server's
/// `retry-after` when it gave one, else doubling from `unit`, at most 30 units.
pub fn retry_wait(err: &str, attempt: u32, unit: Duration) -> Duration {
    let after = err
        .split_once("(retry after ")
        .and_then(|(_, r)| r.split_once("s)"))
        .and_then(|(n, _)| n.parse::<u64>().ok());
    match after {
        Some(s) => Duration::from_secs(s.min(300)),
        None => unit * 2u32.pow(attempt).min(30),
    }
}

#[cfg(test)]
pub(crate) fn with_request<T>(cancelled: bool, f: impl FnOnce(&Request) -> T) -> T {
    let model = Model::parse("claude-test");
    let cancel = AtomicBool::new(cancelled);
    f(&Request {
        model: &model,
        system: "",
        messages: &[],
        tools: &[],
        effort: None,
        fast: false,
        cancel: &cancel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_errors_read_as_type_and_message() {
        assert_eq!(
            error_body(
                r#"{"type":"error","error":{"type":"not_found_error","message":"model: x"},"request_id":"req_1"}"#
            ),
            "not_found_error: model: x"
        );
        assert_eq!(
            error_body(r#"{"error":{"message":"bad key","type":"invalid_request_error"}}"#),
            "invalid_request_error: bad key"
        );
        assert_eq!(
            error_body(
                r#"{"error":{"message":"limit","type":"usage","code":"subscription_sharing_usage_limit_exceeded"}}"#
            ),
            "subscription_sharing_usage_limit_exceeded: limit"
        );
        assert_eq!(
            error_body("  <html>gateway</html> "),
            "<html>gateway</html>"
        );
    }

    #[test]
    fn effort_levels_follow_the_model() {
        let levels = |spec: &str| Model::parse(spec).efforts();
        let all = ["low", "medium", "high", "xhigh", "max"];
        assert!(levels("anthropic/claude-haiku-4-5").is_empty());
        assert_eq!(
            levels("anthropic/claude-opus-4-5"),
            ["low", "medium", "high"]
        );
        assert_eq!(
            levels("anthropic/claude-sonnet-4-6"),
            ["low", "medium", "high", "max"]
        );
        assert_eq!(levels("anthropic/claude-opus-5-5"), all);
        assert_eq!(
            Model::parse("anthropic/claude-opus-5-5")
                .default_effort()
                .as_deref(),
            Some("high")
        );
        // Every level offered is one the request sends unchanged.
        for id in ["claude-opus-4-5", "claude-sonnet-4-6", "claude-opus-5-5"] {
            for l in levels(&format!("anthropic/{id}")) {
                assert_eq!(
                    anthropic::supported_effort(id, Some(&l)),
                    Some(&*l),
                    "{id} {l}"
                );
            }
        }
        learn(
            "chatgpt/gpt-x",
            Info {
                efforts: vec!["low".into(), "high".into()],
                notes: vec!["fast".into(), "deep".into()],
                default_effort: Some("low".into()),
                ..Info::default()
            },
        );
        let gpt = Model::parse("chatgpt/gpt-x");
        assert_eq!(gpt.efforts(), ["low", "high"]);
        assert_eq!(gpt.effort_notes(), ["fast", "deep"]);
        assert_eq!(gpt.default_effort().as_deref(), Some("low"));
    }

    #[test]
    fn context_windows_by_model() {
        let w = |id: &str| Model::parse(&format!("anthropic/{id}")).context_window();
        assert_eq!(w("claude-opus-5-5"), 1_000_000);
        assert_eq!(w("claude-opus-4-8"), 1_000_000);
        assert_eq!(w("claude-sonnet-4-6"), 1_000_000);
        assert_eq!(w("claude-sonnet-4-5"), 200_000);
        assert_eq!(w("claude-opus-4-1"), 200_000);
        assert_eq!(w("claude-haiku-4-5"), 200_000);
        assert_eq!(w("claude-haiku-5-5"), 1_000_000);
    }

    #[test]
    fn retries_only_transient_failures() {
        assert_eq!(
            retry_reason("529 from https://x: {}"),
            Some("the API is overloaded")
        );
        assert_eq!(
            retry_reason("overloaded_error: busy"),
            Some("the API is overloaded")
        );
        assert_eq!(
            retry_reason("request to https://x failed: timeout"),
            Some("the connection failed")
        );
        assert_eq!(retry_reason("400 from https://x: bad"), None);
        assert_eq!(retry_reason("401 from https://x: no key"), None);
        assert_eq!(retry_reason("invalid_request_error: nope"), None);
        let s = Duration::from_secs(1);
        assert_eq!(
            retry_wait("429 from u (retry after 7s): x", 0, s),
            Duration::from_secs(7)
        );
        assert_eq!(retry_wait("529 from u: x", 2, s), Duration::from_secs(4));
        assert_eq!(retry_wait("529 from u: x", 9, s), Duration::from_secs(30));
    }

    #[test]
    fn parses_model_specs() {
        assert_eq!(Model::parse("claude-sonnet-5-5").api, Api::Anthropic);
        assert_eq!(Model::parse("chatgpt/gpt-6-sol").api, Api::Responses);
        let m = Model::parse("openrouter/moonshot/kimi-k3");
        assert_eq!(
            (m.provider.as_str(), m.id.as_str(), m.api),
            ("openrouter", "moonshot/kimi-k3", Api::OpenAi)
        );
        assert_eq!(Model::parse("gpt-6").provider, "openai");
        assert_eq!(Model::parse(DEFAULT_MODEL).id, "claude-opus-5-5");
        assert_eq!(Model::parse(DEFAULT_MODEL).context_window(), 1_000_000);
        assert_eq!(Model::parse("claude-haiku-4-5").context_window(), 200_000);
        assert_eq!(Model::parse("openai/gpt-x").context_window(), 128_000);
    }
}
