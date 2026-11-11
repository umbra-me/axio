use crate::message::{Block, Message};
use crate::tools::ToolSpec;
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};

pub mod anthropic;
pub mod openai;

pub const DEFAULT_MODEL: &str = "anthropic/claude-opus-5-5";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Anthropic,
    OpenAi,
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
        let api = if provider == "anthropic" {
            Api::Anthropic
        } else {
            Api::OpenAi
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
        match self.id.as_str() {
            id if id.starts_with("claude-haiku-4") || id.starts_with("claude-3") => 200_000,
            id if id.starts_with("claude-") => 1_000_000,
            _ => 128_000,
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
    ToolCall,
}

pub struct Request<'a> {
    pub model: &'a Model,
    pub system: &'a str,
    pub messages: &'a [Message],
    pub tools: &'a [ToolSpec],
    pub effort: Option<&'a str>,
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
    let mut body = resp.into_body();
    if !(200..300).contains(&status) {
        let text = body.read_to_string().unwrap_or_default();
        return Err(format!("{status} from {url}: {}", text.trim()));
    }
    Ok(BufReader::new(body.into_reader()))
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
        cancel: &cancel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_specs() {
        assert_eq!(Model::parse("claude-sonnet-5-5").api, Api::Anthropic);
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
