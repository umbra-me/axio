// The typed edge of the Rust boundary.
//
// Every shape here is *generated* from `crates/axio-app/src/model.rs` and
// `hosted.rs` by ts-rs, emitted into `generated/` when the crate's tests run.
// It used to be hand-mirrored, which is how two sides of a boundary drift: the
// prior art this design is drawn from warns about exactly that in four separate
// documents and drifted anyway. Generation caught one here on its first run —
// a `u64` this file declared as `number` and the derive read as `bigint`, which
// is neither what the field means nor what JSON IPC delivers.
//
// What is still written by hand is this file's *invoke wrappers*. Their names
// and argument shapes are checked by Tauri at runtime rather than at compile
// time, so a rename shows up as a failed command rather than a build error.
// Generating those too is what `tauri-specta` would add.

import { invoke } from "@tauri-apps/api/core";

export type { AppError } from "./generated/AppError";
export type { ApprovalView } from "./generated/ApprovalView";
export type { DecisionInput as Decision } from "./generated/DecisionInput";
export type { HostedOutput } from "./generated/HostedOutput";
export type { HostedView } from "./generated/HostedView";
export type { Isolation } from "./generated/Isolation";
export type { PreviewView } from "./generated/PreviewView";
export type { ProjectView } from "./generated/ProjectView";
export type { SessionStatus } from "./generated/SessionStatus";
export type { SessionView } from "./generated/SessionView";
export type { Snapshot } from "./generated/Snapshot";
export type { StartHostedInput } from "./generated/StartHostedInput";
export type { StartSessionInput } from "./generated/StartSessionInput";

import type { ApprovalView } from "./generated/ApprovalView";
import type { DecisionInput } from "./generated/DecisionInput";
import type { HostedOutput } from "./generated/HostedOutput";
import type { HostedView } from "./generated/HostedView";
import type { SessionView } from "./generated/SessionView";
import type { Snapshot } from "./generated/Snapshot";
import type { StartSessionInput } from "./generated/StartSessionInput";

export const api = {
  snapshot: () => invoke<Snapshot>("snapshot"),
  approvals: () => invoke<ApprovalView[]>("approvals"),
  startSession: (input: StartSessionInput) => invoke<SessionView>("start_session", { input }),
  sendPrompt: (sessionId: string, prompt: string) =>
    invoke<void>("send_prompt", { sessionId, prompt }),
  sessionDiff: (sessionId: string) => invoke<string>("session_diff", { sessionId }),
  cancelSession: (sessionId: string) => invoke<void>("cancel_session", { sessionId }),
  closeSession: (sessionId: string, discard: boolean) =>
    invoke<void>("close_session", { sessionId, discard }),
  resolveApproval: (approvalId: string, decision: DecisionInput) =>
    invoke<boolean>("resolve_approval", { approvalId, decision }),
  windowControl: (action: "minimize" | "toggle-maximize" | "close" | "destroy") =>
    invoke<void>("window_control", { action }),

  // Hosted agents: Claude Code, Codex or Pi in a terminal axio owns.
  hostedAvailable: () => invoke<HostedView[]>("hosted_available"),
  hostedList: () => invoke<HostedView[]>("hosted_list"),
  hostedStart: (
    harness: string,
    cwd: string,
    args = "",
    size: { rows: number; cols: number } | null = null,
  ) =>
    invoke<HostedView>("hosted_start", {
      input: { harness, cwd, args, rows: size?.rows ?? null, cols: size?.cols ?? null },
    }),
  hostedRead: (id: string, from: number) => invoke<HostedOutput>("hosted_read", { id, from }),
  hostedWrite: (id: string, data: string, submit: boolean) =>
    invoke<void>("hosted_write", { id, data, submit }),
  hostedResize: (id: string, rows: number, cols: number) =>
    invoke<void>("hosted_resize", { id, rows, cols }),
  hostedKill: (id: string) => invoke<void>("hosted_kill", { id }),
};

// Which colour identifies a supervised session.
//
// Hosted sessions carry their harness's own variable, chosen in Rust beside the
// harness list. This one is for axio's own sessions, where isolation is the
// only thing that distinguishes them.
export function accentFor(session: SessionView): string {
  return session.isolation === "direct" ? "var(--agent-pi)" : "var(--agent-axio)";
}
