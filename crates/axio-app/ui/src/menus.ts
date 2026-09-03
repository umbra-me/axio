import { api, describe, type HostedView, type SessionView } from "./bridge";
import { copy, type MenuItem } from "./RowMenu";

// What can be done to a row, wherever the row is.
//
// The rail and the group's cards show the same sessions and terminals, so they
// offer the same menu — built here once. A caller adds what only it can offer
// (a card can "expand" into its own tab; the rail is already there) through
// `lead`, which goes at the top.

export type MenuHooks = {
  onChanged: () => void;
  onError: (message: string) => void;
  onCloseSession: (id: string, discard: boolean) => void;
  onStopTerminal: (id: string) => void;
};

export function sessionMenu(s: SessionView, hooks: MenuHooks, lead: MenuItem[] = []): MenuItem[] {
  const fail = (e: unknown) => hooks.onError(describe(e));
  return [
    ...lead,
    ...(lead.length > 0 ? [{ kind: "rule" as const }] : []),
    {
      kind: "rename",
      title: "Rename…",
      current: s.title ?? s.label ?? "",
      run: (title) => void api.renameSession(s.id, title).then(hooks.onChanged).catch(fail),
    },
    { kind: "rule" },
    { kind: "action", title: "Reveal worktree", run: () => void api.reveal(s.workspace, false).catch(fail) },
    { kind: "action", title: "Open in editor", run: () => void api.reveal(s.workspace, true).catch(fail) },
    { kind: "action", title: "Copy path", run: () => copy(s.workspace) },
    ...(s.branch ? [{ kind: "action" as const, title: "Copy branch", run: () => copy(s.branch ?? "") }] : []),
    ...(s.open
      ? [
          { kind: "rule" as const },
          { kind: "action" as const, title: "Close, keep worktree", run: () => hooks.onCloseSession(s.id, false) },
          { kind: "action" as const, title: "Discard worktree", danger: true, run: () => hooks.onCloseSession(s.id, true) },
        ]
      : []),
  ];
}

export function hostedMenu(h: HostedView, hooks: MenuHooks, lead: MenuItem[] = []): MenuItem[] {
  const fail = (e: unknown) => hooks.onError(describe(e));
  return [
    ...lead,
    ...(lead.length > 0 ? [{ kind: "rule" as const }] : []),
    {
      kind: "rename",
      title: "Rename…",
      current: h.name,
      run: (title) => void api.renameHosted(h.id, title).then(hooks.onChanged).catch(fail),
    },
    { kind: "rule" },
    { kind: "action", title: "Reveal directory", run: () => void api.reveal(h.cwd, false).catch(fail) },
    { kind: "action", title: "Open in editor", run: () => void api.reveal(h.cwd, true).catch(fail) },
    { kind: "action", title: "Copy path", run: () => copy(h.cwd) },
    ...(h.branch ? [{ kind: "action" as const, title: "Copy branch", run: () => copy(h.branch ?? "") }] : []),
    { kind: "rule" },
    { kind: "action", title: `Stop ${h.name}`, danger: true, run: () => hooks.onStopTerminal(h.id) },
  ];
}

export function projectMenu(root: string, onError: (message: string) => void): MenuItem[] {
  const fail = (e: unknown) => onError(describe(e));
  return [
    { kind: "action", title: "Reveal repository", run: () => void api.reveal(root, false).catch(fail) },
    { kind: "action", title: "Open in editor", run: () => void api.reveal(root, true).catch(fail) },
    { kind: "action", title: "Copy path", run: () => copy(root) },
  ];
}
