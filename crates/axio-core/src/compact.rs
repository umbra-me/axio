//! Deterministic context compaction.
//!
//! Two staged elisions, both pure functions of the transcript, plus a
//! force-only prefix drop. Pure matters: it is what makes a resumed session
//! re-derive exactly the elisions the original run made, rather than drifting a
//! little further on every resume.
//!
//! Nothing here writes to the session file. The file records what happened;
//! compaction decides what to send.

use std::collections::BTreeMap;

use crate::protocol::{Item, ItemBody, ItemId, ToolStatus};

/// Bytes per token. Deliberately crude and model-agnostic: it is only ever used
/// to compare against a threshold, and a real tokeniser in the core crate would
/// cost a dependency to make a rounding error smaller.
const BYTES_PER_TOKEN: u64 = 4;

/// Fire stage one at this fraction of the window.
const STAGE_ONE_AT: f64 = 0.55;
/// Fire stage two at this fraction.
const STAGE_TWO_AT: f64 = 0.70;

/// Tool outputs shorter than this are not worth eliding: the marker would cost
/// more than the content.
const MIN_ELIDE_BYTES: usize = 200;

/// How much room the transcript is allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextBudget {
    pub window_tokens: u64,
}

impl Default for ContextBudget {
    fn default() -> Self {
        Self {
            window_tokens: 1_000_000,
        }
    }
}

/// What to leave out of the next request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Elisions {
    /// Tool results whose output is replaced by a marker.
    pub replaced: BTreeMap<ItemId, String>,
    /// How many leading items to drop. Never touches index 0.
    pub dropped_prefix: usize,
    /// The highest stage that fired, for reporting.
    pub stage: u8,
}

impl Elisions {
    pub fn is_empty(&self) -> bool {
        self.replaced.is_empty() && self.dropped_prefix == 0
    }
}

/// Rough token count of a transcript.
pub fn estimate_tokens(items: &[Item]) -> u64 {
    items
        .iter()
        .map(|i| {
            serde_json::to_string(i)
                .map(|s| s.len() as u64)
                .unwrap_or(0)
        })
        .sum::<u64>()
        / BYTES_PER_TOKEN
}

/// Decide what to elide, given how full the window is.
///
/// Deterministic: the same items and the same budget always produce the same
/// plan, which is what lets a resumed session reproduce the original request.
pub fn plan(items: &[Item], budget: ContextBudget) -> Elisions {
    let mut elisions = Elisions::default();
    let used = estimate_tokens(items);
    let window = budget.window_tokens.max(1) as f64;
    let fraction = used as f64 / window;

    if fraction >= STAGE_ONE_AT {
        elisions.stage = 1;
        for id in stale_reads(items) {
            elisions.replaced.insert(
                id,
                "[this file was read again later; the newer copy is below]".to_owned(),
            );
        }
    }

    if fraction >= STAGE_TWO_AT {
        elisions.stage = 2;
        for (id, bytes) in large_outputs(items) {
            elisions
                .replaced
                .entry(id)
                .or_insert_with(|| format!("[{bytes} bytes of output elided to fit the context]"));
        }
    }

    elisions
}

/// Force a prefix drop when the staged elisions were not enough.
///
/// Only reachable from an overflow, never from a threshold, because it is the
/// one stage that removes history outright.
pub fn force(items: &[Item], previous: &Elisions) -> Option<Elisions> {
    let mut elisions = previous.clone();

    // First: everything stage two would elide, regardless of thresholds.
    let mut changed = false;
    for (id, bytes) in large_outputs(items) {
        if elisions
            .replaced
            .insert(
                id,
                format!("[{bytes} bytes of output elided to fit the context]"),
            )
            .is_none()
        {
            changed = true;
        }
    }
    if changed {
        elisions.stage = 2;
        return Some(elisions);
    }

    // Then: drop a prefix.
    let cut = prefix_cut(items, elisions.dropped_prefix);
    if cut > elisions.dropped_prefix {
        elisions.dropped_prefix = cut;
        elisions.stage = 3;
        return Some(elisions);
    }

    None
}

/// Read results superseded by a later read or edit of the same path.
fn stale_reads(items: &[Item]) -> Vec<ItemId> {
    let mut later: Vec<&str> = Vec::new();
    let mut stale = Vec::new();

    // Walk backwards: a path seen later supersedes the same path seen earlier.
    for item in items.iter().rev() {
        let ItemBody::ToolCall {
            name,
            input,
            status,
            ..
        } = &item.body
        else {
            continue;
        };
        let Some(path) = input.get("path").and_then(|p| p.as_str()) else {
            continue;
        };

        let is_read = name == "read";
        let touches = is_read || name == "write" || name == "edit";
        if !touches {
            continue;
        }

        // Only a completed read carries content worth eliding, and only an
        // `Ok` one — a `Denied` or `Cancelled` result is already short, and
        // rewriting it would lose why it was refused.
        if is_read
            && matches!(status, ToolStatus::Ok { output, .. } if output.len() >= MIN_ELIDE_BYTES)
            && later.contains(&path)
        {
            stale.push(item.id);
        }
        later.push(path);
    }
    stale.reverse();
    stale
}

/// Completed tool outputs large enough to be worth replacing.
fn large_outputs(items: &[Item]) -> Vec<(ItemId, usize)> {
    items
        .iter()
        .filter_map(|item| match &item.body {
            ItemBody::ToolCall {
                status: ToolStatus::Ok { output, .. },
                ..
            } if output.len() >= MIN_ELIDE_BYTES => Some((item.id, output.len())),
            _ => None,
        })
        .collect()
}

/// Where a prefix drop may cut to.
///
/// Two clamps, each protecting something whose loss is undetectable from the
/// request itself:
///
/// * never index 0 — the opening prompt is the task;
/// * never past the most recent `UserMessage` — losing it leaves a valid
///   request about the wrong question, at full price.
///
/// There is deliberately no third clamp for tool pairings. One `ToolCall` item
/// emits *both* the `tool_use` and its `tool_result`, so dropping items can
/// never orphan half a pair — which is exactly why compaction removes whole
/// items rather than editing the wire projection.
fn prefix_cut(items: &[Item], from: usize) -> usize {
    if items.len() <= 2 {
        return from;
    }

    // When the only user message is the opening prompt, everything after it is
    // fair game — index 0 is preserved by `apply` regardless. Clamping to it
    // would block stage three entirely for a long single turn, which is the
    // case it exists for.
    let ceiling = match last_user_message(items) {
        Some(0) | None => items.len(),
        Some(i) => i,
    };
    // Advance by a quarter of what is left, so repeated forces converge rather
    // than dropping everything at once.
    let step = ((items.len() - from) / 4).max(1);
    let cut = (from + step).min(ceiling);

    if cut <= from || cut >= items.len() {
        return from;
    }
    cut.max(1)
}

fn last_user_message(items: &[Item]) -> Option<usize> {
    items
        .iter()
        .rposition(|i| matches!(i.body, ItemBody::UserMessage { .. }))
}

/// Apply a plan, producing the item list the request is built from.
///
/// Index 0 always survives, and a dropped prefix is replaced by one marker so
/// the model sees a hole rather than a silently shortened history.
pub fn apply(items: &[Item], elisions: &Elisions) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());

    if elisions.dropped_prefix > 0 && !items.is_empty() {
        out.push(items[0].clone());
        let dropped = elisions.dropped_prefix.saturating_sub(1);
        if dropped > 0 {
            out.push(Item::new(ItemBody::ContextElision {
                dropped_items: dropped as u32,
            }));
        }
    }

    let start = if elisions.dropped_prefix > 0 {
        elisions.dropped_prefix.max(1)
    } else {
        0
    };

    for item in &items[start.min(items.len())..] {
        let mut item = item.clone();
        if let Some(marker) = elisions.replaced.get(&item.id)
            && let ItemBody::ToolCall { status, .. } = &mut item.body
            && let ToolStatus::Ok {
                output, truncated, ..
            } = status
        {
            *output = marker.clone();
            *truncated = true;
        }
        out.push(item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(text: &str) -> Item {
        Item::new(ItemBody::UserMessage { text: text.into() })
    }

    fn read_call(path: &str, output: &str) -> Item {
        Item::new(ItemBody::ToolCall {
            call_id: format!("toolu_{path}"),
            name: "read".into(),
            input: json!({ "path": path }),
            subject: format!("read:{path}"),
            preview: None,
            status: ToolStatus::Ok {
                output: output.into(),
                truncated: false,
                spill: None,
                ms: 1,
            },
        })
    }

    fn edit_call(path: &str) -> Item {
        Item::new(ItemBody::ToolCall {
            call_id: format!("edit_{path}"),
            name: "edit".into(),
            input: json!({ "path": path }),
            subject: format!("edit:{path}"),
            preview: None,
            status: ToolStatus::Ok {
                output: "edited".into(),
                truncated: false,
                spill: None,
                ms: 1,
            },
        })
    }

    fn big(n: usize) -> String {
        "x".repeat(n)
    }

    /// Small enough that nothing fires.
    fn roomy() -> ContextBudget {
        ContextBudget {
            window_tokens: 1_000_000,
        }
    }

    /// Tight enough that everything fires.
    fn tight() -> ContextBudget {
        ContextBudget { window_tokens: 1 }
    }

    #[test]
    fn nothing_is_elided_below_the_first_threshold() {
        let items = vec![user("hi"), read_call("a.rs", &big(5_000))];
        assert!(plan(&items, roomy()).is_empty());
    }

    #[test]
    fn stage_one_elides_a_read_superseded_by_a_later_read() {
        let items = vec![
            user("look at a.rs"),
            read_call("a.rs", &big(1_000)),
            read_call("a.rs", &big(1_000)),
        ];
        let e = plan(&items, tight());
        assert!(
            e.replaced.contains_key(&items[1].id),
            "the earlier read is stale"
        );
        assert!(
            !e.replaced
                .get(&items[2].id)
                .is_some_and(|m| m.contains("read again")),
            "the newest read must survive stage one"
        );
    }

    #[test]
    fn stage_one_elides_a_read_superseded_by_an_edit() {
        let items = vec![
            user("fix a.rs"),
            read_call("a.rs", &big(1_000)),
            edit_call("a.rs"),
        ];
        let e = plan(&items, tight());
        assert!(e.replaced.contains_key(&items[1].id));
    }

    #[test]
    fn a_read_of_a_different_path_is_not_stale() {
        let items = vec![
            user("look"),
            read_call("a.rs", &big(1_000)),
            read_call("b.rs", &big(1_000)),
        ];
        let stale = stale_reads(&items);
        assert!(stale.is_empty(), "different paths do not supersede");
    }

    #[test]
    fn a_short_output_is_never_worth_eliding() {
        let items = vec![
            user("hi"),
            read_call("a.rs", "tiny"),
            read_call("a.rs", "tiny"),
        ];
        let e = plan(&items, tight());
        assert!(
            e.replaced.is_empty(),
            "the marker would cost more than the content"
        );
    }

    #[test]
    fn applying_a_plan_replaces_the_output_and_marks_it_truncated() {
        let items = vec![
            user("hi"),
            read_call("a.rs", &big(1_000)),
            read_call("a.rs", &big(1_000)),
        ];
        let e = plan(&items, tight());
        let out = apply(&items, &e);
        match &out[1].body {
            ItemBody::ToolCall {
                status: ToolStatus::Ok {
                    output, truncated, ..
                },
                ..
            } => {
                assert!(output.contains("read again"));
                assert!(truncated);
            }
            other => panic!("expected an elided read, got {other:?}"),
        }
    }

    #[test]
    fn compaction_is_deterministic() {
        // The property the whole design rests on: same items, same plan, so a
        // resumed session reproduces the original request rather than drifting.
        let items = vec![
            user("go"),
            read_call("a.rs", &big(1_000)),
            read_call("a.rs", &big(1_000)),
            read_call("b.rs", &big(900)),
        ];
        let first = plan(&items, tight());
        for _ in 0..50 {
            assert_eq!(plan(&items, tight()), first);
        }
    }

    #[test]
    fn index_zero_always_survives() {
        let items: Vec<Item> = std::iter::once(user("the original task"))
            .chain((0..20).map(|n| read_call(&format!("f{n}.rs"), &big(500))))
            .collect();
        let mut e = plan(&items, tight());
        while let Some(next) = force(&items, &e) {
            e = next;
            if e.dropped_prefix > 0 {
                break;
            }
        }
        let out = apply(&items, &e);
        assert_eq!(
            serde_json::to_string(&out[0]).unwrap(),
            serde_json::to_string(&items[0]).unwrap(),
            "the opening prompt is the task and must never be dropped"
        );
    }

    #[test]
    fn a_prefix_drop_never_passes_the_most_recent_user_message() {
        // Losing the current prompt leaves a request that is valid and about
        // the wrong question — the one loss that cannot be detected from the
        // request itself.
        let mut items = vec![user("first task")];
        items.extend((0..30).map(|n| read_call(&format!("f{n}.rs"), &big(400))));
        items.push(user("the question I actually asked"));
        let last_user = items.len() - 1;

        let mut e = Elisions::default();
        for _ in 0..20 {
            match force(&items, &e) {
                Some(next) => e = next,
                None => break,
            }
        }
        assert!(
            e.dropped_prefix <= last_user,
            "the current prompt was dropped: cut {} past {last_user}",
            e.dropped_prefix
        );
    }

    #[test]
    fn a_prefix_drop_leaves_a_marker_so_the_hole_is_visible() {
        let items: Vec<Item> = std::iter::once(user("task"))
            .chain((0..20).map(|n| read_call(&format!("f{n}.rs"), &big(400))))
            .collect();
        let mut e = Elisions::default();
        while let Some(next) = force(&items, &e) {
            e = next;
            if e.dropped_prefix > 1 {
                break;
            }
        }
        let out = apply(&items, &e);
        assert!(
            out.iter()
                .any(|i| matches!(i.body, ItemBody::ContextElision { .. })),
            "a shortened history must show the hole rather than lie about it"
        );
    }

    #[test]
    fn force_eventually_gives_up_rather_than_looping() {
        // If nothing is left to elide, the caller has to be told so it can fail
        // with an explicit message instead of retrying forever.
        let items = vec![user("only this")];
        let mut e = Elisions::default();
        let mut rounds = 0;
        while let Some(next) = force(&items, &e) {
            e = next;
            rounds += 1;
            assert!(rounds < 100, "force must terminate");
        }
        assert!(e.is_empty());
    }

    #[test]
    fn force_elides_large_outputs_before_dropping_history() {
        let items = vec![user("go"), read_call("a.rs", &big(5_000))];
        let e = force(&items, &Elisions::default()).expect("something to elide");
        assert_eq!(e.stage, 2);
        assert_eq!(e.dropped_prefix, 0, "content first, history last");
    }

    #[test]
    fn an_estimate_is_proportional_to_size() {
        let small = vec![user("hi")];
        let large = vec![user(&big(40_000))];
        assert!(estimate_tokens(&large) > estimate_tokens(&small) * 100);
    }
}
