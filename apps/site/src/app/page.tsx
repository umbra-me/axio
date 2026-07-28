import HeroTranscript from "./HeroTranscript";
import Term from "./Term";
import ThemeToggle from "./ThemeToggle";

const REPO = "https://github.com/umbra-me/axio";

/** A claim and the artifact that answers it, side by side. */
function Spread({
  claim,
  evidence,
}: {
  claim: React.ReactNode;
  evidence: React.ReactNode;
}) {
  return (
    <section className="spread">
      <div className="claim">{claim}</div>
      <div className="evidence">{evidence}</div>
    </section>
  );
}

export default function Page() {
  return (
    <main className="wrap">
      <HeroTranscript />

      <Spread
        claim={
          <>
            <h2>It stays inline.</h2>
            <p>
              The finished transcript is printed into your terminal&apos;s own
              scrollback, so it survives the process, scrolls with the scrollbar
              and copies with the mouse. Only the live part is redrawn — a
              status line, the composer, the question being asked.
            </p>
            <p>
              The composer is a real line editor, so a pasted paragraph stays
              one prompt rather than submitting its first line. Fenced code is
              highlighted in your terminal&apos;s colours rather than a bundled
              theme, so it matches everything around it.
            </p>
          </>
        }
        evidence={
          <Term label="the composer is a line editor">{`
enter                     «send»
shift-enter   ctrl-j      «another line»
up   down                 «move a line, or recall history»
ctrl-w  ctrl-u  ctrl-k    «delete a word, to the start, to the end»
ctrl-left   ctrl-right    «move a word»
esc   ctrl-c              «interrupt a turn»
ctrl-d                    «leave»
`}</Term>
        }
      />

      <Spread
        claim={
          <>
            <h2>Reads don&apos;t ask. Writes do.</h2>
            <p>
              The diff or the command lands in scrollback first, then the
              viewport asks. A shell command is shown as the string the shell
              actually receives, never a word-split of it — the split reads as a
              simpler command than the one that runs.
            </p>
            <p>
              In a one-shot run there is nobody to ask, so those actions are
              refused unless <code>--yes</code> was given. A turn that completed
              with something refused exits <code>5</code>, so{" "}
              <code>&amp;&amp;</code> sees it.
            </p>
          </>
        }
        evidence={
          <Term label="an approval, in the viewport">{`
  approve  edit:notes.md
  «this will write files»
  «@@ -1,3 +1,4 @@»
   # Notes
   - first
  ‹+- second›

  allow? «y» once  «a» this session  «n» no
`}</Term>
        }
      />

      <Spread
        claim={
          <>
            <h2>Install it.</h2>
            <p>
              There is no published binary yet — releases are built by tag and
              none has been cut. This builds whatever <code>main</code> is.
            </p>
            <p>
              Needs a Rust toolchain at 1.88 or newer; the binary lands in{" "}
              <code>~/.cargo/bin</code>. Note that{" "}
              <code>cargo install axio</code> from crates.io does{" "}
              <strong>not</strong> work — that name belongs to an unrelated
              crate.
            </p>
          </>
        }
        evidence={
          <>
            <Term label="from the repository" wrap>{`
cargo install --git ${REPO} --locked axio
`}</Term>
            <Term label="from a clone" wrap>{`
cargo install --path crates/axio --locked
`}</Term>
          </>
        }
      />

      <Spread
        claim={
          <>
            <h2>One binary, two surfaces.</h2>
            <p>
              Pipe into it or pass <code>-p</code> and you get a single turn.
              Run it on a terminal with neither and you get the composer.
            </p>
            <p>
              <code>--doctor</code> answers from configuration alone — no
              credential, no socket, safe anywhere. <code>--probe</code>{" "}is the
              opposite by design: it sends two short requests, one carrying a
              tool, and reports whether the tool was accepted. A model can serve
              chat perfectly and reject every request that offers it a tool, and
              nothing in your configuration is wrong when that happens. Only
              asking the model finds it.
            </p>
          </>
        }
        evidence={
          <Term label="the surface you get">{`
axio -p "explain this repo"
cat src/lib.rs | axio -p "review this"

axio                      «# interactive, on a terminal»
axio --list               «# recent sessions»
axio --resume 01K3F       «# a unique prefix is enough»

axio --doctor             «# what axio can see, offline»
axio --probe              «# does this model accept tools»
axio --explain model.effort   «# where a setting came from»
`}</Term>
        }
      />

      <Spread
        claim={
          <>
            <h2>Four providers, three dialects.</h2>
            <p>
              Not a plugin system. <code>anthropic</code> speaks the Messages
              API; <code>ollama</code> and <code>openai-compatible</code> share
              chat-completions; <code>openai-codex</code> speaks Responses and is
              signed in to through the browser rather than pasted into.
            </p>
            <p>
              The model list is fetched from the provider, never compiled in: a
              name missing from a built-in catalogue looks exactly like a name
              the provider refuses. A provider with no credential stays on the
              list and cannot be chosen — removing it would read as one axio
              cannot reach, when the answer is <code>/login</code> away.
            </p>
          </>
        }
        evidence={
          <Term label="/model, choosing a provider">{`
›  1. ollama          ‹✓›
   2. anthropic         «not configured»
   3. openai-codex
   4. openai-compatible

«↑ ↓ move · 1–9 pick · enter run · esc dismiss»
`}</Term>
        }
      />

      <Spread
        claim={
          <>
            <h2>One config path, everywhere.</h2>
            <p>
              Layered: defaults, then <code>~/.axio/config.toml</code>, then the
              nearest project <code>.axio/config.toml</code>, then{" "}
              <code>AXIO_*</code>{" "}variables, then flags. A project&apos;s own
              file may only add restrictions, never remove them.
            </p>
            <p>
              <code>~/.axio</code>{" "}is the same path on Windows, WSL, Linux and
              macOS rather than each platform&apos;s own configuration directory
              — one path to document, to sync between machines, and to name in a
              bug report.
            </p>
          </>
        }
        evidence={
          <Term label="~/.axio/config.toml">{`
«[model]»
effort = "xhigh"

«[budget]»
max_usd_per_turn = 2.0
max_steps = 50

«[permissions]»
deny = ["bash:curl"]

«[sandbox]»          «# Linux only, off by default»
enabled = true
`}</Term>
        }
      />

      <section className="ledger-block">
        <div className="ledger-head">
          <h2>What has actually been run.</h2>
          <p>
            Against a live endpoint, as opposed to built to a documented spec.
            These are not the same claim, so they are not written as though they
            were.
          </p>
        </div>
        <table className="ledger">
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
                Signed in through the browser flow, then the same thirteen checks
                green. Its request body took two live 400s to settle.
              </td>
            </tr>
            <tr>
              <th scope="row">Messages transport</th>
              <td className="state state--unmet">not met</td>
              <td>
                Built from the documented wire format and snapshot-tested against
                it. There has been no credential to run it with, which is not the
                same as having met the endpoint.
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
      </section>

      <Spread
        claim={
          <>
            <h2>It runs model-written code on your machine.</h2>
            <p>
              By default there is no sandbox: confinement is the workspace root,
              the approval prompt and process-group containment.
            </p>
            <p>
              On Linux, <code>--sandbox</code>{" "}adds Landlock — the kernel&apos;s
              own, inherited by every command axio spawns. It says nothing about
              the network, and it is a second wall behind the permission engine
              rather than a replacement for it.
            </p>
            <p>
              <a href={`${REPO}/blob/main/SECURITY.md`}>Read SECURITY.md</a>{" "}
              before running it anywhere that matters.
            </p>
          </>
        }
        evidence={
          <Term label="--sandbox, on Linux">{`
workspace   «writable»
system      «readable»
~/.ssh      ‹not there at all›
network     «unconstrained»
`}</Term>
        }
      />

      <footer className="colophon">
        <div className="colophon__links">
          <a href={REPO}>Repository</a>
          <a href={`${REPO}/blob/main/docs/architecture.md`}>Architecture</a>
          <a href={`${REPO}/blob/main/docs/gotchas.md`}>Gotchas</a>
          <a href={`${REPO}/blob/main/docs/roadmap.md`}>Roadmap</a>
          <a href={`${REPO}/blob/main/CHANGELOG.md`}>Changelog</a>
        </div>
        <div className="colophon__meta">
          <span>Apache-2.0 · a product of Umbra</span>
          <ThemeToggle />
        </div>
      </footer>
    </main>
  );
}
