use crate::message::{Block, Message};
use crate::tools::ToolSpec;
use std::io::{BufReader, Read};

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
    }
}
