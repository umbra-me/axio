//! The M3 acceptance criteria: what the loop does when tools are real.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
// Only the unix-gated tests below need these; on other platforms they would be
// unused imports, which fail clippy at -D warnings.
#[cfg(unix)]
use std::time::{Duration, Instant};

use axio_core::agent::{Agent, RuntimeConfig};
use axio_core::approver::Approver;
use axio_core::policy::Policy;
#[cfg(unix)]
use axio_core::protocol::ToolStatus;
use axio_core::protocol::{ApprovalRequest, Decision, Event, EventKind, ItemBody, TurnOutcome};
use axio_core::provider::{BlockKind, Role, StopReason, StreamEvent, WireContent};
use axio_core::scripted::{Script, ScriptedProvider};
use axio_core::session::Session;
use axio_core::tool::{Tool, ToolCx};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------------ harness

/// Records how many times it was asked, and answers however the test says.
struct CountingApprover {
    asked: Arc<AtomicUsize>,
    answer: Decision,
}

#[async_trait::async_trait]
impl Approver for CountingApprover {
    async fn decide(&self, _req: &ApprovalRequest) -> Decision {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.answer.clone()
    }
}

fn tool_call(index: u32, id: &str, name: &str, args: &str) -> Vec<StreamEvent> {
    vec![
        StreamEvent::BlockStart {
            index,
            kind: BlockKind::ToolUse {
                id: id.into(),
                name: name.into(),
            },
        },
        StreamEvent::ToolInputDelta {
            index,
            json: args.into(),
        },
        StreamEvent::BlockEnd { index },
    ]
}

fn turn_with(calls: Vec<StreamEvent>) -> Script {
    let mut events = calls;
    events.push(StreamEvent::Done {
        stop: StopReason::ToolUse,
    });
    Script::Events(events)
}

fn done() -> Script {
    Script::Events(vec![
        StreamEvent::BlockStart {
            index: 0,
            kind: BlockKind::Text,
        },
        StreamEvent::TextDelta {
            index: 0,
            text: "finished".into(),
        },
        StreamEvent::BlockEnd { index: 0 },
        StreamEvent::Done {
            stop: StopReason::EndTurn,
        },
    ])
}

struct Harness {
    agent: Agent,
    rx: mpsc::UnboundedReceiver<Event>,
    _dir: tempfile::TempDir,
    root: PathBuf,
}

fn harness(scripts: Vec<Script>, policy: Policy, approver: Arc<dyn Approver>) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();

    let mut agent = Agent::new(
        Arc::new(ScriptedProvider::new(scripts)),
        approver,
        Session::new(root.clone(), "claude-opus-5"),
        RuntimeConfig {
            spill_dir: Some(root.join(".axio-spill")),
            max_output_bytes: 4096,
            ..Default::default()
        },
        vec![],
        tx,
    )
    .with_policy(policy)
    .with_env(axio_core::tool::ToolEnv {
        vars: axio_tools::proc::child_env(),
    });

    for tool in axio_tools::all() {
        agent.register_tool(tool);
    }
    Harness {
        agent,
        rx,
        _dir: dir,
        root,
    }
}

fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

/// The tool_results of the last request the provider was asked for — i.e. what
/// the model would actually see next.
fn last_results(agent: &Agent) -> Vec<(String, String, bool)> {
    agent
        .session()
        .wire_messages("claude-opus-5")
        .into_iter()
        .filter(|m| m.role == Role::User)
        .flat_map(|m| m.content)
        .filter_map(|c| match c {
            WireContent::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => Some((tool_use_id, content, is_error)),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ tests

#[test]
fn the_whole_tool_set_serialises_identically_every_time() {
    // Tool definitions render first in the request, so unstable bytes here cost
    // the entire cache prefix with nothing to show for it.
    let render = || {
        let specs: Vec<Value> = axio_tools::all()
            .iter()
            .map(|t| json!({"name": t.name(), "schema": t.schema()}))
            .collect();
        serde_json::to_string(&specs).unwrap()
    };
    let first = render();
    for _ in 0..100 {
        assert_eq!(render(), first);
    }
}

#[test]
fn the_tool_set_is_the_six_that_were_agreed() {
    let names: Vec<String> = axio_tools::all()
        .iter()
        .map(|t| t.name().to_owned())
        .collect();
    assert_eq!(names, ["read", "write", "edit", "glob", "grep", "bash"]);
    for tool in axio_tools::all() {
        assert!(
            !tool.description().trim().is_empty(),
            "{} has no description",
            tool.name()
        );
    }
}

#[tokio::test]
async fn reads_are_never_asked_about_and_writes_always_are() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(0, "t1", "read", r#"{"path":"a.txt"}"#)),
            done(),
        ],
        Policy::new(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Allow,
        }),
    );
    std::fs::write(h.root.join("a.txt"), "hello").unwrap();

    h.agent
        .run_turn("read it".into(), CancellationToken::new())
        .await;
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a read must not prompt");

    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "write",
                r#"{"path":"b.txt","content":"x"}"#,
            )),
            done(),
        ],
        Policy::new(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("write it".into(), CancellationToken::new())
        .await;
    assert_eq!(asked.load(Ordering::SeqCst), 1, "a write must prompt");
    assert_eq!(std::fs::read_to_string(h.root.join("b.txt")).unwrap(), "x");
}

#[tokio::test]
async fn yes_never_asks() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "write",
                r#"{"path":"c.txt","content":"x"}"#,
            )),
            done(),
        ],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("write it".into(), CancellationToken::new())
        .await;
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    assert!(h.root.join("c.txt").exists());
}

#[tokio::test]
async fn a_denial_steers_the_model_and_the_turn_continues() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "write",
                r#"{"path":"d.txt","content":"x"}"#,
            )),
            done(),
        ],
        Policy::new(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Deny {
                feedback: Some("use the existing helper".into()),
            },
        }),
    );
    let outcome = h
        .agent
        .run_turn("write it".into(), CancellationToken::new())
        .await;

    // The turn ran a second step rather than ending on the refusal.
    assert!(matches!(outcome, TurnOutcome::Completed));
    assert!(
        !h.root.join("d.txt").exists(),
        "a denied write must not happen"
    );

    let results = last_results(&h.agent);
    assert_eq!(results.len(), 1);
    assert!(
        results[0].1.contains("use the existing helper"),
        "the feedback must reach the model: {:?}",
        results[0]
    );
    assert!(results[0].2, "a denial is an error result");
}

#[tokio::test]
async fn every_call_of_a_batch_answers_in_call_order_in_one_message() {
    let mut calls = tool_call(0, "t1", "read", r#"{"path":"one.txt"}"#);
    calls.extend(tool_call(1, "t2", "read", r#"{"path":"two.txt"}"#));
    calls.extend(tool_call(
        2,
        "t3",
        "write",
        r#"{"path":"three.txt","content":"3"}"#,
    ));

    let mut h = harness(
        vec![turn_with(calls), done()],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    std::fs::write(h.root.join("one.txt"), "1").unwrap();
    std::fs::write(h.root.join("two.txt"), "2").unwrap();

    h.agent
        .run_turn("do three things".into(), CancellationToken::new())
        .await;

    let wire = h.agent.session().wire_messages("claude-opus-5");
    let result_messages: Vec<_> = wire
        .iter()
        .filter(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|c| matches!(c, WireContent::ToolResult { .. }))
        })
        .collect();
    assert_eq!(
        result_messages.len(),
        1,
        "all results belong in a single user message"
    );
    assert_eq!(result_messages[0].content.len(), 3);

    let results = last_results(&h.agent);
    let ids: Vec<&str> = results.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(ids, ["t1", "t2", "t3"], "call order, not completion order");
    assert_eq!(
        std::fs::read_to_string(h.root.join("three.txt")).unwrap(),
        "3"
    );
}

#[tokio::test]
async fn a_protected_file_is_refused_even_though_reads_are_auto_approved() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(0, "t1", "read", r#"{"path":".env"}"#)),
            done(),
        ],
        // The most permissive rule a user could write.
        Policy::new().allow_rule("read:*").unwrap(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Allow,
        }),
    );
    std::fs::write(h.root.join(".env"), "SECRET=1").unwrap();

    h.agent
        .run_turn("read the env".into(), CancellationToken::new())
        .await;

    let results = last_results(&h.agent);
    assert!(results[0].2, "reading .env must fail");
    assert!(
        !results[0].1.contains("SECRET"),
        "the contents must not leak into the transcript: {}",
        results[0].1
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "it was denied, not asked");
}

#[tokio::test]
async fn a_compound_command_cannot_use_an_allow_rule_for_its_first_word() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "bash",
                r#"{"command":"echo one; echo two"}"#,
            )),
            done(),
        ],
        Policy::new().allow_rule("bash:echo*").unwrap(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Deny { feedback: None },
        }),
    );
    h.agent
        .run_turn("run it".into(), CancellationToken::new())
        .await;

    assert_eq!(
        asked.load(Ordering::SeqCst),
        1,
        "a compound command must fall through to an explicit approval"
    );

    // The subject recorded in the transcript is the unmatchable one.
    let subject = h
        .agent
        .session()
        .transcript()
        .iter()
        .find_map(|i| match &i.body {
            ItemBody::ToolCall { subject, .. } => Some(subject.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(subject, "bash:!compound");
}

#[cfg(unix)]
#[tokio::test]
async fn a_simple_command_matching_an_allow_rule_runs_without_asking() {
    let asked = Arc::new(AtomicUsize::new(0));
    let mut h = harness(
        vec![
            turn_with(tool_call(0, "t1", "bash", r#"{"command":"echo hello"}"#)),
            done(),
        ],
        Policy::new().allow_rule("bash:echo*").unwrap(),
        Arc::new(CountingApprover {
            asked: asked.clone(),
            answer: Decision::Deny { feedback: None },
        }),
    );
    h.agent
        .run_turn("run it".into(), CancellationToken::new())
        .await;
    assert_eq!(asked.load(Ordering::SeqCst), 0);
    let results = last_results(&h.agent);
    assert!(results[0].1.contains("hello"), "{:?}", results[0]);
}

#[tokio::test]
async fn an_unknown_tool_and_a_bad_argument_both_still_answer() {
    let mut calls = tool_call(0, "t1", "nonexistent", "{}");
    calls.extend(tool_call(1, "t2", "read", r#"{"nopath":true}"#));

    let mut h = harness(
        vec![turn_with(calls), done()],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("go".into(), CancellationToken::new())
        .await;

    let results = last_results(&h.agent);
    assert_eq!(results.len(), 2, "every tool_use needs a result");
    assert!(results[0].1.contains("unknown tool"));
    assert!(results[1].1.contains("path"));
    assert!(results.iter().all(|r| r.2));
}

#[tokio::test]
async fn planning_does_not_touch_the_file_it_previews() {
    // A preview that mutates is how an approved diff stops matching what runs.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let path = root.join("target.txt");
    std::fs::write(&path, "original\n").unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();

    let workspace = Arc::new(axio_core::tool::Workspace::new(&root).unwrap());
    let cx = ToolCx {
        workspace,
        cancel: CancellationToken::new(),
        progress: axio_core::tool::ProgressSink::null(),
        limits: Default::default(),
        env: Arc::new(Default::default()),
    };

    let edit = axio_tools::tools::fs::Edit::new();
    let plan = edit
        .plan(
            &json!({"path": "target.txt", "old": "original", "new": "replaced"}),
            &cx,
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&path).unwrap(), "original\n");
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
    // And the plan carries the diff a human would approve.
    assert!(matches!(
        plan.preview,
        Some(axio_core::protocol::Preview::Diff { .. })
    ));
}

#[tokio::test]
async fn an_ambiguous_edit_is_refused_rather_than_guessed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("dup.txt"), "same\nsame\n").unwrap();

    let cx = ToolCx {
        workspace: Arc::new(axio_core::tool::Workspace::new(&root).unwrap()),
        cancel: CancellationToken::new(),
        progress: axio_core::tool::ProgressSink::null(),
        limits: Default::default(),
        env: Arc::new(Default::default()),
    };

    let err = axio_tools::tools::fs::Edit::new()
        .plan(&json!({"path":"dup.txt","old":"same","new":"other"}), &cx)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("2 times"), "{err}");
}

/// Unix-only because it needs a shell command that reliably produces 200KB.
/// The capping and spilling themselves are covered cross-platform by the unit
/// tests in `axio_core::truncate`; this asserts the loop wires them up.
#[cfg(unix)]
#[tokio::test]
async fn a_large_output_is_capped_and_spilled_where_the_model_can_read_it() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "bash",
                r#"{"command":"head -c 200000 /dev/zero | tr '\\0' 'x'"}"#,
            )),
            done(),
        ],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("make noise".into(), CancellationToken::new())
        .await;

    let status = h
        .agent
        .session()
        .transcript()
        .iter()
        .find_map(|i| match &i.body {
            ItemBody::ToolCall { status, .. } => Some(status.clone()),
            _ => None,
        })
        .unwrap();

    match status {
        ToolStatus::Ok {
            output,
            truncated,
            spill,
            ..
        } => {
            assert!(truncated, "200KB must not go into the request whole");
            assert!(output.len() < 8_000, "capped to {} bytes", output.len());
            let spill = spill.expect("the full output must be kept");
            assert!(spill.exists());
            assert!(
                output.contains(&spill.display().to_string()),
                "the model must be told where the rest is"
            );
        }
        other => panic!("expected success, got {other:?}"),
    }
}

/// Regression. The spill path was built from a hardcoded `"output"` rather
/// than the call id, so a session's second large output silently overwrote the
/// first — and the first call's recorded marker went on naming a file that now
/// held someone else's bytes. Silent substitution in the one mechanism whose
/// entire job is not to lose output.
///
/// Unix-only for the same reason as its neighbour: it needs a shell command
/// that produces predictable bulk output.
#[cfg(unix)]
#[tokio::test]
async fn two_large_outputs_in_one_session_do_not_overwrite_each_other() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "call_a",
                "bash",
                r#"{"command":"head -c 60000 /dev/zero | tr '\\0' 'a'"}"#,
            )),
            turn_with(tool_call(
                0,
                "call_b",
                "bash",
                r#"{"command":"head -c 60000 /dev/zero | tr '\\0' 'b'"}"#,
            )),
            done(),
        ],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("make noise twice".into(), CancellationToken::new())
        .await;

    let spills: Vec<std::path::PathBuf> = h
        .agent
        .session()
        .transcript()
        .iter()
        .filter_map(|i| match &i.body {
            ItemBody::ToolCall {
                status: ToolStatus::Ok { spill, .. },
                ..
            } => spill.clone(),
            _ => None,
        })
        .collect();

    assert_eq!(spills.len(), 2, "both calls must have spilled");
    assert_ne!(spills[0], spills[1], "two calls shared one spill file");
    let first = std::fs::read_to_string(&spills[0]).expect("the first spill survived");
    let second = std::fs::read_to_string(&spills[1]).expect("the second spill exists");
    assert!(first.starts_with('a'), "the first spill was overwritten");
    assert!(second.starts_with('b'));
}

/// Unix-only: the assertion depends on how the shell renders an unset
/// variable. `proc::filter_env` is unit-tested on every platform.
#[cfg(unix)]
#[tokio::test]
async fn the_credential_is_absent_from_a_child_process() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "bash",
                r#"{"command":"echo key=[$ANTHROPIC_API_KEY]"}"#,
            )),
            done(),
        ],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("echo".into(), CancellationToken::new())
        .await;

    let results = last_results(&h.agent);
    assert!(
        results[0].1.contains("key=[]"),
        "the credential reached the child: {:?}",
        results[0]
    );
}

/// The hard gate. An interrupt must take the whole process tree, not just the
/// shell — otherwise a build the shell started outlives the turn.
#[cfg(unix)]
#[tokio::test]
async fn cancelling_leaves_no_orphan_process() {
    let marker = format!("axio-orphan-{}", std::process::id());
    let mut h = harness(
        vec![turn_with(tool_call(
            0,
            "t1",
            "bash",
            &format!(r#"{{"command":"sh -c 'sleep 30 # {marker}' & sleep 30"}}"#),
        ))],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        token.cancel();
    });

    let started = Instant::now();
    let outcome = h.agent.run_turn("sleep".into(), cancel).await;
    assert!(matches!(outcome, TurnOutcome::Interrupted));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "cancellation must not wait for the command"
    );

    tokio::time::sleep(Duration::from_millis(500)).await;
    let ps = std::process::Command::new("ps")
        .args(["-eo", "args"])
        .output()
        .unwrap();
    let listing = String::from_utf8_lossy(&ps.stdout);
    let survivors: Vec<&str> = listing.lines().filter(|l| l.contains(&marker)).collect();
    assert!(
        survivors.is_empty(),
        "orphaned processes survived cancellation: {survivors:?}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_cancelled_call_is_recorded_rather_than_left_pending() {
    let mut h = harness(
        vec![turn_with(tool_call(
            0,
            "t1",
            "bash",
            r#"{"command":"sleep 30"}"#,
        ))],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        token.cancel();
    });
    h.agent.run_turn("sleep".into(), cancel).await;

    // Every call still has a result, and the loop knows which.
    assert!(
        h.agent.session().unfinished_calls().is_empty(),
        "no call may be left pending"
    );
    let results = last_results(&h.agent);
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn approval_events_record_what_was_asked_and_what_was_decided() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "write",
                r#"{"path":"e.txt","content":"x"}"#,
            )),
            done(),
        ],
        Policy::new(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("write".into(), CancellationToken::new())
        .await;

    let events = drain(&mut h.rx);
    let requested = events.iter().find_map(|e| match &e.kind {
        EventKind::ApprovalRequested { request, .. } => Some(request.clone()),
        _ => None,
    });
    let resolved = events.iter().find_map(|e| match &e.kind {
        EventKind::ApprovalResolved { decision, .. } => Some(decision.clone()),
        _ => None,
    });

    let request = requested.expect("an approval must be observable");
    assert_eq!(request.tool, "write");
    assert_eq!(request.subject, "write:e.txt");
    assert!(request.effects.writes);
    assert!(!request.reason.is_empty(), "the reason is shown to a human");
    assert!(request.preview.is_some(), "a write is previewed as a diff");
    assert_eq!(resolved, Some(Decision::Allow));
}

#[tokio::test]
async fn a_path_outside_the_workspace_is_refused() {
    let mut h = harness(
        vec![
            turn_with(tool_call(
                0,
                "t1",
                "read",
                r#"{"path":"../../../etc/passwd"}"#,
            )),
            done(),
        ],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );
    h.agent
        .run_turn("read it".into(), CancellationToken::new())
        .await;

    let results = last_results(&h.agent);
    assert!(results[0].2);
    assert!(
        results[0].1.contains("escapes") || results[0].1.contains("relative"),
        "{:?}",
        results[0]
    );
}

/// The Windows half of the orphan gate.
///
/// There is no process group to signal, so the tree is walked with `taskkill`.
/// This asserts the effect rather than a process listing: the grandchild is
/// told to wait and then write a file, and the file must never appear. A
/// listing would have to be parsed, and a parse that goes wrong reports a pass.
#[cfg(windows)]
#[tokio::test]
async fn cancelling_leaves_no_orphan_process() {
    let marker = std::env::temp_dir().join(format!("axio-orphan-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&marker);

    // A grandchild: the shell starts it and then waits, so killing only the
    // shell leaves it running long enough to write the file.
    let command = format!(
        "Start-Process -NoNewWindow -FilePath powershell -ArgumentList \
         '-NoProfile','-Command','Start-Sleep 3; Set-Content -Path ''{}'' -Value done'; \
         Start-Sleep 30",
        marker.display()
    );
    let args = json!({ "command": command }).to_string();

    let mut h = harness(
        vec![turn_with(tool_call(0, "t1", "bash", &args))],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );

    let cancel = CancellationToken::new();
    let token = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        token.cancel();
    });

    let outcome = h.agent.run_turn("sleep".into(), cancel).await;
    assert!(matches!(outcome, TurnOutcome::Interrupted));

    // Longer than the grandchild's own wait, so "not yet" cannot pass for
    // "never".
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let survived = marker.exists();
    let _ = std::fs::remove_file(&marker);
    assert!(
        !survived,
        "an orphaned grandchild survived cancellation and did its work"
    );
}

/// A call the tool refuses to plan still has to say which tool refused it.
///
/// The regression: three things can end a call before it has a subject — no
/// such tool, arguments the schema rejects, and a `plan` that fails — and all
/// three left it empty, so the surface rendered `invalid arguments: …` against
/// a blank name. With six tools registered that is a guessing game. Found by
/// watching a real session produce four of them in a row, none identifiable.
#[tokio::test]
async fn a_call_that_fails_to_plan_still_names_its_tool() {
    let mut h = harness(
        // `grep` takes `pattern`; this asks for arguments it does not have.
        vec![turn_with(tool_call(
            0,
            "t1",
            "grep",
            r#"{"query":"needle","path":"src","max_results":10}"#,
        ))],
        Policy::new().unattended_allow(),
        Arc::new(CountingApprover {
            asked: Arc::new(AtomicUsize::new(0)),
            answer: Decision::Allow,
        }),
    );

    // The turn's own outcome is beside the point: the scripted provider has one
    // response, so what follows the tool result is not what is under test.
    let _ = h
        .agent
        .run_turn("search".into(), CancellationToken::new())
        .await;

    let named = drain(&mut h.rx).into_iter().any(|e| match e.kind {
        EventKind::ItemUpdated { item } | EventKind::ItemCompleted { item } => match item.body {
            ItemBody::ToolCall {
                subject, status, ..
            } => {
                subject.contains("grep")
                    && matches!(status, axio_core::protocol::ToolStatus::Failed { .. })
            }
            _ => false,
        },
        _ => false,
    });
    assert!(
        named,
        "a failed call reached the surface without naming its tool"
    );
}
