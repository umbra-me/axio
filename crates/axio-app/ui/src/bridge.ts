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

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { mockInvoke } from "./mock";

// `VITE_MOCK=1 npm run dev` swaps the Rust side for `mock.ts`, so every state
// the window has can be looked at without a provider. The check is a build-time
// constant, so a build made without the variable drops the mock entirely.
export const MOCK = import.meta.env.VITE_MOCK === "1";
const invoke: typeof tauriInvoke = MOCK ? mockInvoke : tauriInvoke;

/** Rust's events, or nothing at all under the mock — which then relies on the
 *  fallback timers every subscriber already has. */
export function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  if (MOCK) return Promise.resolve(() => {});
  return tauriListen<T>(event, handler);
}

export type { AppError } from "./generated/AppError";
export type { ApprovalView } from "./generated/ApprovalView";
export type { DecisionInput as Decision } from "./generated/DecisionInput";
export type { HostedOutput } from "./generated/HostedOutput";
export type { HostedView } from "./generated/HostedView";
export type { SlashCommand } from "./generated/SlashCommand";
export type { WindowState } from "./generated/WindowState";
export type { HostedApproval } from "./generated/HostedApproval";
export type { Isolation } from "./generated/Isolation";
export type { PreviewView } from "./generated/PreviewView";
export type { ProjectView } from "./generated/ProjectView";
export type { SessionStatus } from "./generated/SessionStatus";
export type { SessionView } from "./generated/SessionView";
export type { Snapshot } from "./generated/Snapshot";
export type { StartHostedInput } from "./generated/StartHostedInput";
export type { StartSessionInput } from "./generated/StartSessionInput";
export type { TranscriptEntry } from "./generated/TranscriptEntry";
export type { TranscriptView } from "./generated/TranscriptView";
export type { AppSettings } from "./generated/AppSettings";
export type { Appearance } from "./generated/Appearance";
export type { TerminalSettings } from "./generated/TerminalSettings";
export type { AgentSettings } from "./generated/AgentSettings";
export type { ModelDefault } from "./generated/ModelDefault";
export type { SettingsView } from "./generated/SettingsView";
export type { StartGroupInput } from "./generated/StartGroupInput";
export type { GroupStart } from "./generated/GroupStart";
export type { AddToGroupInput } from "./generated/AddToGroupInput";
export type { ProviderView } from "./generated/ProviderView";
export type { LandingView } from "./generated/LandingView";
export type { LandAction } from "./generated/LandAction";
export type { LandOutcome } from "./generated/LandOutcome";

import type { ApprovalView } from "./generated/ApprovalView";
import type { DecisionInput } from "./generated/DecisionInput";
import type { HostedOutput } from "./generated/HostedOutput";
import type { HostedView } from "./generated/HostedView";
import type { SlashCommand } from "./generated/SlashCommand";
import type { WindowState } from "./generated/WindowState";
import type { HostedApproval } from "./generated/HostedApproval";
import type { ProjectView } from "./generated/ProjectView";
import type { SessionView } from "./generated/SessionView";
import type { Snapshot } from "./generated/Snapshot";
import type { StartSessionInput } from "./generated/StartSessionInput";
import type { TranscriptView } from "./generated/TranscriptView";
import type { AppSettings } from "./generated/AppSettings";
import type { Isolation } from "./generated/Isolation";
import type { ModelDefault } from "./generated/ModelDefault";
import type { SettingsView } from "./generated/SettingsView";
import type { StartGroupInput } from "./generated/StartGroupInput";
import type { GroupStart } from "./generated/GroupStart";
import type { AddToGroupInput } from "./generated/AddToGroupInput";
import type { ProviderView } from "./generated/ProviderView";
import type { LandingView } from "./generated/LandingView";
import type { LandAction } from "./generated/LandAction";
import type { LandOutcome } from "./generated/LandOutcome";

export const api = {
  snapshot: () => invoke<Snapshot>("snapshot"),
  approvals: () => invoke<ApprovalView[]>("approvals"),
  startSession: (input: StartSessionInput) => invoke<SessionView>("start_session", { input }),
  sendPrompt: (sessionId: string, prompt: string) =>
    invoke<void>("send_prompt", { sessionId, prompt }),
  sessionDiff: (sessionId: string) => invoke<string>("session_diff", { sessionId }),
  sessionTranscript: (sessionId: string) =>
    invoke<TranscriptView>("session_transcript", { sessionId }),
  openProject: (path: string) => invoke<ProjectView>("open_project", { path }),
  // The native folder picker, opened by Rust. `null` means it was dismissed.
  addRepository: () => invoke<ProjectView | null>("add_repository"),
  cancelSession: (sessionId: string) => invoke<void>("cancel_session", { sessionId }),
  /** Turns with a checkpoint either side, and what one of them changed. */
  sessionTurns: (sessionId: string) => invoke<number[]>("session_turns", { sessionId }),
  sessionTurnDiff: (sessionId: string, turn: number) => invoke<string>("session_turn_diff", { sessionId, turn }),
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
    isolation: Isolation | null = null,
    args = "",
    size: { rows: number; cols: number } | null = null,
  ) =>
    invoke<HostedView>("hosted_start", {
      input: { harness, cwd, isolation, args, rows: size?.rows ?? null, cols: size?.cols ?? null },
    }),
  hostedRead: (id: string, from: number) => invoke<HostedOutput>("hosted_read", { id, from }),
  hostedWrite: (id: string, data: string, submit: boolean) =>
    invoke<void>("hosted_write", { id, data, submit }),
  hostedResize: (id: string, rows: number, cols: number) =>
    invoke<void>("hosted_resize", { id, rows, cols }),
  /** How the window was looking at things when it last saved, and saving it. */
  windowState: () => invoke<WindowState>("window_state"),
  saveWindowState: (window: WindowState) => invoke<void>("save_window_state", { window }),
  /** Type a line and submit it, paced so the agent treats it as typed. */
  hostedSubmit: (id: string, text: string) => invoke<void>("hosted_submit", { id, text }),
  /** A structured agent's questions, and answering one. */
  hostedApprovals: (id: string) => invoke<HostedApproval[]>("hosted_approvals", { id }),
  hostedDecide: (id: string, approval: string, decision: string) => invoke<void>("hosted_decide", { id, approval, decision }),
  /** The agent's own transcript, from the file its hooks named. */
  hostedTranscript: (id: string) => invoke<TranscriptView>("hosted_transcript", { id }),
  /** The slash commands the terminal's agent answers to. */
  hostedCommands: (id: string) => invoke<SlashCommand[]>("hosted_commands", { id }),
  /** Stop and forget. The worktree stays. */
  hostedKill: (id: string) => invoke<void>("hosted_kill", { id }),
  /** Stop and keep the row, ended, for a resume. */
  hostedStop: (id: string) => invoke<void>("hosted_stop", { id }),
  /** Start an ended or remembered terminal again in its own directory. */
  hostedResume: (id: string, size: { rows: number; cols: number } | null = null) =>
    invoke<HostedView>("hosted_resume", { id, rows: size?.rows ?? null, cols: size?.cols ?? null }),

  // Groups, names, landing, and the machine's own facts.
  startGroup: (input: StartGroupInput) => invoke<GroupStart>("start_group", { input }),
  addToGroup: (input: AddToGroupInput) => invoke<GroupStart>("add_to_group", { input }),
  renameSession: (sessionId: string, title: string | null) =>
    invoke<void>("rename_session", { sessionId, title }),
  renameHosted: (id: string, title: string | null) => invoke<HostedView>("rename_hosted", { id, title }),
  hostedDiff: (id: string) => invoke<string>("hosted_diff", { id }),
  landing: (sessionId: string) => invoke<LandingView>("landing", { sessionId }),
  land: (sessionId: string, action: LandAction) => invoke<LandOutcome>("land", { sessionId, action }),
  providers: () => invoke<ProviderView[]>("providers"),
  reveal: (path: string, editor: boolean) => invoke<void>("reveal", { path, editor }),

  // Settings: the window's own file, and the default model in axio's.
  settings: () => invoke<SettingsView>("settings"),
  saveSettings: (settings: AppSettings) => invoke<SettingsView>("save_settings", { settings }),
  setDefaultModel: (model: ModelDefault) => invoke<SettingsView>("set_default_model", { model }),
};

// What went wrong, as words.
//
// A command's error arrives as the tagged `AppError` object — `{kind,
// message}` — and `String()` of an object is "[object Object]", which is what
// the window showed for its first real failure. The message is the part a
// person reads; the kind is for code that wants to branch.
export function describe(e: unknown): string {
  if (e && typeof e === "object" && "message" in e && typeof e.message === "string") {
    return e.message;
  }
  return String(e);
}

// Which colour identifies a supervised session.
//
// Hosted sessions carry their harness's own variable, chosen in Rust beside the
// harness list. This one is for axio's own sessions, where isolation is the
// only thing that distinguishes them.
export function accentFor(session: SessionView): string {
  return session.isolation === "direct" ? "var(--agent-pi)" : "var(--agent-axio)";
}
