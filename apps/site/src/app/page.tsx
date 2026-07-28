import HeroTranscript from "./HeroTranscript";
import ThemeToggle from "./ThemeToggle";

const REPO = "https://github.com/umbra-me/axio";

/** A section's left gutter marker and its body, on the page's two-column grid. */
function Row({
  mark,
  children,
}: {
  mark?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="row">
      <div className="mark">{mark ?? ""}</div>
      <div>{children}</div>
    </div>
  );
}

export default function Page() {
  return (
    <div className="wrap">
      <header className="masthead">
        <Row mark="Apache-2.0">
          <h1 className="wordmark">axio</h1>
          <p className="standfirst">
            An AI coding agent that stays inside your terminal instead of taking
            it over.
          </p>
          <p className="status-line">
            <span>Rust 1.88+</span>
            <span>Linux · macOS · Windows</span>
            <span>
              <b>Pre-release — nothing is tagged</b>
            </span>
          </p>
        </Row>
      </header>

      <section>
        <Row mark="What it is">
          <p className="lead">
            axio runs its own agent loop: it talks to model providers directly,
            executes tool calls against your workspace, and streams the result
            back. One binary serves two surfaces — a one-shot CLI when you pipe
            into it or pass <code>-p</code>, and an interactive composer when
            you don&apos;t.
          </p>
          <p>
            The interactive surface is <em>inline</em>, not a full-screen
            application. The finished transcript is printed into your
            terminal&apos;s own scrollback, so it survives the process, scrolls
            with the scrollbar and copies with the mouse. Only the live part is
            redrawn. Fenced code is highlighted in your terminal&apos;s own
            colours rather than a bundled theme, so it matches everything around
            it.
          </p>
        </Row>
        <Row>
          <HeroTranscript />
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Install">
          <p>
            There is no published binary yet. Releases are built by tag and none
            has been cut, so this builds whatever <code>main</code> is:
          </p>
          <div className="term term--wide term--cmd">
            <pre>cargo install --git {REPO} --locked axio</pre>
          </div>
          <p style={{ marginTop: "1rem" }}>
            Needs a Rust toolchain at 1.88 or newer. The binary lands in{" "}
            <code>~/.cargo/bin</code>. From a clone,{" "}
            <code>cargo install --path crates/axio --locked</code> does the same.
          </p>
          <p>
            <code>cargo install axio</code> from crates.io does{" "}
            <strong>not</strong> work — that name belongs to an unrelated crate.
          </p>
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Use">
          <p>
            A credential comes from <code>axio auth login</code>, which reads
            from stdin and stores at <code>0600</code>, or from the environment,
            which always takes precedence.
          </p>
        </Row>
        <Row>
          <div className="term term--wide">
            <pre>
              {'axio -p "explain this repo"\ncat src/lib.rs | axio -p "review this"\n\n'}
              {"axio                      "}
              <span className="dim">
                # interactive, when stdin is a terminal
              </span>
              {"\naxio --list               "}
              <span className="dim"># recent sessions</span>
              {"\naxio --resume 01K3F       "}
              <span className="dim">
                # continue one; a unique prefix is enough
              </span>
              {"\n\naxio --doctor             "}
              <span className="dim">
                # what axio can see, offline, no credential
              </span>
              {"\naxio --probe              "}
              <span className="dim">
                # ask the model whether it accepts tools
              </span>
              {"\naxio --explain model.effort   "}
              <span className="dim"># where a setting came from</span>
            </pre>
          </div>
        </Row>
        <Row>
          <p style={{ marginTop: "1.5rem" }}>
            <code>--doctor</code> answers from configuration alone, so it is
            safe anywhere. <code>--probe</code> is the opposite by design: it
            sends two short requests, one carrying a tool, and reports whether
            the tool was accepted. A model can serve chat perfectly and reject
            every request that offers it a tool, and nothing in your
            configuration is wrong when that happens — only asking the model
            finds it.
          </p>
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Configuration">
          <p>
            Layered: defaults, then <code>~/.axio/config.toml</code>, then the
            nearest project <code>.axio/config.toml</code>, then{" "}
            <code>AXIO_*</code> variables, then flags. A project&apos;s own file
            may only add restrictions, never remove them.
          </p>
          <p>
            <code>~/.axio</code> is the same path on Windows, WSL, Linux and
            macOS rather than each platform&apos;s own configuration directory —
            one path to document, to sync between machines, and to name in a bug
            report. <code>AXIO_HOME</code> moves it.
          </p>
          <p>
            If the workspace root has an <code>AGENTS.md</code>, axio reads it
            and tells the model those instructions describe this codebase
            specifically and outrank its general habits.
          </p>
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Providers">
          <p>
            Four names over three wire dialects, not a plugin system.{" "}
            <code>anthropic</code> speaks the Messages API; <code>ollama</code>{" "}
            and <code>openai-compatible</code> share chat-completions, the second
            for any other host speaking that dialect;{" "}
            <code>openai-codex</code> speaks the Responses API and is signed in
            to through the browser rather than pasted into.
          </p>
          <dl className="pairs">
            <div>
              <dt>anthropic</dt>
              <dd>
                Messages API. <code>ANTHROPIC_API_KEY</code>, or stored.
              </dd>
            </div>
            <div>
              <dt>ollama</dt>
              <dd>
                Chat-completions, local. <code>OLLAMA_API_KEY</code>.
              </dd>
            </div>
            <div>
              <dt>openai-compatible</dt>
              <dd>
                Chat-completions, any host. Pointed with{" "}
                <code>model.base_url</code> or <code>AXIO_BASE_URL</code>.
              </dd>
            </div>
            <div>
              <dt>openai-codex</dt>
              <dd>
                Responses API. Browser sign-in on the loopback; the token pair is
                renewed before it expires.
              </dd>
            </div>
          </dl>
          <p style={{ marginTop: "1.5rem" }}>
            The model list is fetched from the provider, never compiled in: a
            name missing from a built-in catalogue looks exactly like a name the
            provider refuses.
          </p>
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Verification">
          <table className="ledger">
            <caption>
              What has actually been run against a live endpoint, and what has
              only been built to a documented spec. These are not the same claim,
              so they are not written as though they were.
            </caption>
            <thead>
              <tr>
                <th scope="col">Path</th>
                <th scope="col">State</th>
                <th scope="col">Basis</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <th scope="row">Chat-completions transport</th>
                <td className="state state--met">met</td>
                <td>
                  Thirteen checks over seven turns, end to end against a real
                  model, including on Windows.
                </td>
              </tr>
              <tr>
                <th scope="row">Responses transport</th>
                <td className="state state--met">met</td>
                <td>
                  Signed in through the browser flow, then the same thirteen
                  checks green. Its request body took two live 400s to settle.
                </td>
              </tr>
              <tr>
                <th scope="row">Messages transport</th>
                <td className="state state--unmet">not met</td>
                <td>
                  Built from the documented wire format and snapshot-tested
                  against it. There has been no credential to run it with, which
                  is not the same as having met the endpoint.
                </td>
              </tr>
              <tr>
                <th scope="row">Release artifacts</th>
                <td className="state state--unmet">none</td>
                <td>
                  Nothing is tagged. The version is cut when a few days pass
                  without real use turning something up, not before.
                </td>
              </tr>
            </tbody>
          </table>
        </Row>
      </section>

      <hr />

      <section>
        <Row mark="Safety">
          <p>
            axio executes code written by a language model against your working
            directory. Reads do not ask. Writes and shell commands do.
          </p>
          <p>
            Interactively you are asked, and the diff or the command lands in
            scrollback first. A shell command is shown as the string the shell
            actually receives, never a word-split of it — the split reads as a
            simpler command than the one that runs. In a one-shot run there is
            nobody to ask, so those actions are refused unless <code>--yes</code>{" "}
            was given, and a turn that completed with something refused exits{" "}
            <code>5</code>, so <code>&amp;&amp;</code> sees it.
          </p>
          <p>
            By default there is no sandbox: confinement is the workspace root,
            the approval prompt and process-group containment. On Linux,{" "}
            <code>--sandbox</code> adds Landlock, inherited by every command axio
            spawns — the workspace writable, the system readable, and{" "}
            <code>~/.ssh</code> not there at all. It says nothing about the
            network, and it is a second wall behind the permission engine rather
            than a replacement for it.
          </p>
          <p>
            Read <a href={`${REPO}/blob/main/SECURITY.md`}>SECURITY.md</a> before
            running it anywhere that matters.
          </p>
        </Row>
      </section>


      <hr />

      <footer>
        <Row mark="Docs">
          <p className="footer-links">
            <a href={REPO}>Repository</a>
            <a href={`${REPO}/blob/main/docs/architecture.md`}>Architecture</a>
            <a href={`${REPO}/blob/main/docs/gotchas.md`}>Gotchas</a>
            <a href={`${REPO}/blob/main/docs/roadmap.md`}>Roadmap</a>
            <a href={`${REPO}/blob/main/CHANGELOG.md`}>Changelog</a>
          </p>
          <p className="colophon">Apache-2.0. A product of Umbra.</p>
          <ThemeToggle />
        </Row>
      </footer>
    </div>
  );
}
