import HeroTranscript, { ScrollbackBackdrop } from "./HeroTranscript";
import Term from "./Term";

const REPO = "https://github.com/umbra-me/axio";

export default function Page() {
  return (
    <>
      {/* Four header links and a nav stand between a keyboard or screen-reader
          visitor and the page. Hidden until focused, first in the tab order. */}
      <a className="skip-link" href="#content">
        Skip to content
      </a>

      <header className="header">
        <div className="container header__row">
          <a className="wordmark" href="/">
            <span className="wordmark__mark" aria-hidden="true">
              a
            </span>
            axio
          </a>
          <nav className="header__nav">
            <a className="header__nav-link" href="#install">
              Install
            </a>
            <a className="header__nav-link" href="#verification">
              Verification
            </a>
            <a
              className="header__nav-link"
              href={`${REPO}/blob/main/docs/architecture.md`}
            >
              Docs
            </a>
            <a className="btn btn--ghost" href={REPO}>
              Repository →
            </a>
          </nav>
        </div>
      </header>

      {/* tabIndex -1 so the skip link actually moves focus here rather than
          only scrolling, which is the difference between the link working for a
          screen reader and appearing to. */}
      <main id="content" tabIndex={-1}>
        <section className="hero">
          <div className="hero__glow" aria-hidden="true">
            <span />
            <span />
          </div>
          <ScrollbackBackdrop />

          <div className="container">
            <div className="hero__body">
              <span className="pill">
                <span className="pill__dot" />
                Pre-release · nothing tagged
              </span>
              <h1>
                It stays in your <span className="gradient-text">terminal.</span>
              </h1>
              <p className="hero__sub">
                A cross-platform AI coding agent in Rust. One binary, two
                surfaces — a one-shot CLI when you pipe into it, an inline
                composer when you don&apos;t. It prints into your scrollback
                instead of taking the screen.
              </p>
              <div className="hero__actions">
                <a className="btn btn--primary" href="#install">
                  Install it
                </a>
                <a className="btn btn--ghost" href="#verification">
                  What has been verified
                </a>
              </div>
            </div>
          </div>
        </section>

        <div className="container">
          <dl className="stats">
            <div>
              <dt>4</dt>
              <dd>Crates</dd>
            </div>
            <div>
              <dt>3</dt>
              <dd>Wire dialects</dd>
            </div>
            <div>
              <dt>0</dt>
              <dd>Releases cut</dd>
            </div>
            <div>
              <dt>Apache-2.0</dt>
              <dd>Open source</dd>
            </div>
          </dl>
        </div>

        <section className="section">
          <div className="container">
            <div className="section__head">
              <span className="section-label">A turn</span>
              <h2>Watch one run.</h2>
              <p>
                The transcript is printed into your terminal&apos;s own
                scrollback, so it survives the process, scrolls with the
                scrollbar and copies with the mouse. Only the live part is
                redrawn.
              </p>
            </div>
            <HeroTranscript />
          </div>
        </section>

        <section className="section">
          <div className="container">
            <div className="section__head">
              <span className="section-label">How it behaves</span>
              <h2>Predictable where it matters.</h2>
            </div>
            <div className="cards">
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  ⏺
                </div>
                <h3>Reads don&apos;t ask. Writes do.</h3>
                <p>
                  The diff or the command lands in scrollback first, then the
                  viewport asks. A shell command is shown as the string the shell
                  actually receives, never a word-split of it.
                </p>
              </article>
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  5
                </div>
                <h3>Refusals have an exit code.</h3>
                <p>
                  In a one-shot run there is nobody to ask, so those actions are
                  refused unless <code>--yes</code> was given. A turn that
                  completed with something refused exits <code>5</code>, so{" "}
                  <code>&amp;&amp;</code> sees it.
                </p>
              </article>
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  ⇄
                </div>
                <h3>Four providers, three dialects.</h3>
                <p>
                  Not a plugin system. The model list is fetched from the
                  provider, never compiled in — a name missing from a built-in
                  catalogue looks exactly like a name the provider refuses.
                </p>
              </article>
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  ~
                </div>
                <h3>One config path, everywhere.</h3>
                <p>
                  <code>~/.axio</code> is the same on Windows, WSL, Linux and
                  macOS rather than each platform&apos;s own directory. One path
                  to document, to sync between machines, and to name in a bug
                  report.
                </p>
              </article>
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  ▣
                </div>
                <h3>A sandbox on Linux.</h3>
                <p>
                  <code>--sandbox</code> adds Landlock, inherited by every
                  command axio spawns. It says nothing about the network, and it
                  is a second wall behind the permission engine rather than a
                  replacement for it.
                </p>
              </article>
              <article className="card">
                <div className="card__icon" aria-hidden="true">
                  ?
                </div>
                <h3>Ask the model, not the config.</h3>
                <p>
                  <code>--probe</code> sends two short requests, one carrying a
                  tool. A model can serve chat perfectly and reject every request
                  that offers it a tool; nothing in your configuration is wrong
                  when that happens, and only asking finds it.
                </p>
              </article>
            </div>
          </div>
        </section>

        <section className="section" id="verification">
          <div className="container">
            <div className="section__head">
              <span className="section-label">Verification</span>
              <h2>What has actually been run.</h2>
              <p>
                Against a live endpoint, as opposed to built to a documented
                spec. These are not the same claim, so they are not written as
                though they were.
              </p>
            </div>
            <div className="ledger-card">
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
                      Signed in through the browser flow, then the same thirteen
                      checks green. Its request body took two live 400s to
                      settle.
                    </td>
                  </tr>
                  <tr>
                    <th scope="row">Messages transport</th>
                    <td className="state state--unmet">not met</td>
                    <td>
                      Built from the documented wire format and snapshot-tested
                      against it. There has been no credential to run it with,
                      which is not the same as having met the endpoint.
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
            </div>
          </div>
        </section>

        <section className="section" id="install">
          <div className="container">
            <div className="section__head">
              <span className="section-label">Install</span>
              <h2>Build it from main.</h2>
              <p>
                There is no published binary yet — releases are built by tag and
                none has been cut. Needs a Rust toolchain at 1.88 or newer; the
                binary lands in <code>~/.cargo/bin</code>.
              </p>
            </div>
            <div className="install">
              <Term label="macOS · Linux · WSL" wrap copy="macOS, Linux and WSL install">{`
curl -fsSL https://axio.sh/install | sh
`}</Term>
              <Term label="Windows · PowerShell" wrap copy="Windows PowerShell install">{`
irm https://axio.sh/install.ps1 | iex
`}</Term>
              <Term label="or drive cargo yourself" wrap copy="cargo install">{`
cargo install --git ${REPO} --locked axio
`}</Term>
            </div>
            <p className="note">
              The installer checks for a toolchain, refuses politely if it is
              missing or older than 1.88, and installs into your cargo bin as
              your own user — no sudo, nothing written outside{" "}
              <code>CARGO_HOME</code>, no changes to your shell profile.{" "}
              <a href="/install">Read it first</a>; that advice applies to every
              script anyone asks you to pipe into a shell, including this one.
            </p>
            <p className="note">
              <code>cargo install axio</code> from crates.io does{" "}
              <strong>not</strong> work — that name belongs to an unrelated
              crate.
            </p>
          </div>
        </section>
      </main>

      <footer className="footer">
        <div className="container footer__row">
          <div className="footer__links">
            <a href={REPO}>Repository</a>
            <a href={`${REPO}/blob/main/docs/architecture.md`}>Architecture</a>
            <a href={`${REPO}/blob/main/docs/gotchas.md`}>Gotchas</a>
            <a href={`${REPO}/blob/main/docs/roadmap.md`}>Roadmap</a>
            <a href={`${REPO}/blob/main/SECURITY.md`}>Security</a>
          </div>
          <span>Apache-2.0 · a product of Umbra</span>
        </div>
      </footer>
    </>
  );
}
