import { useMemo } from "react";

// What a session changed, one file at a time.
//
// A unified diff is one long string, and reading it as one long string is how
// the interesting hunk in the fourth file gets missed. Split on the file
// headers git writes, give each file a heading with its counts, and let a file
// that is not the point fold away.
type FileDiff = { path: string; added: number; removed: number; lines: string[] };

function split(text: string): FileDiff[] {
  const files: FileDiff[] = [];
  let current: FileDiff | null = null;
  for (const line of text.split("\n")) {
    const header = /^diff --git a\/(.+?) b\/(.+)$/.exec(line);
    if (header) {
      const file: FileDiff = { path: header[2] ?? "", added: 0, removed: 0, lines: [] };
      files.push(file);
      current = file;
      continue;
    }
    if (!current) {
      // Preamble before the first header — rare, but a string that starts
      // with it must not vanish.
      const file: FileDiff = { path: "", added: 0, removed: 0, lines: [] };
      files.push(file);
      current = file;
    }
    const file: FileDiff = current;
    if (line.startsWith("+") && !line.startsWith("+++")) file.added += 1;
    else if (line.startsWith("-") && !line.startsWith("---")) file.removed += 1;
    file.lines.push(line);
  }
  return files;
}

function classOf(line: string): string | undefined {
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("index ") || line.startsWith("new file") || line.startsWith("deleted file"))
    return "meta";
  return undefined;
}

export function Diff({ text }: { text: string | null }) {
  const files = useMemo(() => (text ? split(text) : []), [text]);
  if (text === null) return <div className="empty">Reading…</div>;
  if (text.trim() === "") {
    // A real outcome and a different one from a failure to read, which an empty
    // pane would be indistinguishable from.
    return <div className="empty">That session changed nothing.</div>;
  }
  return (
    <div className="diff-files">
      {files.map((file, n) => (
        <details className="diff-file" key={`${file.path}-${n}`} open>
          <summary>
            <span className="path">{file.path || "(preamble)"}</span>
            <span className="counts">
              {file.added > 0 && <span className="add">+{file.added}</span>}
              {file.removed > 0 && <span className="del">−{file.removed}</span>}
            </span>
          </summary>
          <pre className="diff">
            {file.lines.map((line, i) => (
              <div key={i} className={classOf(line)}>
                {line || " "}
              </div>
            ))}
          </pre>
        </details>
      ))}
    </div>
  );
}
