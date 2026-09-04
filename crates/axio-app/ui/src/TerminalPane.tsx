import { useCallback, useEffect, useState } from "react";
import { api, describe, listen, type HostedApproval, type HostedView, type TerminalSettings, type TranscriptView } from "./bridge";
import { Transcript } from "./Transcript";
import { TerminalSummary, viewItems, type TerminalView } from "./GroupPane";
import { hostedMenu, type MenuHooks } from "./menus";
import { copy, RowMenu, type MenuItem } from "./RowMenu";
import { HostedTerminal } from "./Terminal";
import { IconBranch, IconClose, IconMore, IconStop, IconTerminal } from "./icons";

// One terminal in its own tab, three ways.
//
// A card, the same one a group shows, filling the pane: what the agent is,
// what state it is in, a follow-up typed into it, where it works. The same
// card with the real terminal for a body. Or the terminal alone, edge to
// edge, which is what this tab was before cards existed and is still the
// right answer for a full-screen TUI. The choice is on the card's menu and
// is the terminal's own; a new terminal opens the way the settings say.
export { HostedChat };

export function TerminalPane({
  terminal,
  view,
  settings,
  resumeKey,
  hooks,
  onView,
  onResume,
  onRemove,
}: {
  terminal: HostedView;
  view: TerminalView;
  settings: TerminalSettings | null;
  /** Changes on a resume, so the emulator remounts for the new process. */
  resumeKey: number;
  hooks: MenuHooks;
  onView: (view: TerminalView) => void;
  onResume: () => void;
  onRemove: () => void;
}) {
  const [menu, setMenu] = useState<{ items: MenuItem[]; at: { x: number; y: number } } | null>(null);
  const live = terminal.status === "running";
  // The chat view reads the agent's own transcript, which exists once its
  // hooks have named the file; offered only then.
  const structured = terminal.transport === "app";
  const choices: TerminalView[] = structured
    ? ["card", "chat"]
    : terminal.providerSession
      ? ["card", "tui", "plain", "chat"]
      : ["card", "tui", "plain"];
  const items: MenuItem[] = [...viewItems(view, choices, onView), { kind: "rule" }, ...hostedMenu(terminal, hooks)];
  const openMenu = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ items, at: { x: e.clientX, y: e.clientY } });
  };

  const ended = !live && (
    <p className="notice dismissable terminal-ended">
      <span>
        {terminal.name} {terminal.stopped ? "is stopped" : "is not running"}
        {terminal.exitCode !== null ? ` (exit ${terminal.exitCode})` : ""}. Its worktree is kept
        {terminal.branch ? ` on ${terminal.branch}` : ""}; resume starts it there again and asks it to continue
        {terminal.stopped ? ". It stays stopped until you do, this window and the next." : "."}
      </span>
      <button className="act primary" onClick={onResume}>
        Resume
      </button>
      <button className="act" onClick={onRemove}>
        Remove
      </button>
    </p>
  );
  const emulator = <HostedTerminal key={`${terminal.id}:${resumeKey}`} session={terminal} terminal={settings} />;
  const popup = menu && <RowMenu items={menu.items} at={menu.at} onClose={() => setMenu(null)} />;

  if (view === "plain" && !structured) {
    // No chrome, but still a way back: the menu is on the terminal itself.
    return (
      <>
        {ended}
        <div className="terminal-plain" onContextMenu={openMenu}>
          {emulator}
        </div>
        {popup}
      </>
    );
  }

  return (
    <>
      {ended}
      <section className="group-card solo" style={{ ["--agent-accent" as string]: `var(${terminal.accentVar})` }} onContextMenu={openMenu}>
        <div className="group-card-head">
          <span className={`dot ${terminal.status}`} />
          <IconTerminal size={12} />
          <span className="name">{terminal.name}</span>
          <span className="state">{live ? "working" : terminal.status}</span>
          <span className="card-acts">
            <button className="row-more" aria-label={`Options for ${terminal.name}`} title="Options — including how this terminal is shown" onClick={openMenu}>
              <IconMore size={12} />
            </button>
            {live ? (
              <button className="row-more" aria-label={`Stop ${terminal.name}`} title="Stop the terminal" onClick={() => hooks.onStopTerminal(terminal.id)}>
                <IconStop size={12} />
              </button>
            ) : (
              <button className="row-more" aria-label={`Remove ${terminal.name}`} title="Remove from the list" onClick={onRemove}>
                <IconClose size={12} />
              </button>
            )}
          </span>
        </div>
        <div className="group-card-facts">
          {terminal.branch && (
            <span className="branch" title="Copy the branch name" onClick={() => copy(terminal.branch ?? "")}>
              <IconBranch size={11} />
              {terminal.branch}
            </span>
          )}
          <span className="quiet" title={terminal.cwd}>
            {terminal.cwd}
          </span>
        </div>
        {view === "card" ? (
          <TerminalSummary terminal={terminal} settings={settings} resumeKey={resumeKey} onError={hooks.onError} />
        ) : view === "chat" || structured ? (
          <HostedChat terminal={terminal} onError={hooks.onError} />
        ) : (
          <div className="group-card-term">{emulator}</div>
        )}
      </section>
      {popup}
    </>
  );
}

// The agent's transcript as it writes it, read from its own file whenever
// the terminal says something happened — the same rows a session shows,
// from a file rather than a stream.
function HostedChat({ terminal, onError }: { terminal: HostedView; onError: (message: string) => void }) {
  const [view, setView] = useState<TranscriptView | null>(null);
  // A structured agent's questions, answered here; a terminal's are
  // answered in the terminal and this list stays empty.
  const [asks, setAsks] = useState<HostedApproval[]>([]);
  const pull = useCallback(async () => {
    try {
      setView(await api.hostedTranscript(terminal.id));
      if (terminal.transport === "app") setAsks(await api.hostedApprovals(terminal.id));
    } catch (e) {
      onError(describe(e));
    }
  }, [terminal.id, terminal.transport, onError]);
  const decide = async (approval: string, decision: string) => {
    try {
      await api.hostedDecide(terminal.id, approval, decision);
      await pull();
    } catch (e) {
      onError(describe(e));
    }
  };
  useEffect(() => {
    void pull();
    const timer = window.setInterval(() => void pull(), 4000);
    const unlisten = listen<string>("axio://hosted-activity", (event) => {
      if (event.payload === terminal.id) void pull();
    });
    return () => {
      window.clearInterval(timer);
      void unlisten.then((off) => off());
    };
  }, [pull, terminal.id]);
  return (
    <div className="group-card-live hosted-chat">
      {asks.length > 0 && (
        <div className="approvals hosted-asks">
          {asks.map((a) => (
            <div className="approval" key={a.id}>
              <div className="approval-head">
                <b>{a.kind === "command" ? "Run" : "Edit"}</b> <code>{a.subject}</code>
                {a.detail && <span className="quiet"> — {a.detail}</span>}
              </div>
              <div className="approval-acts">
                <button className="act primary" onClick={() => void decide(a.id, "accept")}>
                  Allow once
                </button>
                <button className="act" onClick={() => void decide(a.id, "acceptForSession")}>
                  Allow this session
                </button>
                <button className="act danger" onClick={() => void decide(a.id, "decline")}>
                  Deny
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
      <Transcript view={view} />
    </div>
  );
}
