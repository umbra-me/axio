// Where each member of a pane sits: a tree of splits ending in cards.
//
// Held by the window, like card sizes, because where things are on screen is
// a fact about the window and not about the work. A tree rather than a grid
// because "one on the left, two stacked on the right" is a tree and not a
// grid, and every editor that lets you split a view keeps one.

export type LayoutNode =
  | { kind: "leaf"; id: string }
  | { kind: "split"; dir: "row" | "col"; ratio: number; a: LayoutNode; b: LayoutNode };

export type Side = "left" | "right" | "top" | "bottom" | "center";

/** Every member id in the tree, left to right, top to bottom. */
export function leaves(node: LayoutNode): string[] {
  return node.kind === "leaf" ? [node.id] : [...leaves(node.a), ...leaves(node.b)];
}

/** An even split of `ids` along `dir`, nested so no divider dominates. */
export function balanced(ids: string[], dir: "row" | "col"): LayoutNode | null {
  if (ids.length === 0) return null;
  if (ids.length === 1) return { kind: "leaf", id: ids[0] ?? "" };
  const mid = Math.ceil(ids.length / 2);
  const a = balanced(ids.slice(0, mid), dir);
  const b = balanced(ids.slice(mid), dir);
  if (!a || !b) return a ?? b;
  return { kind: "split", dir, ratio: mid / ids.length, a, b };
}

/** Columns side by side, each column its members stacked: the shape a plan
 *  like "one left, two right" describes. */
export function fromColumns(columns: string[][]): LayoutNode | null {
  const cols = columns.map((c) => balanced(c, "col")).filter((c): c is LayoutNode => c !== null);
  if (cols.length === 0) return null;
  return cols.reduce<LayoutNode | null>((acc, col, i) => {
    if (!acc) return col;
    // Each new column takes its fair share of what is left.
    return { kind: "split", dir: "row", ratio: i / (i + 1), a: acc, b: col };
  }, null);
}

/** The tree with `id` taken out, collapsing the split it leaves behind. */
export function without(node: LayoutNode, id: string): LayoutNode | null {
  if (node.kind === "leaf") return node.id === id ? null : node;
  const a = without(node.a, id);
  const b = without(node.b, id);
  if (!a) return b;
  if (!b) return a;
  return { ...node, a, b };
}

/** `moving` placed against `target`: beside it, above or below it, or in its
 *  place with `target` taking the vacated spot. */
export function place(node: LayoutNode, moving: string, target: string, side: Side): LayoutNode {
  if (moving === target) return node;
  if (side === "center") return swap(node, moving, target);
  const pruned = without(node, moving);
  if (!pruned) return node;
  const insert = (n: LayoutNode): LayoutNode => {
    if (n.kind === "leaf") {
      if (n.id !== target) return n;
      const me: LayoutNode = { kind: "leaf", id: moving };
      const dir = side === "left" || side === "right" ? "row" : "col";
      const first = side === "left" || side === "top";
      return { kind: "split", dir, ratio: 0.5, a: first ? me : n, b: first ? n : me };
    }
    return { ...n, a: insert(n.a), b: insert(n.b) };
  };
  return insert(pruned);
}

function swap(node: LayoutNode, x: string, y: string): LayoutNode {
  if (node.kind === "leaf") return node.id === x ? { kind: "leaf", id: y } : node.id === y ? { kind: "leaf", id: x } : node;
  return { ...node, a: swap(node.a, x, y), b: swap(node.b, x, y) };
}

/** The tree brought up to date with who is actually here: members that
 *  left are removed, members that arrived are added on the right. */
export function reconcile(node: LayoutNode | null, ids: string[]): LayoutNode | null {
  let tree = node;
  if (tree) {
    for (const id of leaves(tree)) {
      if (!ids.includes(id)) tree = tree ? without(tree, id) : null;
    }
  }
  const present = tree ? leaves(tree) : [];
  for (const id of ids) {
    if (present.includes(id)) continue;
    const leaf: LayoutNode = { kind: "leaf", id };
    tree = tree ? { kind: "split", dir: "row", ratio: present.length / (present.length + 1), a: tree, b: leaf } : leaf;
    present.push(id);
  }
  return tree;
}

/** A ratio set on the split found by `path` — the a/b turns from the root. */
export function withRatio(node: LayoutNode, path: ("a" | "b")[], ratio: number): LayoutNode {
  if (node.kind === "leaf") return node;
  if (path.length === 0) return { ...node, ratio: Math.min(0.85, Math.max(0.15, ratio)) };
  const [head, ...rest] = path;
  return head === "a" ? { ...node, a: withRatio(node.a, rest, ratio) } : { ...node, b: withRatio(node.b, rest, ratio) };
}

// --- the launch plan -------------------------------------------------------

/** One planned member, as the composer holds it. `axio` is a session. */
/** `harness` is an agent name, or `codex-app`: Codex through its own
 *  protocol rather than a terminal — rows and questions instead of bytes. */
export type PlanMember = { harness: string; model: string; effort: string; permission: string; args: string; column: number };

/** The backend's view of a plan member's harness: the tool, and how it is driven. */
export function memberTransport(harness: string): { harness: string | null; transport: string | null } {
  if (harness === "axio") return { harness: null, transport: null };
  if (harness === "codex-app") return { harness: "codex", transport: "app" };
  return { harness, transport: null };
}

export const EFFORTS = ["", "low", "medium", "high", "xhigh"] as const;
/** What the tool may do without asking; `""` is the tool's own default. */
export const PERMISSIONS = ["", "ask", "edits", "auto", "full"] as const;

/**
 * A plan written as one line. Columns are separated by `|`, members within
 * a column by `,`; each member is an optional count, an agent, and flags:
 *
 *   2x claude --model opus | codex -m gpt-5.4 --effort high, pi
 *
 * That is two Claude Codes stacked on the left, and on the right a Codex
 * above a Pi. `--model`/`-m`/`model=`, `--effort`/`effort=` and
 * `--permission`/`permission=` (ask, edits, auto, full) are read; anything
 * else is passed to the tool as it is. Strict: an agent that is
 * not one of the four is an error naming it, not a guess.
 */
export function parsePlan(line: string, known: string[]): { members: PlanMember[] } | { error: string } {
  try {
    return { members: parseStrict(line, known) };
  } catch (e) {
    return { error: e instanceof Error ? e.message : String(e) };
  }
}

function parseStrict(line: string, known: string[]): PlanMember[] {
  const members: PlanMember[] = [];
  const columns = line.split("|").map((c) => c.trim()).filter((c) => c !== "");
  if (columns.length === 0) throw new Error("nothing to start");
  columns.forEach((column, ci) => {
    for (const cell of column.split(",").map((c) => c.trim()).filter((c) => c !== "")) {
      const tokens = cell.split(/\s+/);
      let count = 1;
      const first = tokens[0] ?? "";
      const n = /^(\d+)[x×]?$/i.exec(first);
      if (n) {
        count = Math.max(1, Math.min(8, Number(n[1])));
        tokens.shift();
      }
      const harness = (tokens.shift() ?? "").toLowerCase();
      if (!known.includes(harness)) throw new Error(`\`${harness || cell}\` is not an agent here — one of ${known.join(", ")}`);
      const m: PlanMember = { harness, model: "", effort: "", permission: "", args: "", column: ci };
      const rest: string[] = [];
      for (let i = 0; i < tokens.length; i++) {
        const t = tokens[i] ?? "";
        const eq = /^(model|effort|permission)=(.+)$/.exec(t);
        if (eq) {
          if (eq[1] === "model") m.model = eq[2] ?? "";
          else if (eq[1] === "effort") m.effort = eq[2] ?? "";
          else m.permission = eq[2] ?? "";
        } else if ((t === "--model" || t === "-m") && tokens[i + 1]) {
          m.model = tokens[++i] ?? "";
        } else if (t === "--effort" && tokens[i + 1]) {
          m.effort = tokens[++i] ?? "";
        } else if (t === "--permission" && tokens[i + 1]) {
          m.permission = tokens[++i] ?? "";
        } else {
          rest.push(t);
        }
      }
      m.args = rest.join(" ");
      for (let k = 0; k < count; k++) members.push({ ...m });
    }
  });
  return members;
}

/** The plan as a line, the inverse of `parsePlan` for what it can express. */
export function formatPlan(members: PlanMember[]): string {
  const cols = new Map<number, string[]>();
  for (const m of members) {
    const flags = [
      m.model ? `--model ${m.model}` : "",
      m.effort ? `--effort ${m.effort}` : "",
      m.permission ? `--permission ${m.permission}` : "",
      m.args,
    ]
      .filter(Boolean)
      .join(" ");
    const list = cols.get(m.column) ?? [];
    list.push(flags ? `${m.harness} ${flags}` : m.harness);
    cols.set(m.column, list);
  }
  return [...cols.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([, cells]) => cells.join(", "))
    .join(" | ");
}
