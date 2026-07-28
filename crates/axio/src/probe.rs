//! Whether the configured model can be driven as an agent — asked of the model.
//!
//! `--doctor` answers from configuration alone and opens no socket, which is
//! what lets it run anywhere and touch nothing. That same property is why it
//! cannot answer the question a provider actually fails on: a model that
//! serves chat perfectly and rejects every request carrying a tool. Nothing in
//! the configuration is wrong, so nothing reports a problem — the agent simply
//! never acts, and the first symptom is a transport error partway through a
//! turn, with nothing naming the cause.
//!
//! Two requests, one verdict. Opt-in, because unlike every other
//! report-and-exit mode this one spends a credential and a few tokens.

use super::*;

use axio_core::provider::{
    BlockKind, Effort, ModelRequest, Provider, ProviderError, Role, StreamEvent, ToolSpec,
    WireContent, WireMessage,
};
use std::pin::Pin;

pub(crate) async fn probe(resolved: &Resolved) -> u8 {
    let cfg = resolved.config();
    print_notices(resolved);

    let provider = match provider::build_provider(resolved) {
        Ok(provider) => provider,
        Err(message) => {
            eprintln!("axio: {message}");
            return 2;
        }
    };

    let mut out = std::io::stdout();
    let _ = writeln!(out, "axio {VERSION}");
    let _ = writeln!(out);
    let _ = writeln!(out, "probe");
    let _ = writeln!(out, "  provider            {}", cfg.model.provider);
    let _ = writeln!(out, "  model               {}", cfg.model.name);
    let _ = writeln!(out, "  endpoint            {}", endpoint(cfg));

    let cancel = CancellationToken::new();

    // The plain request goes first, and its only job is attribution. Without
    // it a refusal cannot be pinned on anything: a wrong credential, a model
    // that is having a bad afternoon and a model that rejects tools all arrive
    // as one failed request, and guessing between them is what this command
    // exists to stop.
    if let Err(e) = ask(
        provider.as_ref(),
        request(cfg, WithTool::No),
        cancel.clone(),
    )
    .await
    {
        let _ = writeln!(out, "  chat                failed: {}", describe(&e));
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  nothing is claimed about tools: the request without one did not\n  \
             succeed either, so there is no baseline to compare against"
        );
        return 1;
    }
    let _ = writeln!(out, "  chat                ok");

    // Retried once, and only for a failure that says nothing durable. A single
    // 5xx read as a capability limit produces exactly the wrong advice —
    // change your model — for what was a bad few seconds on the server. That
    // is not hypothetical: it is how this command came to be written.
    let verdict = match ask(
        provider.as_ref(),
        request(cfg, WithTool::Yes),
        cancel.clone(),
    )
    .await
    {
        Err(e) if is_transient(&e) => {
            let _ = writeln!(out, "  tools               {}, retrying once", describe(&e));
            ask(provider.as_ref(), request(cfg, WithTool::Yes), cancel).await
        }
        settled => settled,
    };

    match verdict {
        Ok(Some(name)) => {
            let _ = writeln!(out, "  tools               ok, called `{name}`");
            0
        }
        Ok(None) => {
            let _ = writeln!(out, "  tools               accepted, not called");
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  the transport works: the request carrying a tool was accepted.\n  \
                 Whether this model reliably calls one is a question about the\n  \
                 model, and a single sample does not settle it."
            );
            0
        }
        Err(e) if is_transient(&e) => {
            let _ = writeln!(out, "  tools               inconclusive: {}", describe(&e));
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  twice, and a server-side failure is not a statement about the\n  \
                 model. Nothing is concluded here: re-run before changing\n  \
                 anything about the configuration."
            );
            1
        }
        Err(e) => {
            let _ = writeln!(out, "  tools               refused: {}", describe(&e));
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  the same request without a tool succeeded, so this is the tool\n  \
                 payload being rejected rather than the endpoint or the\n  \
                 credential. This model cannot be driven as an agent here."
            );
            1
        }
    }
}

/// Whether a failure says something durable about the model, or only about the
/// last few seconds.
///
/// The distinction is the whole difference between "choose another model" and
/// "try again", and getting it wrong costs someone a working configuration.
fn is_transient(e: &ProviderError) -> bool {
    match e {
        ProviderError::Http { status, .. } => *status >= 500,
        ProviderError::Overloaded
        | ProviderError::RateLimited { .. }
        | ProviderError::Transport(_)
        | ProviderError::Truncated => true,
        _ => false,
    }
}

/// Whether the request being built offers the model a tool.
///
/// A bool parameter at two call sites reads as `request(cfg, true)`, which
/// says nothing at the point it matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WithTool {
    Yes,
    No,
}

/// Where the request will actually go.
///
/// Resolved the same way `build_provider` resolves it, because the endpoint a
/// stale `base_url` points at is exactly what a probe is for.
pub(crate) fn endpoint(cfg: &axio_core::config::Config) -> String {
    cfg.model.base_url.clone().unwrap_or_else(|| {
        match cfg.model.provider.as_str() {
            "anthropic" => axio_provider::API_URL,
            "openai-codex" => axio_provider::CODEX_BASE,
            _ => OLLAMA_BASE,
        }
        .to_owned()
    })
}

/// The smallest request that answers the question.
///
/// `effort` is pinned low rather than taken from the configuration, and
/// `max_tokens` is small. Depth is not what is being measured, and a probe
/// that costs a full reasoning budget is a probe people stop running. Low is
/// used rather than disabling thinking outright because disabling it is itself
/// a 400 on some models — the probe must not fail for a reason it invented.
fn request(cfg: &axio_core::config::Config, tool: WithTool) -> ModelRequest {
    let mut req = ModelRequest::new(cfg.model.name.clone());
    req.max_tokens = 1024;
    req.effort = Effort::Low;
    req.messages = vec![WireMessage {
        role: Role::User,
        content: vec![WireContent::Text {
            text: match tool {
                WithTool::Yes => "Call the ping tool with note set to \"probe\".",
                WithTool::No => "Reply with exactly the word: ok",
            }
            .to_owned(),
        }],
    }];

    if tool == WithTool::Yes {
        req.tools = Arc::from(vec![ToolSpec {
            name: "ping".to_owned(),
            description: "Record a short note. Exists only to check that tool calls work."
                .to_owned(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "note": { "type": "string" } },
                "required": ["note"]
            }),
        }]);
    }

    req
}

/// Send one request; report the name of the first tool call it produced.
///
/// A truncated stream is not treated as failure the way [`complete`] treats
/// it. The question here is whether the request was accepted, and a short
/// `max_tokens` makes an early stop the expected case rather than a fault.
///
/// [`complete`]: axio_core::provider::complete
async fn ask(
    provider: &dyn Provider,
    req: ModelRequest,
    cancel: CancellationToken,
) -> Result<Option<String>, ProviderError> {
    use futures_core::Stream;

    let mut stream = provider.stream(req, cancel).await?;
    let mut called: Option<String> = None;

    std::future::poll_fn(|cx| {
        loop {
            match Pin::new(&mut stream).poll_next(cx) {
                std::task::Poll::Ready(Some(Ok(event))) => {
                    if let StreamEvent::BlockStart {
                        kind: BlockKind::ToolUse { name, .. },
                        ..
                    } = event
                        && called.is_none()
                    {
                        called = Some(name);
                    }
                }
                std::task::Poll::Ready(Some(Err(e))) => return std::task::Poll::Ready(Err(e)),
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(Ok(())),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    })
    .await?;

    Ok(called)
}

/// One line, and never the body.
///
/// The body of a rejected request is where a provider echoes back what it was
/// sent, which on an authentication failure is the credential.
fn describe(e: &ProviderError) -> String {
    match e {
        ProviderError::Http { status, .. } => format!("http {status}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: &str, base_url: Option<&str>) -> axio_core::config::Config {
        let mut cfg = axio_core::config::Config::default();
        cfg.model.provider = provider.to_owned();
        cfg.model.name = "a-model".to_owned();
        cfg.model.base_url = base_url.map(str::to_owned);
        cfg
    }

    #[test]
    fn only_the_tool_request_carries_one() {
        let cfg = config("ollama", None);
        assert!(
            request(&cfg, WithTool::No).tools.is_empty(),
            "the baseline exists to be the same request minus the tool"
        );
        assert_eq!(request(&cfg, WithTool::Yes).tools.len(), 1);
    }

    /// The probe must not fail for a reason it introduced itself. Effort is
    /// pinned rather than inherited, so a configuration asking for maximum
    /// depth cannot turn a capability check into an expensive one.
    #[test]
    fn depth_is_not_inherited_from_the_configuration() {
        let mut cfg = config("ollama", None);
        cfg.model.effort = Effort::XHigh;
        assert_eq!(request(&cfg, WithTool::Yes).effort, Effort::Low);
    }

    #[test]
    fn the_reported_endpoint_is_the_one_that_will_be_used() {
        assert_eq!(endpoint(&config("ollama", None)), OLLAMA_BASE);
        assert_eq!(endpoint(&config("anthropic", None)), axio_provider::API_URL);
        // The whole point: an override is what a probe is meant to expose.
        assert_eq!(
            endpoint(&config("ollama", Some("https://gateway.internal/v1"))),
            "https://gateway.internal/v1"
        );
    }

    /// The regression this exists for happened to a person, not to a test: two
    /// consecutive 5xx responses were read as "this model cannot call tools",
    /// a working configuration was changed on the strength of it, and the
    /// model had been fine the whole time.
    #[test]
    fn a_server_side_failure_is_not_a_verdict_on_the_model() {
        let server_error = ProviderError::Http {
            status: 500,
            body: axio_core::redact::Redacted::new(String::new()),
        };
        assert!(is_transient(&server_error), "5xx says nothing durable");
        assert!(is_transient(&ProviderError::Overloaded));
        assert!(is_transient(&ProviderError::Truncated));

        // A rejected payload is deterministic: the same request fails the same
        // way every time, and retrying it only wastes the user's tokens.
        let rejected = ProviderError::Http {
            status: 400,
            body: axio_core::redact::Redacted::new(String::new()),
        };
        assert!(!is_transient(&rejected), "4xx is the model's actual answer");
        assert!(!is_transient(&ProviderError::ContextOverflow));
    }

    /// A rejected request is where a provider echoes back what it received,
    /// and on an auth failure that is the credential.
    #[test]
    fn an_http_failure_reports_the_status_and_not_the_body() {
        let e = ProviderError::Http {
            status: 500,
            body: axio_core::redact::Redacted::new("sk-should-never-appear".to_owned()),
        };
        let line = describe(&e);
        assert_eq!(line, "http 500");
        assert!(!line.contains("sk-"), "{line}");
    }
}
