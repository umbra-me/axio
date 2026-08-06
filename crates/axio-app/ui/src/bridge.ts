// The typed edge of the Rust boundary.
//
// Hand-written for now, and that is a known debt rather than a design: the
// shapes here mirror `crates/axio-app/src/model.rs`, and mirroring is how the
// two drift. The prior art this is drawn from warns about exactly that in four
// separate documents and has drifted anyway - one field is a union on this side
// and an unvalidated string on the other. The fix is generation
// (`tauri-specta`), and it is the next change to this file, not a later one.

import { invoke } from "@tauri-apps/api/core";

export type Isolation = "worktree" | "direct";
export type SessionStatus = "idle" | "running" | "closed";

export interface ProjectView {
  id: string;
  name: string;
  root: string;
  openSessions: number;
  totalSessions: number;
}

export interface SessionView {
  id: string;
  shortId: string;
  projectId: string;
  projectName: string;
  label: string | null;
  branch: string | null;
  workspace: string;
  isolation: Isolation;
  status: SessionStatus;
  startedMs: number;
}

export type PreviewView =
  | { kind: "diff"; path: string; unified: string; added: number; removed: number }
  | { kind: "command"; program: string; raw: string; cwd: string }
  | { kind: "text"; text: string };

export interface ApprovalView {
  id: string;
  sessionId: string;
  shortSessionId: string;
  projectId: string;
  subject: string;
  tool: string;
  reason: string;
  preview: PreviewView | null;
  atMs: number;
}

export interface Snapshot {
  projects: ProjectView[];
  sessions: SessionView[];
  approvals: ApprovalView[];
  unavailable: string | null;
}

export type Decision =
  | { decision: "allow" }
  | { decision: "allowSession" }
  | { decision: "deny"; feedback: string | null };

export interface StartSessionInput {
  path: string;
  prompt: string | null;
  isolation: Isolation | null;
}

export interface HostedView {
  id: string;
  harness: string;
  label: string;
  accentVar: string;
  cwd: string;
  status: string;
  exitCode: number | null;
}

export interface HostedOutput {
  text: string;
  cursor: number;
}

export const api = {
  hostedAvailable: () => invoke<HostedView[]>("hosted_available"),
  hostedList: () => invoke<HostedView[]>("hosted_list"),
  hostedStart: (harness: string, cwd: string, args = "") =>
    invoke<HostedView>("hosted_start", { input: { harness, cwd, args } }),
  hostedRead: (id: string, from: number) => invoke<HostedOutput>("hosted_read", { id, from }),
  hostedWrite: (id: string, data: string, submit: boolean) =>
    invoke<void>("hosted_write", { id, data, submit }),
  hostedResize: (id: string, rows: number, cols: number) =>
    invoke<void>("hosted_resize", { id, rows, cols }),
  hostedKill: (id: string) => invoke<void>("hosted_kill", { id }),
  snapshot: () => invoke<Snapshot>("snapshot"),
  startSession: (input: StartSessionInput) => invoke<SessionView>("start_session", { input }),
  sendPrompt: (sessionId: string, prompt: string) =>
    invoke<void>("send_prompt", { sessionId, prompt }),
  approvals: () => invoke<ApprovalView[]>("approvals"),
  sessionDiff: (sessionId: string) => invoke<string>("session_diff", { sessionId }),
  cancelSession: (sessionId: string) => invoke<void>("cancel_session", { sessionId }),
  closeSession: (sessionId: string, discard: boolean) =>
    invoke<void>("close_session", { sessionId, discard }),
  resolveApproval: (approvalId: string, decision: Decision) =>
    invoke<boolean>("resolve_approval", { approvalId, decision }),
  windowControl: (action: "minimize" | "toggle-maximize" | "close" | "destroy") =>
    invoke<void>("window_control", { action }),
};

// Which colour identifies a session's agent.
//
// Keyed off isolation for now because every session this shell can see is one
// axio started. When foreign harnesses arrive - Claude Code, Codex, Pi, each in
// its own PTY - this becomes a lookup on the harness id, and the CSS already
// reads whatever it is told through `--agent-accent`.
export function accentFor(session: SessionView): string {
  return session.isolation === "direct" ? "var(--agent-pi)" : "var(--agent-axio)";
}
