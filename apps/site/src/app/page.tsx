import HeroTranscript from "./HeroTranscript";
import Surface from "./Surface";
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
            <i aria-hidden="true" />
            axio
          </a>
          <nav className="header__nav">
            <a className="header__nav-link" href="#install">
              Install
            </a>
            <a className="header__nav-link" href="#counts">
              Quota
            </a>
            <a className="header__nav-link" href="#verification">
              Verification
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
          <div className="container">
            {/* The panel is first in the DOM and the slab second. Both are
                placed by an explicit grid-area on desktop, so order does not
                decide the overlap there — but below 64rem the two simply stack,
                and source order is the whole layout. Reversed, the page opened
                on a decorative illustration and reached the headline a screen
                later. */}
            <div className="hero__stage">
              <div className="hero__glass">
                <span className="eyebrow label">
                  <i aria-hidden="true" />
                  Pre-release · nothing tagged
                </span>
                <h1 className="display">
                  Many agents.
                  <em>One queue.</em>
                </h1>
                <p className="hero__sub">
                  A local-first coding agent in Rust. Every supervised session
                  gets its own git worktree and branch, so an agent edits an
                  isolated checkout rather than the one you are working in — and
                  every approval they need arrives in one place.
                </p>
                <div className="hero__actions">
                  <a className="btn btn--primary" href="#install">
                    Install it
                  </a>
                  <a className="btn btn--ghost" href="#verification">
                    See what has been verified
                  </a>
                </div>
              </div>

              <Surface className="hero__surface" />
            </div>
          </div>
        </section>

        {/* The application's status bar rather than a row of stat tiles. The
            count is the fact and the noun is the caption, so they are not the
            same colour — and the one figure that is not neutral does not read
            as neutral. */}
        <div className="strip">
          <div className="container">
            <dl>
              <div>
                <dt>9</dt>
                <dd>crates</dd>
              </div>
              <div>
                <dt>6</dt>
                <dd>tools</dd>
              </div>
              <div>
                <dt>4</dt>
                <dd>providers over 3 dialects</dd>
              </div>
              <div>
                <dt>23</dt>
                <dd>agents priced</dd>
              </div>
              <div data-state="warn">
                <dt>0</dt>
                <dd>releases tagged</dd>
              </div>
            </dl>
          </div>
        </div>

        <section className="section">
          <div className="container">
            <div className="section__head">
              <span className="label">One turn</span>
              <h2 className="display">Watch one run.</h2>
              <p>
                The transcript is printed into your terminal&apos;s own
                scrollback, so it survives the process, scrolls with the
                scrollbar and copies with the mouse. Only the live part — the
                status line, the composer, the question being asked — is
                redrawn.
              </p>
            </div>
            <HeroTranscript />
          </div>
        </section>

        <section className="section">
          <div className="container">
            <div className="section__head">
              <span className="label">How it behaves</span>
              <h2 className="display">Predictable where it matters.</h2>
              <p>
                Six behaviours you can check from the outside, each named by the
                string the product actually prints.
              </p>
            </div>
            <div className="cards">
              <article className="card">
                <span className="card__artifact">allow? y a n</span>
                <h3>Reads don&apos;t ask. Writes do.</h3>
                <p>
                  The diff or the command lands in scrollback first, then the
                  viewport asks. A shell command is shown as the string the
                  shell actually receives, never a word-split of it — the split
                  reads as a simpler command than the one that runs.
                </p>
              </article>
              <article className="card">
                <span className="card__artifact">exit 5</span>
                <h3>Refusals have an exit code.</h3>
                <p>
                  In a one-shot run there is nobody to ask, so those actions are
                  refused unless <code>--yes</code> was given. A turn that
                  completed with something refused exits <code>5</code>, so{" "}
                  <code>&amp;&amp;</code> sees it.
                </p>
              </article>
              <article className="card">
                <span className="card__artifact">.axio/config.toml</span>
                <h3>A project adds rules. It never removes them.</h3>
                <p>
                  That covers <code>[permissions] allow</code>, and it covers{" "}
                  <code>[worktree] enabled = false</code> — which does not look
                  like a permission, and is the point. A repository that could
                  switch worktrees off would be deciding, for everyone who
                  cloned it, that its agents may write to your working tree.
                </p>
              </article>
              <article className="card">
                <span className="card__artifact">--probe</span>
                <h3>Ask the model, not the config.</h3>
                <p>
                  It sends two short requests, one carrying a tool. A model can
                  serve chat perfectly and reject every request that offers it a
                  tool; nothing in your configuration is wrong when that
                  happens, and only asking finds it.
                </p>
              </article>
              <article className="card">
                <span className="card__artifact">~/.axio</span>
                <h3>One config path, everywhere.</h3>
                <p>
                  The same on Windows, WSL, Linux and macOS rather than each
                  platform&apos;s own directory. One path to document, to sync
                  between machines, and to name in a bug report.
                </p>
              </article>
              <article className="card">
                <span className="card__artifact">--sandbox</span>
                <h3>A second wall on Linux.</h3>
                <p>
                  Landlock, inherited by every command axio spawns. It says
                  nothing about the network, and it stands behind the permission
                  engine rather than replacing it.
                </p>
              </article>
            </div>
          </div>
        </section>

        <section className="section" id="counts">
          <div className="container">
            <div className="section__head">
              <span className="label">Quota and cost</span>
              <h2 className="display">
                What you have left. What you already spent.
              </h2>
              <p>
                Two commands that never take a turn. Neither reads axio&apos;s
                own stored credentials — a usage probe and an agent turn are
                different trust boundaries, so they are kept apart in the code
                as well as in the sentence.
              </p>
            </div>
            <div className="duo">
              <div>
                <h3>axio quota</h3>
                <p>
                  How much of each provider&apos;s limit is left and when it
                  resets, across ten providers. Six are read from a credential
                  another tool already wrote, so they need no configuration at
                  all.
                </p>
                <Term label="provider limits">{`
> axio quota
Codex (pro)
  Weekly                        «22% used  resets in 5d»
Claude (max)
  5h                             «8% used  resets in 2h»
  Weekly                         «2% used  resets in 6d»
  Weekly (Fable)                 «0% used»
`}</Term>
              </div>
              <div>
                <h3>axio cost</h3>
                <p>
                  What every coding agent on this machine has spent, read from
                  the transcripts they already write. No network, no
                  credentials. A model with no known rate is reported unpriced,
                  never as zero.
                </p>
                <Term label="local spend" wrap>{`
$ axio cost --by client
$ axio cost --calendar   «the year, shaded»
$ axio cost --wide       «cache share, $/M, share»
$ axio cost --diagnose   «what each parser skipped»
`}</Term>
              </div>
            </div>
          </div>
        </section>

        <section className="section" id="verification">
          <div className="container">
            <div className="section__head">
              <span className="label">Verification</span>
              <h2 className="display">What has actually been run.</h2>
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
                    <th scope="col" className="label">
                      Path
                    </th>
                    <th scope="col" className="label">
                      State
                    </th>
                    <th scope="col" className="label">
                      Basis
                    </th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <th scope="row">Chat-completions transport</th>
                    <td className="state state--met">met</td>
                    <td>
                      Thirteen checks over seven turns, end to end against a
                      real model, including on Windows.
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
              <span className="label">Install</span>
              <h2 className="display">Build it from main.</h2>
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
          {/* data-umbra-link is what the shared collector watches for. The
              press is counted as an hourly total against this site and this
              link name, with no visitor identifier, and the navigation is
              never delayed waiting for it. */}
          <span>
            Apache-2.0 ·{" "}
            <a href="https://umbra.me" data-umbra-link="umbra-attribution">
              a product of Umbra
            </a>
          </span>
        </div>
      </footer>
    </>
  );
}
