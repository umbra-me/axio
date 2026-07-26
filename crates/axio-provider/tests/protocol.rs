//! The M1 acceptance criteria, as executable tests.

use std::sync::Arc;
use std::time::Duration;

use axio_core::protocol::Usage;
use axio_core::provider::{
    BlockKind, CachePlan, Effort, ModelRequest, ProviderError, ReasoningDisplay, Role, StopReason,
    StreamEvent, SystemBlock, ToolSpec, WireContent, WireMessage,
};
use axio_provider::anthropic::{
    AnthropicStream, ToolInputAccumulator, build_body, classify, rolling_cache_plan,
};
use serde_json::json;

const FIXTURE: &[u8] = include_bytes!("fixtures/turn.sse");

fn decode(bytes: &[u8]) -> Vec<StreamEvent> {
    let mut s = AnthropicStream::new();
    let mut out = s.push(bytes).expect("fixture decodes");
    out.extend(s.finish().expect("fixture is not truncated"));
    out
}

// --------------------------------------------------------------- stream shape

#[test]
fn fixture_maps_to_the_expected_event_sequence() {
    let events = decode(FIXTURE);

    assert!(matches!(events[0], StreamEvent::MessageStart { .. }));
    assert!(matches!(
        events[1],
        StreamEvent::Usage(Usage {
            input_tokens: 1240,
            cache_read_input_tokens: 980,
            ..
        })
    ));
    assert!(matches!(
        events[2],
        StreamEvent::BlockStart {
            index: 0,
            kind: BlockKind::Thinking
        }
    ));
    // A tool_use block carries its id and name from content_block_start.
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::BlockStart { kind: BlockKind::ToolUse { id, name }, .. }
            if id == "toolu_01Ab" && name == "read"
    )));
    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            stop: StopReason::ToolUse
        })
    ));
}

#[test]
fn thinking_signature_is_surfaced_so_it_can_be_echoed_back() {
    let events = decode(FIXTURE);
    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::ReasoningSignature { signature, .. } if signature == "ErUBCkYIB"
    )));
}

#[test]
fn three_input_json_fragments_concatenate_and_parse_exactly_once() {
    let events = decode(FIXTURE);

    let fragments: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolInputDelta { json, .. } => Some(json.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(fragments.len(), 3, "fixture streams three fragments");
    // Each fragment on its own is not valid JSON — parsing per delta fails.
    assert!(serde_json::from_str::<serde_json::Value>(fragments[0]).is_err());

    let mut acc = ToolInputAccumulator::new();
    for e in &events {
        match e {
            StreamEvent::ToolInputDelta { index, json } => acc.push(*index, json),
            StreamEvent::BlockEnd { index } if *index == 2 => {
                let parsed = acc.finish(*index).expect("assembled JSON parses");
                assert_eq!(parsed, json!({"path": "src/lib.rs"}));
            }
            _ => {}
        }
    }
    assert_eq!(acc.parse_count(), 1, "parsed exactly once, at block end");
}

#[test]
fn a_stream_without_message_stop_is_truncated() {
    // Cut on a frame boundary: every frame is well-formed, message_stop is
    // simply absent.
    let end = find_last_frame_boundary(FIXTURE);
    let mut s = AnthropicStream::new();
    let _ = s.push(&FIXTURE[..end]).unwrap();
    let err = s
        .finish()
        .expect_err("a truncated stream must not look complete");
    assert!(matches!(err, ProviderError::Truncated), "got {err:?}");
    assert!(err.retryable());
}

#[test]
fn a_stream_cut_mid_json_is_truncated_not_a_decode_error() {
    // The connection dropped halfway through a `data:` line. Reporting a decode
    // failure here would turn a retryable truncation into a fatal-looking bug.
    let mut s = AnthropicStream::new();
    let _ = s.push(&FIXTURE[..FIXTURE.len() - 60]).unwrap();
    let err = s
        .finish()
        .expect_err("a mid-JSON cut must not look complete");
    assert!(matches!(err, ProviderError::Truncated), "got {err:?}");
    assert!(err.retryable());
}

/// Index just past the second-to-last `\r\n\r\n`, i.e. the start of the final
/// frame — so slicing there yields a stream of whole frames.
fn find_last_frame_boundary(bytes: &[u8]) -> usize {
    let sep = b"\r\n\r\n";
    let mut ends: Vec<usize> = Vec::new();
    for i in 0..bytes.len().saturating_sub(sep.len() - 1) {
        if &bytes[i..i + sep.len()] == sep {
            ends.push(i + sep.len());
        }
    }
    ends[ends.len() - 2]
}

#[test]
fn refusal_arrives_on_the_success_path_not_as_an_error() {
    let body = concat!(
        "event: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":{"type":"refusal","category":"cyber"}}}"#,
        "\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    let events = decode(body.as_bytes());
    match events.last() {
        Some(StreamEvent::Done {
            stop: StopReason::Refusal { category },
        }) => assert_eq!(category.as_deref(), Some("cyber")),
        other => panic!("expected a refusal stop reason, got {other:?}"),
    }
}

#[test]
fn a_mid_stream_error_frame_becomes_a_provider_error() {
    let body = concat!(
        "event: error\n",
        r#"data: {"type":"error","error":{"type":"overloaded_error","message":"try later"}}"#,
        "\n\n"
    );
    let mut s = AnthropicStream::new();
    let err = s.push(body.as_bytes()).expect_err("an error frame fails");
    assert!(matches!(err, ProviderError::Overloaded));
    assert!(err.retryable());
}

#[test]
fn an_unknown_delta_kind_is_ignored_rather_than_fatal() {
    let body = concat!(
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{}}}"#,
        "\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
    );
    assert!(decode(body.as_bytes()).is_empty());
}

// ------------------------------------------------------- error classification

#[test]
fn rate_limit_honours_retry_after() {
    let e = classify(429, Some("3"), r#"{"error":{"type":"rate_limit_error"}}"#);
    match &e {
        ProviderError::RateLimited { retry_after } => {
            assert_eq!(*retry_after, Some(Duration::from_secs(3)))
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
    assert!(e.retryable());
    assert_eq!(e.backoff_hint(), Some(Duration::from_secs(3)));
}

#[test]
fn oversize_prompt_is_context_overflow_not_a_generic_400() {
    let e = classify(
        400,
        None,
        r#"{"error":{"type":"invalid_request_error","message":"prompt is too long: 1200000 tokens > 1000000 maximum"}}"#,
    );
    assert!(matches!(e, ProviderError::ContextOverflow));
    assert!(!e.retryable(), "compaction is the fix, not a retry");
}

#[test]
fn a_plain_400_is_not_retryable_and_a_500_is() {
    let bad = classify(400, None, r#"{"error":{"message":"bad tool schema"}}"#);
    assert!(!bad.retryable());
    let server = classify(500, None, "upstream exploded");
    assert!(server.retryable());
}

#[test]
fn auth_and_overload_statuses() {
    assert!(matches!(
        classify(401, None, r#"{"error":{"message":"invalid x-api-key"}}"#),
        ProviderError::Auth(_)
    ));
    assert!(matches!(
        classify(529, None, "{}"),
        ProviderError::Overloaded
    ));
}

#[test]
fn an_error_body_is_redacted_before_it_can_be_printed() {
    let e = classify(
        400,
        None,
        "bad request with key sk-ant-api03-SECRETVALUE123",
    );
    let shown = format!("{e}");
    assert!(!shown.contains("sk-ant-api03-SECRETVALUE123"));
    assert!(!format!("{e:?}").contains("SECRETVALUE123"));
}

// ------------------------------------------------------------- request body

fn sample_request() -> ModelRequest {
    let mut req = ModelRequest::new("claude-opus-5");
    req.system = Arc::from(vec![
        SystemBlock {
            text: "You are axio.".into(),
        },
        SystemBlock {
            text: "Platform: linux, shell: zsh.".into(),
        },
    ]);
    req.tools = Arc::from(vec![
        ToolSpec {
            name: "read".into(),
            description: "Read a file.".into(),
            input_schema: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        },
        ToolSpec {
            name: "bash".into(),
            description: "Run a command.".into(),
            input_schema: json!({"type":"object","properties":{"command":{"type":"string"}},"required":["command"]}),
        },
    ]);
    req.messages = vec![WireMessage {
        role: Role::User,
        content: vec![WireContent::Text {
            text: "review src/lib.rs".into(),
        }],
    }];
    req.reasoning = ReasoningDisplay::Summarized;
    req
}

#[test]
fn body_carries_xhigh_effort_and_none_of_the_forbidden_knobs() {
    let body = build_body(&sample_request());

    assert_eq!(body["output_config"]["effort"], "xhigh");
    for forbidden in ["temperature", "top_p", "top_k", "budget_tokens"] {
        assert!(
            body.get(forbidden).is_none(),
            "{forbidden} is a 400 on this model and must not be emitted"
        );
    }
    // Also not nested inside thinking.
    assert!(body["thinking"].get("budget_tokens").is_none());
    assert_eq!(body["thinking"]["type"], "adaptive");
    assert_eq!(body["stream"], true);
}

#[test]
fn cache_control_lands_on_the_last_tool_and_the_last_system_block() {
    let body = build_body(&sample_request());

    let system = body["system"].as_array().unwrap();
    assert!(system[0].get("cache_control").is_none());
    assert_eq!(system[1]["cache_control"]["type"], "ephemeral");

    let tools = body["tools"].as_array().unwrap();
    assert!(tools[0].get("cache_control").is_none());
    assert_eq!(tools[1]["cache_control"]["type"], "ephemeral");
}

#[test]
fn omitting_reasoning_display_omits_the_thinking_field_entirely() {
    let mut req = sample_request();
    req.reasoning = ReasoningDisplay::Omitted;
    let body = build_body(&req);
    // Adaptive is the default on this model, so saying nothing is correct —
    // and there is no code path that can emit `disabled`.
    assert!(body.get("thinking").is_none());
}

#[test]
fn serialisation_is_byte_stable_across_runs() {
    // Non-deterministic key order silently destroys the cache prefix and
    // nothing errors when it happens.
    let first = serde_json::to_string(&build_body(&sample_request())).unwrap();
    for _ in 0..100 {
        assert_eq!(
            serde_json::to_string(&build_body(&sample_request())).unwrap(),
            first
        );
    }
}

#[test]
fn request_body_snapshot() {
    insta::assert_json_snapshot!(build_body(&sample_request()));
}

#[test]
fn effort_is_the_only_depth_knob_and_it_round_trips() {
    for (effort, wire) in [
        (Effort::Low, "low"),
        (Effort::Medium, "medium"),
        (Effort::High, "high"),
        (Effort::XHigh, "xhigh"),
        (Effort::Max, "max"),
    ] {
        let mut req = sample_request();
        req.effort = effort;
        assert_eq!(build_body(&req)["output_config"]["effort"], wire);
    }
}

#[test]
fn the_rolling_breakpoint_is_placed_before_the_lookback_window_closes() {
    // The window is 20 content blocks; re-placing at 15 keeps a margin.
    assert_eq!(rolling_cache_plan(4, 14).message, None);
    assert_eq!(rolling_cache_plan(4, 15).message, Some(3));
    assert_eq!(rolling_cache_plan(0, 99).message, None);
}

#[test]
fn a_rolling_breakpoint_attaches_to_the_last_block_of_its_message() {
    let mut req = sample_request();
    req.messages.push(WireMessage {
        role: Role::Assistant,
        content: vec![
            WireContent::Text { text: "one".into() },
            WireContent::Text { text: "two".into() },
        ],
    });
    req.cache = CachePlan {
        tools: true,
        system: true,
        message: Some(1),
    };
    let body = build_body(&req);
    let content = body["messages"][1]["content"].as_array().unwrap();
    assert!(content[0].get("cache_control").is_none());
    assert_eq!(content[1]["cache_control"]["type"], "ephemeral");
}

/// Regression. The redaction invariant is "no provider body reaches an error in
/// the clear", not "the Http variant redacts". A 401 or 403 is the response most
/// likely to quote back what was sent, and an intermediary may echo the header
/// verbatim — so the variants that carry those bodies must redact too.
#[test]
fn every_error_variant_carrying_a_body_redacts_it() {
    const KEY: &str = "sk-ant-api03-SECRETVALUE123456";

    let cases = vec![
        (
            "401",
            classify(401, None, &format!("invalid x-api-key {KEY}")),
        ),
        ("403", classify(403, None, &format!("proxy rejected {KEY}"))),
        ("400", classify(400, None, &format!("bad request {KEY}"))),
        ("500", classify(500, None, &format!("upstream said {KEY}"))),
    ];
    for (label, err) in cases {
        assert!(
            !format!("{err}").contains(KEY),
            "{label}: Display leaked the key: {err}"
        );
        assert!(
            !format!("{err:?}").contains(KEY),
            "{label}: Debug leaked the key"
        );
    }

    // And a mid-stream error frame, which routes through a different path.
    let frame = format!(
        "event: error\ndata: {{\"type\":\"error\",\"error\":{{\"type\":\"api_error\",\"message\":\"upstream said {KEY}\"}}}}\n\n"
    );
    let mut s = AnthropicStream::new();
    let err = s.push(frame.as_bytes()).expect_err("an error frame fails");
    assert!(
        !format!("{err}").contains(KEY),
        "error frame leaked the key: {err}"
    );
    assert!(!format!("{err:?}").contains(KEY));
}

/// A credential registered at client construction is scrubbed by value, not just
/// by shape — a proxy token looks nothing like an API key.
#[test]
fn a_registered_credential_is_scrubbed_from_every_variant() {
    const TOKEN: &str = "my-corporate-proxy-token-abcdef";
    axio_core::redact::register_secret(TOKEN);

    for err in [
        classify(403, None, &format!("proxy rejected token {TOKEN}")),
        classify(400, None, &format!("bad request {TOKEN}")),
    ] {
        assert!(!format!("{err}").contains(TOKEN), "leaked: {err}");
        assert!(!format!("{err:?}").contains(TOKEN));
    }
}

// ------------------------------------------------- the documented wire format
//
// `turn.sse` above is hand-written: it exercises the shapes axio cares about,
// but it is evidence about our own assumptions, not about the API. These three
// are the complete response transcripts published in the streaming
// documentation — basic text, tool use, and thinking — copied verbatim. They
// cannot prove the endpoint behaves as documented, but they do prove the
// decoder handles the exact bytes the documentation says to expect, which is
// the strongest claim available without a key.

const DOC_TEXT: &[u8] = include_bytes!("fixtures/doc-text.sse");
const DOC_TOOL_USE: &[u8] = include_bytes!("fixtures/doc-tool-use.sse");
const DOC_THINKING: &[u8] = include_bytes!("fixtures/doc-thinking.sse");

#[test]
fn the_documented_text_stream_decodes() {
    let events = decode(DOC_TEXT);

    assert!(matches!(
        events.first(),
        Some(StreamEvent::MessageStart { .. })
    ));
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Hello!");
    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            stop: StopReason::EndTurn
        })
    ));

    // A `ping` carries nothing and must not appear as an event.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { text, .. } if text.is_empty()))
    );
}

#[test]
fn the_documented_tool_use_stream_decodes() {
    let events = decode(DOC_TOOL_USE);

    let started: Vec<&BlockKind> = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::BlockStart { kind, .. } => Some(kind),
            _ => None,
        })
        .collect();
    assert!(matches!(started[0], BlockKind::Text));
    match started[1] {
        BlockKind::ToolUse { id, name } => {
            assert_eq!(id, "toolu_01T1x1fJ34qAmk2tNTrN7Up6");
            assert_eq!(name, "get_weather");
        }
        other => panic!("expected a tool use block, got {other:?}"),
    }

    // The fragments are partial JSON: each one alone is invalid, and only the
    // concatenation parses.
    let joined: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ToolInputDelta { json, .. } => Some(json.as_str()),
            _ => None,
        })
        .collect();
    let parsed: serde_json::Value = serde_json::from_str(&joined).expect("the fragments join");
    assert_eq!(parsed["location"], "San Francisco, CA");

    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            stop: StopReason::ToolUse
        })
    ));
}

#[test]
fn the_documented_thinking_stream_decodes() {
    let events = decode(DOC_THINKING);

    assert!(events.iter().any(|e| matches!(
        e,
        StreamEvent::BlockStart {
            kind: BlockKind::Thinking,
            ..
        }
    )));
    let thinking: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::ReasoningDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(thinking.contains("Euclidean algorithm"));

    // The signature arrives as its own delta just before the block closes, and
    // is required on the way back: echoing the text without it is rejected.
    let signature = events.iter().find_map(|e| match e {
        StreamEvent::ReasoningSignature { signature, .. } => Some(signature.as_str()),
        _ => None,
    });
    assert_eq!(
        signature,
        Some("EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds...")
    );

    let answer: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        answer,
        "The greatest common divisor of 1071 and 462 is **21**."
    );
}

/// Every documented stream, re-fed one byte at a time.
///
/// The decoder is fed whatever the socket hands it, which is not aligned to
/// anything. A frame split across two reads is the normal case, not an edge one.
#[test]
fn the_documented_streams_survive_any_byte_split() {
    for (name, fixture) in [
        ("text", DOC_TEXT),
        ("tool_use", DOC_TOOL_USE),
        ("thinking", DOC_THINKING),
    ] {
        let whole = decode(fixture);
        let mut s = AnthropicStream::new();
        let mut piecemeal = Vec::new();
        for byte in fixture {
            piecemeal.extend(s.push(&[*byte]).expect("a byte at a time decodes"));
        }
        piecemeal.extend(s.finish().expect("not truncated"));
        assert_eq!(
            format!("{whole:?}"),
            format!("{piecemeal:?}"),
            "{name} decoded differently one byte at a time"
        );
    }
}

/// The documented mid-stream error, which is how a 529 arrives once headers
/// have already been sent. It must be retryable, not fatal.
#[test]
fn a_mid_stream_overload_is_retryable() {
    let bytes = b"event: error\ndata: {\"type\": \"error\", \"error\": {\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}\n\n";
    let mut s = AnthropicStream::new();
    let err = s.push(bytes).expect_err("an error event fails the stream");
    assert!(matches!(err, ProviderError::Overloaded));
    assert!(err.retryable());
}

/// New event types may be added at any time, and the documented instruction is
/// to handle them gracefully rather than fail.
#[test]
fn an_unknown_event_type_is_ignored_rather_than_fatal() {
    let bytes = b"event: something_new\ndata: {\"type\": \"something_new\", \"whatever\": 1}\n\n";
    let mut s = AnthropicStream::new();
    let out = s.push(bytes).expect("an unknown event is not an error");
    assert!(out.is_empty());
}

/// A server-side fallback response opens one of these at each model boundary:
/// a start/stop pair with no deltas between them. Treated as a text block it
/// becomes an empty assistant message, which is then echoed back on every
/// subsequent request for the rest of the session.
#[test]
fn a_fallback_block_produces_no_item() {
    let bytes = concat!(
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"fallback\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    )
    .as_bytes();
    let mut s = AnthropicStream::new();
    let out = s.push(bytes).expect("a fallback block is not an error");
    assert!(
        !out.iter()
            .any(|e| matches!(e, StreamEvent::BlockStart { .. })),
        "a fallback block must not open a content block: {out:?}"
    );
}
