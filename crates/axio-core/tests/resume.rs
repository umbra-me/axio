//! The M4 acceptance criteria for persistence and resume.

use std::path::PathBuf;
use std::sync::Arc;

use axio_core::agent::{Agent, RuntimeConfig};
use axio_core::approver::NonInteractive;
use axio_core::protocol::Usage;
use axio_core::protocol::{ItemBody, SessionId, ToolStatus, TurnOutcome};
use axio_core::provider::{BlockKind, StopReason, StreamEvent};
use axio_core::record::{
    self, Header, Recorder, SESSION_FORMAT_VERSION, SessionFile, SessionStore,
};
use axio_core::scripted::{Script, ScriptedProvider};
use axio_core::session::Session;
use tokio_util::sync::CancellationToken;

fn header(id: SessionId, cwd: &std::path::Path) -> Header {
    Header {
        version: SESSION_FORMAT_VERSION,
        protocol: axio_core::PROTOCOL_VERSION,
        id,
        cwd: cwd.to_path_buf(),
        model: "claude-opus-5".into(),
        started: "0".into(),
        label: Some("first prompt".into()),
        axio: "test".into(),
    }
}

fn say(text: &str) -> Script {
    Script::Events(vec![
        StreamEvent::BlockStart {
            index: 0,
            kind: BlockKind::Text,
        },
        StreamEvent::TextDelta {
            index: 0,
            text: text.into(),
        },
        StreamEvent::BlockEnd { index: 0 },
        StreamEvent::Usage(Usage {
            input_tokens: 100,
            output_tokens: 20,
            ..Default::default()
        }),
        StreamEvent::Done {
            stop: StopReason::EndTurn,
        },
    ])
}

fn agent_writing_to(path: PathBuf, session: Session, scripts: Vec<Script>) -> Agent {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let header = header(session.id(), session.cwd());
    let recorder = Recorder::File(SessionFile::create(path, &header).unwrap());
    Agent::new(
        Arc::new(ScriptedProvider::new(scripts)),
        Arc::new(NonInteractive::deny()),
        session,
        RuntimeConfig::default(),
        vec![],
        tx,
    )
    .with_recorder(recorder)
}

#[tokio::test]
async fn a_loaded_transcript_is_the_fold_of_its_own_records() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let path = dir.path().join("s.jsonl");

    let session = Session::new(cwd.clone(), "claude-opus-5");
    let id = session.id();
    let mut agent = agent_writing_to(path.clone(), session, vec![say("first answer")]);
    agent
        .run_turn("first prompt".into(), CancellationToken::new())
        .await;
    let original = agent.session().wire_messages("claude-opus-5");
    drop(agent);

    let loaded = record::load(&path).unwrap();
    assert_eq!(loaded.header.id, id);
    assert!(loaded.notices.is_empty());

    // The criterion, scoped as it must be: the loaded transcript is exactly the
    // fold of its own records, so the request built from it is byte-identical.
    assert_eq!(
        serde_json::to_string(&loaded.session.wire_messages("claude-opus-5")).unwrap(),
        serde_json::to_string(&original).unwrap(),
        "a resumed session must reproduce the request the first run would have sent"
    );
}

#[tokio::test]
async fn a_resumed_session_continues_with_the_prior_context() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let path = dir.path().join("s.jsonl");

    let session = Session::new(cwd.clone(), "claude-opus-5");
    let mut agent = agent_writing_to(path.clone(), session, vec![say("remembered")]);
    agent
        .run_turn("what is my name".into(), CancellationToken::new())
        .await;
    drop(agent);

    // A second process picks it up.
    let loaded = record::load(&path).unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut resumed = Agent::new(
        Arc::new(ScriptedProvider::new(vec![say("second answer")])),
        Arc::new(NonInteractive::deny()),
        loaded.session,
        RuntimeConfig::default(),
        vec![],
        tx,
    )
    .with_recorder(Recorder::File(SessionFile::reopen(path.clone()).unwrap()));

    let outcome = resumed
        .run_turn("and again".into(), CancellationToken::new())
        .await;
    assert!(matches!(outcome, TurnOutcome::Completed));

    let wire = resumed.session().wire_messages("claude-opus-5");
    let text = serde_json::to_string(&wire).unwrap();
    assert!(text.contains("what is my name"), "prior context was lost");
    assert!(text.contains("remembered"), "prior answer was lost");
    assert!(text.contains("and again"), "the new prompt is missing");

    // And the file grew rather than being rewritten.
    let reloaded = record::load(&path).unwrap();
    assert_eq!(reloaded.session.transcript().len(), 4);
}

#[tokio::test]
async fn a_turn_records_what_it_cost() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let session = Session::new(dir.path().to_path_buf(), "claude-opus-5");
    let mut agent = agent_writing_to(path.clone(), session, vec![say("done")]);
    agent.run_turn("go".into(), CancellationToken::new()).await;
    drop(agent);

    let text = std::fs::read_to_string(&path).unwrap();
    let turn_ended: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("\"rec\":\"turn_ended\""))
        .collect();
    assert_eq!(turn_ended.len(), 1, "one cost record per turn");
    let value: serde_json::Value = serde_json::from_str(turn_ended[0]).unwrap();
    assert!(value["cost_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(value["usage"]["output_tokens"], 20);
}

#[tokio::test]
async fn an_ephemeral_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let session = Session::new(dir.path().to_path_buf(), "claude-opus-5");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = Agent::new(
        Arc::new(ScriptedProvider::new(vec![say("nothing recorded")])),
        Arc::new(NonInteractive::deny()),
        session,
        RuntimeConfig::default(),
        vec![],
        tx,
    );
    agent.run_turn("go".into(), CancellationToken::new()).await;
    assert!(agent.session_path().is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn a_session_killed_mid_turn_still_loads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let session = Session::new(dir.path().to_path_buf(), "claude-opus-5");
    let mut agent = agent_writing_to(path.clone(), session, vec![say("answer")]);
    agent
        .run_turn("prompt".into(), CancellationToken::new())
        .await;
    drop(agent);

    // Simulate a kill part-way through appending the next record.
    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("{\"rec\":\"item\",\"id\":\"01K\",\"item\":\"user_mes");
    std::fs::write(&path, text).unwrap();

    let loaded = record::load(&path).unwrap();
    assert!(!loaded.degraded, "a torn tail is a crash, not a hole");
    assert_eq!(loaded.notices.len(), 1);
    assert!(
        loaded
            .session
            .transcript()
            .iter()
            .any(|i| matches!(&i.body, ItemBody::AgentMessage { text } if text == "answer")),
        "the completed turn survived the interruption"
    );
}

#[tokio::test]
async fn an_interrupted_tool_call_is_repaired_so_the_next_request_is_valid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let id = SessionId::generate();
    let cwd = dir.path().to_path_buf();

    {
        let mut f = SessionFile::create(path.clone(), &header(id, &cwd)).unwrap();
        f.append(&record::Record::Item {
            item: axio_core::protocol::Item::new(ItemBody::UserMessage {
                text: "do a thing".into(),
            }),
        });
        f.append(&record::Record::Item {
            item: axio_core::protocol::Item::new(ItemBody::ToolCall {
                call_id: "toolu_1".into(),
                name: "bash".into(),
                input: serde_json::json!({"command":"sleep 30"}),
                subject: "bash:sleep".into(),
                preview: None,
                status: ToolStatus::Running,
            }),
        });
    }

    let loaded = record::load(&path).unwrap();
    assert!(loaded.session.unfinished_calls().is_empty());

    // Every tool_use has exactly one tool_result, which is what makes the
    // resumed request acceptable at all.
    let wire = loaded.session.wire_messages("claude-opus-5");
    let uses = wire
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, axio_core::provider::WireContent::ToolUse { .. }))
        .count();
    let results = wire
        .iter()
        .flat_map(|m| &m.content)
        .filter(|c| matches!(c, axio_core::provider::WireContent::ToolResult { .. }))
        .count();
    assert_eq!((uses, results), (1, 1));
}

#[test]
fn a_store_lists_newest_first_and_resolves_a_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new(dir.path().to_path_buf());
    let cwd = dir.path().to_path_buf();

    let mut ids = Vec::new();
    for _ in 0..5 {
        let id = SessionId::generate();
        SessionFile::create(store.path_for(id), &header(id, &cwd)).unwrap();
        ids.push(id);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let files = store.files();
    assert_eq!(files.len(), 5);
    let newest = files[0].file_stem().unwrap().to_string_lossy().into_owned();
    assert_eq!(newest, ids.last().unwrap().to_string(), "newest first");

    let text = ids[0].to_string();
    assert_eq!(store.resolve(&text[..10]).unwrap(), ids[0]);
}
