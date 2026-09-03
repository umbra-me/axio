import type { ReactNode } from "react";

// The model's prose, rendered — and rendered by hand.
//
// Agent messages are markdown in practice: fenced code, lists, headings,
// `inline code`, **emphasis**. Showing them as raw text was honest and hard to
// read. This is the small subset that appears in a coding session, built as
// React elements rather than HTML, so nothing the model writes can become
// markup — there is no `innerHTML` anywhere in this file, and a link is shown
// with its address rather than made clickable.

type Block =
  | { kind: "code"; lang: string; text: string }
  | { kind: "heading"; level: number; text: string }
  | { kind: "list"; ordered: boolean; items: string[] }
  | { kind: "quote"; text: string }
  | { kind: "para"; text: string };

function blocks(source: string): Block[] {
  const out: Block[] = [];
  const lines = source.replace(/\r\n?/g, "\n").split("\n");
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] ?? "";
    if (line.trim() === "") {
      i += 1;
      continue;
    }
    const fence = /^\s*```\s*(\S*)\s*$/.exec(line);
    if (fence) {
      const body: string[] = [];
      i += 1;
      while (i < lines.length && !/^\s*```\s*$/.test(lines[i] ?? "")) {
        body.push(lines[i] ?? "");
        i += 1;
      }
      i += 1;
      out.push({ kind: "code", lang: fence[1] ?? "", text: body.join("\n") });
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    if (heading) {
      out.push({ kind: "heading", level: (heading[1] ?? "#").length, text: heading[2] ?? "" });
      i += 1;
      continue;
    }
    if (/^\s*>/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && /^\s*>/.test(lines[i] ?? "")) {
        body.push((lines[i] ?? "").replace(/^\s*>\s?/, ""));
        i += 1;
      }
      out.push({ kind: "quote", text: body.join("\n") });
      continue;
    }
    const bullet = /^\s*([-*+]|\d+[.)])\s+/.exec(line);
    if (bullet) {
      const ordered = /\d/.test(bullet[1] ?? "");
      const items: string[] = [];
      while (i < lines.length) {
        const item = /^\s*([-*+]|\d+[.)])\s+(.*)$/.exec(lines[i] ?? "");
        if (!item) break;
        items.push(item[2] ?? "");
        i += 1;
        // A continuation line indented under the bullet belongs to it.
        while (i < lines.length && /^\s{2,}\S/.test(lines[i] ?? "") && !/^\s*([-*+]|\d+[.)])\s+/.test(lines[i] ?? "")) {
          items[items.length - 1] += " " + (lines[i] ?? "").trim();
          i += 1;
        }
      }
      out.push({ kind: "list", ordered, items });
      continue;
    }
    const body: string[] = [];
    while (i < lines.length) {
      const next = lines[i] ?? "";
      if (next.trim() === "" || /^\s*```/.test(next) || /^#{1,6}\s/.test(next) || /^\s*>/.test(next) || /^\s*([-*+]|\d+[.)])\s+/.test(next)) {
        break;
      }
      body.push(next);
      i += 1;
    }
    out.push({ kind: "para", text: body.join("\n") });
  }
  return out;
}

// Inline: code spans first, so nothing inside one is read as emphasis.
function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\*[^*\n]+\*|_[^_\n]+_)|(\[[^\]\n]+\]\([^)\s]+\))/g;
  let last = 0;
  let key = 0;
  for (const m of text.matchAll(re)) {
    const at = m.index ?? 0;
    if (at > last) out.push(text.slice(last, at));
    const token = m[0];
    if (m[1]) out.push(<code key={key++}>{token.slice(1, -1)}</code>);
    else if (m[2]) out.push(<strong key={key++}>{token.slice(2, -2)}</strong>);
    else if (m[3]) out.push(<em key={key++}>{token.slice(1, -1)}</em>);
    else if (m[4]) {
      const link = /^\[([^\]]+)\]\(([^)]+)\)$/.exec(token);
      out.push(
        <span key={key++} className="md-link">
          {link?.[1]}
          <span className="md-href"> {link?.[2]}</span>
        </span>,
      );
    }
    last = at + token.length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

export function Markdown({ text }: { text: string }) {
  return (
    <div className="md">
      {blocks(text).map((block, n) => {
        switch (block.kind) {
          case "code":
            return (
              <pre key={n} className="md-code" data-lang={block.lang || undefined}>
                {block.text}
              </pre>
            );
          case "heading": {
            const Tag = (`h${Math.min(block.level + 2, 6)}`) as "h3" | "h4" | "h5" | "h6";
            return <Tag key={n}>{inline(block.text)}</Tag>;
          }
          case "list":
            return block.ordered ? (
              <ol key={n}>
                {block.items.map((item, i) => (
                  <li key={i}>{inline(item)}</li>
                ))}
              </ol>
            ) : (
              <ul key={n}>
                {block.items.map((item, i) => (
                  <li key={i}>{inline(item)}</li>
                ))}
              </ul>
            );
          case "quote":
            return <blockquote key={n}>{inline(block.text)}</blockquote>;
          case "para":
            return <p key={n}>{inline(block.text)}</p>;
        }
      })}
    </div>
  );
}
