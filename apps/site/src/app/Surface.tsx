import type { CSSProperties } from "react";

/**
 * The desktop surface, drawn rather than screenshotted.
 *
 * It is the ground the hero's glass panel is translucent to — the web page's
 * substitute for the wallpaper the real window samples. Drawn because a
 * screenshot would be a picture of an interface at one size, in one theme, at
 * one moment, and this has to reflow into a phone and dim behind text.
 *
 * Everything in it is the product's own vocabulary: four sessions across two
 * repositories, each wearing its agent's colour, one of them holding a question
 * the person has not answered yet. That is the argument the headline beside it
 * makes, so the two are saying the same thing in two registers.
 *
 * Marked aria-hidden. It is an illustration of the interface, not the
 * interface; every fact in it is stated in words elsewhere on the page.
 */

const AGENTS = {
  axio: "var(--accent)",
  claude: "var(--agent-claude)",
  codex: "var(--agent-codex)",
  pi: "var(--agent-pi)",
} as const;

type Session = {
  label: string;
  agent: keyof typeof AGENTS;
  /** `waiting` is the state the whole page is about, so it is not `idle`. */
  state: "running" | "waiting" | "idle";
  age: string;
  active?: boolean;
};

const PROJECTS: { name: string; sessions: Session[] }[] = [
  {
    name: "axio",
    sessions: [
      { label: "span table", agent: "axio", state: "running", age: "4m" },
      {
        label: "pty exit codes",
        agent: "claude",
        state: "waiting",
        age: "2m",
        active: true,
      },
    ],
  },
  {
    name: "web-stack",
    sessions: [
      { label: "bump next 16", agent: "codex", state: "running", age: "11m" },
      { label: "dep audit", agent: "pi", state: "idle", age: "1h" },
    ],
  },
];

function IconRepo() {
  return (
    <svg
      width="12"
      height="12"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <path d="M3.5 2.5h9v11h-9a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1Z" />
      <path d="M2.5 11h10" />
    </svg>
  );
}

function IconBranch() {
  return (
    <svg
      width="11"
      height="11"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <circle cx="4.5" cy="3.5" r="1.75" />
      <circle cx="4.5" cy="12.5" r="1.75" />
      <circle cx="11.5" cy="3.5" r="1.75" />
      <path d="M4.5 5.25v5.5M11.5 5.25c0 3-3.5 2.5-7 2.5" />
    </svg>
  );
}

export default function Surface({ className }: { className?: string }) {
  const total = PROJECTS.reduce((n, p) => n + p.sessions.length, 0);

  return (
    <div className={className} aria-hidden="true">
      <div className="surface">
        <div className="surface__bar">
          <i />
          axio
          <span className="sep">/</span>
          <span>4 sessions</span>
          <span className="surface__branch">
            <IconBranch />
          </span>
        </div>

        <div className="surface__body">
          <div className="surface__rail">
            <div className="rail-head label">
              Sessions
              <span className="count">{total}</span>
            </div>

            {PROJECTS.map((project) => (
              <div className="project" key={project.name}>
                <h3>
                  <IconRepo />
                  {project.name}
                </h3>
                <ul className="sessions">
                  {project.sessions.map((s) => (
                    <li
                      key={s.label}
                      className={`session${s.active ? " active" : ""}`}
                      style={{ "--agent-accent": AGENTS[s.agent] } as CSSProperties}
                    >
                      <span className={`dot ${s.state}`} />
                      <span className="label-text">{s.label}</span>
                      <small>{s.age}</small>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </div>

          <div className="surface__pane">
            <div className="crumbs">
              axio
              <span className="sep">/</span>
              <span className="here">pty exit codes</span>
            </div>
            <div>
              <pre className="surface__diff">
                <span className="hunk">
                  {"@@ -140,11 +140,14 @@ impl HarnessSession\n"}
                </span>
                {"     /// The child's status, once the waiter has seen it.\n"}
                {"     pub fn status(&self) -> Option<ExitStatus> {\n"}
                <span className="del">
                  {"-        self.child.lock().try_wait().ok().flatten()\n"}
                </span>
                <span className="add">
                  {"+        self.exit.load(Ordering::Acquire).map(Into::into)\n"}
                </span>
                {"     }\n"}
                {"\n"}
                <span className="hunk">{"@@ -204,6 +207,9 @@ impl Drop for Master\n"}</span>
                {"     fn drop(&mut self) {\n"}
                <span className="add">
                  {"+        // Closing blocks until the pipe drains, so it\n"}
                </span>
                <span className="add">
                  {"+        // cannot happen on the thread that asked.\n"}
                </span>
                {"         let pty = self.pty.take();\n"}
              </pre>
              <div className="surface__approval">
                {"approve  edit:crates/axio-pty/src/session.rs\n"}
                <span>{"allow? y once   a this session   n no"}</span>
              </div>
            </div>
          </div>
        </div>

        <div className="surface__status">
          <span className="ok">2 running</span>
          <span>1 idle</span>
          <span className="warn">1 waiting on you</span>
        </div>
      </div>
    </div>
  );
}
