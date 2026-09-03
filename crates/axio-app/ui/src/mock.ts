// A stand-in for the Rust side, for looking at the interface.
//
// Every state the window can be in — a session streaming, one waiting on an
// approval, one finished while nobody watched, a closed one, a hosted
// terminal — needs a real provider, a real repository and real money to reach
// otherwise, and a design pass that can only see the empty state is a design
// pass done blind. This module answers every `invoke` the bridge makes from
// memory, and advances a little on each read so the streaming caret, the
// working dot and the unread mark all have something to do.
//
// It is reached only when the frontend is started with `VITE_MOCK=1`, and the
// bridge selects it at build time, so a release build carries none of it.
// Nothing here is a second implementation of anything: the shapes are the
// generated ones, and the commands are the bridge's own names.

import type { ApprovalView } from "./generated/ApprovalView";
import type { DecisionInput } from "./generated/DecisionInput";
import type { HostedOutput } from "./generated/HostedOutput";
import type { HostedView } from "./generated/HostedView";
import type { ProjectView } from "./generated/ProjectView";
import type { SessionView } from "./generated/SessionView";
import type { Snapshot } from "./generated/Snapshot";
import type { StartSessionInput } from "./generated/StartSessionInput";
import type { TranscriptEntry } from "./generated/TranscriptEntry";
import type { TranscriptView } from "./generated/TranscriptView";
import type { AppSettings } from "./generated/AppSettings";
import type { ModelDefault } from "./generated/ModelDefault";
import type { SettingsView } from "./generated/SettingsView";
import type { StartGroupInput } from "./generated/StartGroupInput";
import type { LandAction } from "./generated/LandAction";

const now = Date.now();

const projects: ProjectView[] = [
  { id: "p-axio", name: "axio", root: "/Users/me/src/axio", openSessions: 3, totalSessions: 5 },
  { id: "p-umbra", name: "umbra", root: "/Users/me/src/umbra", openSessions: 1, totalSessions: 1 },
];

const sessions: SessionView[] = [
  {
    id: "s-1111aaaa2222bbbb",
    shortId: "1111aaaa",
    projectId: "p-axio",
    projectName: "axio",
    label: "Merge the tab strip into the session bar",
    title: null,
    group: null,
    branch: "axio/1111aaaa",
    workspace: "/Users/me/src/axio/.axio/worktrees/1111aaaa",
    isolation: "worktree",
    status: "running",
    open: true,
    startedMs: now - 4 * 60_000,
  },
  {
    id: "s-3333cccc4444dddd",
    shortId: "3333cccc",
    projectId: "p-axio",
    projectName: "axio",
    label: "Add a --json flag to axio quota",
    title: "quota --json",
    group: null,
    branch: "axio/3333cccc",
    workspace: "/Users/me/src/axio/.axio/worktrees/3333cccc",
    isolation: "worktree",
    status: "idle",
    open: true,
    startedMs: now - 40 * 60_000,
  },
  {
    id: "s-5555eeee6666ffff",
    shortId: "5555eeee",
    projectId: "p-axio",
    projectName: "axio",
    label: "Explain the compaction heuristic",
    title: null,
    group: null,
    branch: null,
    workspace: "/Users/me/src/axio",
    isolation: "direct",
    status: "idle",
    open: true,
    startedMs: now - 2 * 3_600_000,
  },
  {
    id: "s-7777gggg8888hhhh",
    shortId: "7777gggg",
    projectId: "p-axio",
    projectName: "axio",
    label: "Rename the workspace lock reader",
    title: null,
    group: null,
    branch: "axio/7777gggg",
    workspace: "/Users/me/src/axio/.axio/worktrees/7777gggg",
    isolation: "worktree",
    status: "closed",
    open: false,
    startedMs: now - 26 * 3_600_000,
  },
  {
    id: "s-9999iiii0000jjjj",
    shortId: "9999iiii",
    projectId: "p-umbra",
    projectName: "umbra",
    label: "Check the port registry against the compose files",
    title: null,
    group: null,
    branch: "axio/9999iiii",
    workspace: "/Users/me/src/umbra/.axio/worktrees/9999iiii",
    isolation: "worktree",
    status: "idle",
    open: true,
    startedMs: now - 15 * 60_000,
  },
];

for (const [n, status] of [["a", "running"], ["b", "idle"]] as const) {
  sessions.push({
    id: `s-group${n}`,
    shortId: `group${n}00`,
    projectId: "p-umbra",
    projectName: "umbra",
    label: "Make check-port-docs.py report every collision, not the first",
    title: null,
    group: "g-01k4grp",
    branch: `axio/01k4grp${n}`,
    workspace: `/Users/me/src/umbra/.axio/worktrees/01k4grp${n}`,
    isolation: "worktree",
    status,
    open: true,
    startedMs: now - 9 * 60_000,
  });
}

const DIFF = `--- a/crates/axio-app/ui/src/App.tsx
+++ b/crates/axio-app/ui/src/App.tsx
@@ -12,7 +12,6 @@ import { Rail } from "./Rail";
 import { SessionPane } from "./SessionPane";
-import { Tabs, sameTab, type Tab } from "./Tabs";
+import { sameTab, type Tab } from "./Tabs";
 import { HostedTerminal, paneSize } from "./Terminal";
@@ -300,9 +299,6 @@ export default function App() {
         <main className="main">
-          <Tabs
-            tabs={tabs}
-            active={active}
           <div className="pane">`;

const approvals: ApprovalView[] = [
  {
    id: "a-1",
    sessionId: "s-9999iiii0000jjjj",
    shortSessionId: "9999iiii",
    projectId: "p-umbra",
    subject: "shell:python3 scripts/check-port-docs.py",
    tool: "shell",
    reason: "runs a program outside the allow list",
    preview: {
      kind: "command",
      program: "python3",
      raw: "python3 scripts/check-port-docs.py",
      cwd: "/Users/me/src/umbra/.axio/worktrees/9999iiii",
    },
    atMs: now - 20_000,
  },
];

const hostedAvailable: HostedView[] = [
  { id: "", harness: "axio", label: "axio", name: "axio", branch: null, group: null, accentVar: "--agent-axio", cwd: "", status: "available", exitCode: null },
  { id: "", harness: "claude", label: "Claude Code", name: "Claude Code", branch: null, group: null, accentVar: "--agent-claude", cwd: "", status: "available", exitCode: null },
  { id: "", harness: "codex", label: "Codex", name: "Codex", branch: null, group: null, accentVar: "--agent-codex", cwd: "", status: "available", exitCode: null },
  { id: "", harness: "pi", label: "Pi", name: "Pi", branch: null, group: null, accentVar: "--agent-pi", cwd: "", status: "available", exitCode: null },
];

const hosted: HostedView[] = [
  {
    id: "h-1",
    harness: "claude",
    label: "Claude Code",
    name: "Claude Code",
    branch: "axio/01k4hosted1",
    group: null,
    accentVar: "--agent-claude",
    cwd: "/Users/me/.axio/supervisor/worktrees/p-umbra/01K4HOSTED1",
    status: "running",
    exitCode: null,
  },
  {
    id: "h-2",
    harness: "claude",
    label: "Claude Code",
    name: "Claude Code 2",
    branch: null,
    group: null,
    accentVar: "--agent-claude",
    cwd: "/Users/me/src/axio",
    status: "running",
    exitCode: null,
  },
  {
    id: "h-3",
    harness: "codex",
    label: "Codex",
    name: "Codex",
    branch: "axio/01k4grpc",
    group: "g-01k4grp",
    accentVar: "--agent-codex",
    cwd: "/Users/me/src/umbra/.axio/worktrees/01k4grpc",
    status: "running",
    exitCode: null,
  },
];

let settings: AppSettings = {
  appearance: { uiFont: "", uiScale: 1, density: "comfortable", theme: import.meta.env.VITE_MOCK_THEME ?? "dark" },
  terminal: { font: "", fontSize: 13, lineHeight: 1, scrollback: 10000 },
  editor: "code",
  agents: { claude: { args: "--verbose" } },
};
let model: ModelDefault = { provider: "openai-codex", name: "gpt-5.4-mini" };
const settingsView = (): SettingsView => ({
  settings,
  model,
  providers: ["anthropic", "ollama", "openai-compatible", "openai-codex"],
  appPath: "/Users/me/.axio/app.toml",
  configPath: "/Users/me/.axio/config.toml",
});

// The running session's answer, typed out a few words per read.
const STREAM =
  "The strip and the bar say the same thing twice: which session is in front. " +
  "Folding the tab list into the bar leaves one row that names the place and " +
  "carries its controls.\n\n" +
  "I'll move `Transcript`/`Changes` and `Close`/`Discard` into the strip when a " +
  "session tab is active, and drop the strip entirely when nothing is open.";
let streamed = 0;

function transcriptFor(session: SessionView): TranscriptEntry[] {
  const base: TranscriptEntry[] = [
    { kind: "user", id: "u1", text: session.label ?? "(no prompt)" },
    {
      kind: "reasoning",
      id: "r1",
      text: "Look at how the tab strip and session bar relate before changing either.",
    },
    {
      kind: "tool",
      id: "t1",
      name: "read",
      subject: "read:crates/axio-app/ui/src/App.tsx",
      status: "ok",
      output: "606 lines",
      truncated: false,
      preview: null,
      ms: 4,
    },
    {
      kind: "tool",
      id: "t2",
      name: "shell",
      subject: "shell:npm --prefix crates/axio-app/ui run typecheck",
      status: "ok",
      output: "> tsc --noEmit\n",
      truncated: false,
      preview: { kind: "command", program: "npm", raw: "npm --prefix crates/axio-app/ui run typecheck", cwd: session.workspace },
      ms: 3120,
    },
  ];
  if (session.status === "running") {
    streamed = Math.min(STREAM.length, streamed + 9);
    base.push({ kind: "agent", id: "m1", text: STREAM.slice(0, streamed), streaming: streamed < STREAM.length });
    if (streamed >= STREAM.length) {
      base.push({
        kind: "tool",
        id: "t3",
        name: "edit",
        subject: "edit:crates/axio-app/ui/src/App.tsx",
        status: "running",
        output: "",
        truncated: false,
        preview: { kind: "diff", path: "crates/axio-app/ui/src/App.tsx", unified: DIFF, added: 1, removed: 5 },
        ms: 0,
      });
    }
  } else {
    base.push({ kind: "agent", id: "m1", text: STREAM, streaming: false });
    base.push({ kind: "turn", id: "e1", outcome: "completed", detail: "4 steps", costUsd: 0.0182 });
  }
  return base;
}

let hostedCursor = 0;
const HOSTED_BYTES =
  "\x1b[1;35m▐ Claude Code\x1b[0m  v1.0.0\r\n\r\n" +
  "  /Users/me/src/umbra\r\n\r\n" +
  "\x1b[2m› \x1b[0mWhat should I work on?\r\n";

export async function mockInvoke<T>(cmd: string, rawArgs?: unknown): Promise<T> {
  const out = (value: unknown) => value as T;
  const args = (rawArgs ?? {}) as Record<string, unknown>;
  switch (cmd) {
    case "snapshot":
      return out({ projects, sessions, approvals, unavailable: null } satisfies Snapshot);
    case "approvals":
      return out(approvals);
    case "session_transcript": {
      const s = sessions.find((x) => x.id === (args.sessionId as string));
      if (!s) throw { kind: "NoSuchSession", message: "no session by that id" };
      return out({
        entries: transcriptFor(s),
        model: "claude-sonnet-5",
        costUsd: s.status === "running" ? 0.0091 : 0.0182,
        fromRecord: s.status === "closed",
      } satisfies TranscriptView);
    }
    case "session_diff":
      return out(DIFF);
    case "start_session": {
      const input = args.input as StartSessionInput;
      const id = `s-new${Math.random().toString(16).slice(2, 10)}`;
      const project = projects.find((p) => p.root === input.path) ?? projects[0]!;
      sessions.unshift({
        id,
        shortId: id.slice(2, 10),
        projectId: project.id,
        projectName: project.name,
        label: input.prompt ?? null,
        title: null,
        group: input.group ?? null,
        branch: `axio/${id.slice(2, 10)}`,
        workspace: `${project.root}/.axio/worktrees/${id.slice(2, 10)}`,
        isolation: input.isolation ?? "worktree",
        status: "running",
        open: true,
        startedMs: Date.now(),
      });
      return out(sessions[0]);
    }
    case "send_prompt":
    case "cancel_session": {
      const s = sessions.find((x) => x.id === (args.sessionId as string));
      if (s) s.status = cmd === "send_prompt" ? "running" : "idle";
      return out(undefined);
    }
    case "close_session": {
      const s = sessions.find((x) => x.id === (args.sessionId as string));
      if (s) {
        s.status = "closed";
        s.open = false;
      }
      return out(undefined);
    }
    case "resolve_approval": {
      const at = approvals.findIndex((a) => a.id === (args.approvalId as string));
      if (at >= 0) approvals.splice(at, 1);
      const decision = args.decision as DecisionInput;
      return out(decision.decision !== "deny");
    }
    case "add_repository":
      return out(null);
    case "open_project":
      return out(projects[0]);
    case "window_control":
      return out(undefined);
    case "hosted_available":
      return out(hostedAvailable);
    case "hosted_list":
      return out(hosted);
    case "hosted_start": {
      const input = args.input as { harness: string; cwd: string; isolation: string | null };
      const h = hostedAvailable.find((a) => a.harness === input.harness) ?? hostedAvailable[0]!;
      const n = hosted.filter((x) => x.harness === h.harness).length + 1;
      const id = Math.random().toString(16).slice(2, 8);
      const view: HostedView = {
        ...h,
        id: `h-${id}`,
        name: n === 1 ? h.label : `${h.label} ${n}`,
        branch: input.isolation === "direct" ? null : `axio/01k4${id}`,
        group: (args.input as { group?: string | null }).group ?? null,
        cwd: input.cwd,
        status: "running",
      };
      hosted.push(view);
      return out(view);
    }
    case "hosted_read": {
      const from = Number(args.from ?? 0);
      const text = HOSTED_BYTES.slice(from);
      hostedCursor = HOSTED_BYTES.length;
      return out({ text, cursor: hostedCursor } satisfies HostedOutput);
    }
    case "hosted_write":
    case "hosted_resize":
      return out(undefined);
    case "hosted_kill": {
      const at = hosted.findIndex((h) => h.id === (args.id as string));
      if (at >= 0) hosted.splice(at, 1);
      return out(undefined);
    }
    case "start_group": {
      const input = args.input as StartGroupInput;
      const group = `g-${Math.random().toString(16).slice(2, 8)}`;
      const started: SessionView[] = [];
      for (let i = 0; i < input.count; i++) {
        const s = (await mockInvoke<SessionView>("start_session", {
          input: { path: input.path, prompt: input.prompt, isolation: "worktree", group },
        })) as SessionView;
        started.push(s);
      }
      const terminals: HostedView[] = [];
      for (const harness of input.agents) {
        terminals.push(
          await mockInvoke<HostedView>("hosted_start", {
            input: { harness, cwd: input.path, isolation: "worktree", group, args: "", rows: null, cols: null },
          }),
        );
      }
      return out({ group, sessions: started, terminals });
    }
    case "add_to_group": {
      const input = args.input as { group: string; path: string; prompt: string | null; harness: string | null };
      if (input.harness === null) {
        const s = (await mockInvoke<SessionView>("start_session", {
          input: { path: input.path, prompt: input.prompt, isolation: "worktree", group: input.group },
        })) as SessionView;
        return out({ group: input.group, sessions: [s], terminals: [] });
      }
      const h = await mockInvoke<HostedView>("hosted_start", {
        input: { harness: input.harness, cwd: input.path, isolation: "worktree", group: input.group, args: "", rows: null, cols: null },
      });
      return out({ group: input.group, sessions: [], terminals: [h] });
    }
    case "rename_session": {
      const s = sessions.find((x) => x.id === (args.sessionId as string));
      if (s) s.title = (args.title as string | null) || null;
      return out(undefined);
    }
    case "rename_hosted": {
      const h = hosted.find((x) => x.id === (args.id as string));
      if (!h) throw { kind: "NoSuchSession", message: "no hosted session" };
      h.name = (args.title as string | null) || h.label;
      return out(h);
    }
    case "hosted_diff":
      return out(DIFF);
    case "landing": {
      const s = sessions.find((x) => x.id === (args.sessionId as string));
      return out({
        branch: s?.branch ?? null,
        base: "main",
        changed: [" M crates/axio-app/ui/src/App.tsx", "?? crates/axio-app/ui/src/GroupPane.tsx"],
        ahead: s?.status === "idle" ? 2 : 0,
        remote: "git@github.com:umbra-me/axio.git",
        canPr: true,
      });
    }
    case "land": {
      const action = args.action as LandAction;
      return out({
        message: action === "merge" ? "merged into main" : action === "push" ? "pushed to origin" : "opened a pull request",
        url: action === "pullRequest" ? "https://github.com/umbra-me/axio/pull/42" : null,
      });
    }
    case "providers":
      return out([
        { name: "anthropic", ready: false },
        { name: "ollama", ready: false },
        { name: "openai-compatible", ready: false },
        { name: "openai-codex", ready: import.meta.env.VITE_MOCK_VIEW !== "firstrun" },
      ]);
    case "reveal":
      return out(undefined);
    case "settings":
      return out(settingsView());
    case "save_settings":
      settings = args.settings as AppSettings;
      return out(settingsView());
    case "set_default_model":
      model = args.model as ModelDefault;
      return out(settingsView());
    default:
      throw { kind: "Unknown", message: `the mock has no answer for \`${cmd}\`` };
  }
}
