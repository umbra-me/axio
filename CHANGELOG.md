# Changelog

Notable changes per release. Dates are the release date; the format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[SemVer](https://semver.org/spec/v2.0.0.html) — with the usual `0.x` caveat that
a minor bump may break things.

## [Unreleased]

- September 5: Local-profile launch integration, versioned capture imports, desktop CI and installer packaging with flat release artifacts. See [change set](docs/change-set-2026-09-05.md).

### Added

- **Hosted agents report their own state, by their hooks.** Claude Code and
  Codex are launched with hooks on their command line — `--settings` for
  Claude Code, `-c notify=…` for Codex — that report to a loopback listener
  the window runs, through one script under `~/.axio/hooks`. Nothing is
  written into the person's own configuration and a session the window did
  not start runs no hook. The rail, tabs and cards now say *needs you* when
  an agent is blocked on a permission, *done* when its turn ended, and the
  dock badge counts blocked agents with the questions waiting. Any other
  tool can print `ESC ] 9999 ; {"status": …} BEL` and be understood the
  same way. The quiet-timer mark stays as the fallback for tools with none.
- **Resume is exact.** The hooks carry the tool's own session id, kept in
  the journal; a resume passes `--resume <id>` to Claude Code and
  `resume <id>` to Codex instead of "the most recent one in this directory".
  Codex's notify names a thread that is not its session, so its id is read
  from the rollout it writes under `~/.codex/sessions`, found by the
  worktree it ran in — which also gives its chat view a transcript. A Codex
  with no session known starts fresh in its worktree rather than resuming
  "the last session anywhere", which `resume --last` means.
- **A resumed process shows in the terminal that was already open.** An
  emulator whose cursor is past the new process's output resets and reads
  from the start, rather than showing the old screen forever.
- **A terminal's chat view.** Once an agent has named its transcript, the
  terminal's header menu offers *Its transcript, as a chat*: the agent's own
  JSONL folded into the same rows a session shows, kept current as it writes.
  Claude Code's transcript and Codex's rollout are both read.
- **Codex through its own protocol.** *Codex, structured* in the plan (or
  `codex-app` in the plan line) drives Codex through `codex app-server`
  instead of a terminal: rows stream into the card as it works, and its
  questions — run this command, make this edit — are answered from the
  card with *Allow once*, *Allow this session* or *Deny*. Model, effort and
  permission mode go as protocol fields. It resumes by thread id. The
  terminal stays the way in for every other tool.
- **A first prompt goes on the command line** for Claude Code and Codex,
  which take one there and still open their interface; nothing is typed
  into them after a quiet timer any more. Pi is still typed at.
- **Permission mode in the plan.** A fifth column: *ask*, *edits*, *auto* or
  *full*, mapped to each tool's own flags (`--permission-mode` and
  `--dangerously-skip-permissions` for Claude Code, `-a`/`-s` and
  `--full-auto` for Codex) and dropped where a tool has none.
- **The window remembers how it was looking at things.** Layouts, card
  sizes, each terminal's view and the open tabs are kept in
  `~/.axio/window.json`, read field by field so one bad record costs only
  itself, and restored once the first snapshot says what still exists.
- **A setup command per repository.** `[worktree] setup = "pnpm install"` in
  a repository's `.axio/config.toml` (or the user's, as a default) runs in
  every fresh worktree before its agent starts; a failure fails the start
  with the last lines it printed.
- **Every turn has a checkpoint either side**, as hidden refs under
  `refs/axio/checkpoints/<session>/<turn>/`, taken from a throwaway index
  so the agent's own index is untouched and untracked files count. The
  Changes view gains a picker: all changes, or one turn's.
- **Reattach is clean.** A terminal read from the start — a fresh emulator
  catching up — is stripped of the queries the program asked on its way up
  (device attributes, cursor position, mode and colour queries), so the
  emulator does not answer them all over again into the program's stdin.
- **Notifications do not burst.** One within two seconds of another is
  dropped.
- **A terminal you stopped stays stopped.** Stopping one is a decision and
  is remembered as one, so the next window lists it and waits to be asked;
  only a terminal the window itself interrupted — by closing — comes back
  running. The rail says *stopped* rather than *ended*, and its pane says it
  will stay that way until you resume it.
- **Remembered terminals come back running.** A new window resumes every
  terminal the last one had, each in its own worktree with its tool asked
  to continue, rather than listing them ended to be clicked one by one.
  *On launch* in Settings › Terminal (`[terminal] resume_on_launch`) turns
  that off. One whose directory is gone stays ended and says so.
- **Hosted terminals survive the window.** What each terminal was — its
  harness, worktree, branch, name, group and arguments — is journaled to
  `~/.axio/terminals.json` as it changes, and the next window lists every one
  of them, ended, where it was. **Resume** starts the tool again in the same
  worktree and asks it to continue its own conversation (`claude --continue`,
  `codex resume --last`, `pi --continue`; axio starts fresh there, since its
  `--resume` wants an id). A process cannot outlive its owner; the work is a
  directory and a branch, and those can. **Stop** now keeps the row — it is
  the thing a resume needs — and **Remove from list** is the one that forgets.
  The pane of an ended terminal says so above its last screen, with both.
- **Every agent on the composer gets a count, and none is required.** The
  start bar used to hold axio at one and offer the other agents as on-or-off
  chips, so "two Claude Codes and no axio" could not be asked for; it could
  only be reached by ticking Claude Code first and then stepping axio down,
  which nobody found. Each agent now has its own stepper, axio included, from
  zero to eight; Start counts the total.
- **A terminal is a thread under its repository, and a repository opens side
  by side.** Hosted terminals used to sit in a pile of their own at the bottom
  of the rail, and only axio sessions were listed under the repository they
  worked in. Every terminal now carries the repository its directory belongs
  to, and the rail lists it there — with the sessions, inside its group when
  it was started in one — each repository folding away on its chevron. The
  repository's name folds and unfolds it; *Open side by side* on its menu,
  or the palette, opens it as a tab — the same view a group gets, over
  everything running in that repository whether it was started together or
  not, with **Add** starting one more session or terminal there. Terminals
  started somewhere no listed repository owns appear under *Elsewhere*,
  which is drawn only when there are any.
- **Terminals get the "done" mark too.** A hosted terminal that wrote while
  it was not on screen is marked *new output* in the rail and its tab, the
  way a session that finished a turn is marked *done*. The mark is set once
  the output has been quiet for a moment, and clears when the terminal — or
  the group or repository pane showing it — is in front.
- **A session started without a prompt is labelled by its first.** It was
  "(no prompt)" forever, whatever it was asked later. The first prompt sent
  to it now becomes its label, written to the index as its own record so it
  survives a restart; a session that had a label keeps it.
- **A launch is a plan: models, effort, arguments, columns.** Under the
  composer's steppers, *Plan* opens a row per member — agent, model,
  reasoning effort, extra arguments, and the column it sits in — and one
  line that writes the rows: `2x claude --model opus | codex -m gpt-5.4
  --effort high, pi` is two Claude Codes stacked on the left and a Codex
  over a Pi on the right. Models go to each tool as its own flag; effort
  goes where a tool takes one on its command line (Codex) and is dropped,
  not passed, elsewhere. The columns become the pane's layout.
- **Split panes.** A group or repository pane can be laid out as a tree of
  splits instead of a grid: *Split* in its header, then drag a card's
  header onto another's edge to split it there, onto its middle to swap,
  and drag the dividers. *Grid* goes back. The layout is the window's, per
  pane, and follows members as they come and go.
- **A master box, and cards that listen.** Every group and repository pane
  has one box at its foot that types into every member at once — a session
  gets it as a prompt, a terminal as a line — and every card has a listen
  toggle in its header to take itself out.
- **The card's box drives the agent's menus.** Sending a slash command puts
  the box into relay: every key — arrows, Enter, Esc, letters to filter —
  goes straight to the agent, so Pi's `/model` picker is filtered and chosen
  from the box that opened it. A pill says so; Enter or Esc end relay (and
  reach the agent), clicking the pill ends it silently. An empty box always
  hands the steering keys through — arrows, Enter, Esc, Tab, Ctrl-letters —
  while letters keep typing into it.
- **A terminal's card is the terminal.** The card overview used to describe
  the agent — "its own interface, typed below goes to its prompt" — and show
  nothing of it. It now holds the agent's real interface, live, with every
  menu and selection it draws: `/model` in Pi lists the models and the
  arrows pick one, as in its own terminal. Our follow-up box sits under it,
  and the two are the same prompt seen twice.
- **The card's follow-up box knows the agent's slash commands.** Typing `/`
  into it lists what that agent's own interface answers to — the built-in
  set per harness, plus for Claude Code every `.claude/commands/*.md` and
  `.claude/skills/*/SKILL.md` in the home directory and the repository —
  filtered as you type, chosen with the arrows, Tab or Enter. What is sent is
  typed into the agent's prompt and then, after a pause, submitted, so
  `/model` opens the agent's own menu on the command and the Enter chooses
  it; sent in one burst it was a paste, and a paste is not submitted.
- **A terminal in its own tab is a card, or a terminal, or both.** It opens
  as the card a group shows — state, a typed follow-up, the branch and
  directory — filling the pane. Its header menu offers three views:
  *Card overview*, *Terminal, with this header* (the real terminal for the
  card's body), and *Plain terminal* (edge to edge, no chrome, as the tab
  was before; right-click the terminal to get the menu back). The choice is
  the terminal's own for as long as it is listed, and a new terminal opens
  in the view `[terminal] view` names in `~/.axio/app.toml` — *Opens as* in
  Settings › Terminal, `card` by default. Inside a group each terminal card
  offers the first two, overriding the pane's Overview / Live switch for that
  card alone.
- **Adding goes where the thing is added.** The *New* and *Repository*
  buttons above the list are gone. A repository is added from the `+` on the
  *Repositories* heading, beside the closed-sessions toggle; work is started
  from the `+` on the repository it is for, which drops the same menu *New*
  had — a session or group in the composer, or an agent in a terminal — and
  starts it in that repository rather than in whichever the composer last
  had selected.
- **Worktrees and branches have names you can read.** A session's branch was
  its ULID — `axio/01k4grpc2q8…` — unique and unsayable, and a rail of them
  was a rail of look-alikes. Each worktree is now `axio/<quality>-<thing>`
  from two lists of fifty in the supervisor's `names` module: 2,500 names on
  the umbra and iconoclast themes, `umbral-heretic`, `waning-comet`,
  `unbowed-penumbra`. The name is picked, from a random start, among those no
  branch of the repository already carries, and a collision between two
  sessions started in the same instant is picked again rather than failed.
  The ULID comes back only as a suffix once every name is in use.
- **A start needs no prompt.** The composer used to refuse to start anything
  until something was typed, which made "open two Claude Codes and work in
  them" impossible from the window. Start is now enabled whenever there is
  something to start: a session without a prompt starts and waits for its
  first turn, a terminal opens with the tool at its own prompt, and one
  terminal on its own opens as itself rather than as a group of one.
- **`axio app` opens the desktop surface from the command line.** The window
  used to be reachable only by starting its binary by hand; the command now
  finds `axio-app` beside the running `axio` (then on `PATH`), starts it and
  returns, so a turn in the terminal and the window are the same invocation
  apart. The desktop build itself is unchanged: still the frontend under
  `crates/axio-app/ui`, then `cargo build --release -p axio-app --features
  app`. When the binary is not there, the command says so and names the two
  lines that put it there.

### Fixed

- **The glass was not real on macOS.** A transparent window there sits behind
  Tauri's `macOSPrivateApi` flag, and without it `transparent: true` is
  ignored: the window under the stylesheet's glass tints was opaque, so the
  surface came out flat black, and the vibrancy material the macOS overlay
  named had nothing to show through. The flag is set and the matching cargo
  feature is on the `tauri` dependency line, where `tauri-build` looks for it.
- **The composer sat at the top of an empty pane.** The tab strip above it
  renders nothing when there are no tabs, so the pane moved up into the
  strip's `auto` grid row, was sized to its content, and the centring rule had
  no room to work in. The pane is pinned to its own row; the composer is in
  the middle of the page, as the stylesheet had said all along.
- **The macOS traffic lights hung below the title bar.** The bar was 22px,
  built on the belief that macOS centres the lights in a 22pt overlay bar; it
  centres them about 15pt down, so they sat against the bar's bottom edge and
  below the wordmark. The bar is 30px on macOS, sized to where the lights
  actually are, since `trafficLightPosition` still does nothing there.

### Changed

- **The window has less chrome, and the chrome it has says one thing.** A
  session used to sit under three bars — the title bar, the tab strip and a
  session bar — that each named the project, and an empty tab strip stayed on
  screen holding a `+` the rail already had. The strip is now the pane's only
  toolbar: the front tab's controls (transcript or changes, close, discard, or
  stop for a terminal) sit at its right end, the branch, model and cost of the
  session in front moved to the status bar, and the strip is not drawn until
  something is open. The status bar stopped repeating the rail's counts and
  shows what the rail cannot: the session in front, how many are working, and
  how many are waiting on you.

  The rail is 260px, hides with `⌘B` (or the toggle by the wordmark), and lost
  its footer tagline. Session rows carry a word only where the dot cannot —
  "needs you", "done", "closed" — rather than beside every row. Two buttons
  head the rail: **New**, a menu offering an axio session in its own worktree
  and each agent this machine can host in a terminal, and **Repository**. The
  row of launcher chips under Terminals and the `+` in the Repositories header
  are gone; those two buttons are the one place things are added, and the
  keyboard still reaches all of it. The opening pane is the composer and one line: the
  repository picker moved into the composer's footer and the paragraph of
  explanation became its placeholder. In a session, Stop takes the Send
  button's slot while a turn runs rather than a strip of its own above the
  textarea, and the streaming caret sits at the end of the last line instead
  of on a line beneath it.

- **`n` in the terminal's approval prompt opens a note.** The window sent what
  was typed with a refusal; the terminal sent a fixed sentence, so the claim
  that refusing steers the next step was true on one surface. `n` now takes a
  line in the composer, Enter sends it as the refusal's feedback — the tool
  result the model reads next — and Enter on an empty line, or Esc, refuses
  plainly as before. The note is recorded in scrollback under the verdict.

- **The desktop window and the quota tray wear the family's colours.**
  `axio-app` now ships the standard icon set — a geometric single-storey `a`
  in the accent on the family's slate tile, generated from
  `crates/axio-app/icons/source.svg` — where it had only a Windows `.ico`,
  and `tauri.conf.json` lists the five files. The quota tray's ring gauge
  stays a gauge and moves from amber to the accent the website and the
  other products use.

- **axio.sh now wears the desktop application's design system rather than the
  house one.** The site was built in the Umbra language with amber as its
  per-product hue, which was the right answer when the only surface was a
  terminal and there was no application to disagree with. There is one now, and
  it settled the question differently, so the website follows it: one product
  with two surfaces cannot credibly wear two identities.

  The thesis is ported rather than paraphrased. `tokens.css` says the chrome is
  glass and the content is slate, and the page is built on that — header,
  section labels and the panel the headline sits in are translucent and float,
  while the transcript, the ledger and the install commands are dense slabs
  sitting on top. The application's glass is translucent to your desktop and a
  web page has no wallpaper, so the hero supplies its own: a drawn slab of the
  application's surface, with the headline in a glass panel lapping its left
  edge. The lap is capped at 1rem, because the rail starts at that edge and a
  larger one hides the four agent colours the hero exists to show.

  The palette is the application's — `#7ba0ff` for axio and one colour per agent
  it can host — so "this belongs to an agent" and "this belongs to axio" stay
  different questions on the page as they are in the window. The amber gradient
  is gone; the application has no gradient anywhere. The display face is now
  Geist Mono, since this product's material is monospaced output, and its
  roughly doubled advance is a real constraint on headline length rather than a
  reason to shrink the type.

  The content was as stale as the paint. The page claimed four crates and
  described a terminal-only tool; there are nine, three binaries, a supervisor,
  a desktop window, quota across ten providers and cost across twenty-three
  agents, and none of the last four were mentioned anywhere on it. The
  verification ledger gained no rows: what the desktop surface has actually been
  run against is not written down, and that table's whole value is that it does
  not guess.

- **The desktop surface is glass, and the glass is real.** The stylesheet had
  described translucent chrome over a window that never set `transparent`, so
  every blur in it was compositing one flat colour against another — no effect,
  and a pass per element per frame. The compositor does it now, and the window
  is genuinely translucent to the desktop behind it.

  The material inverts on purpose: chrome floats, content does not. Title bar,
  rail, toolbar and status bar sit at 0.56–0.66 alpha, while the panes you
  actually read — a diff, a terminal — stay at 0.84 and 0.93, because code
  judged against a moving background is code judged badly. Acrylic samples the
  wallpaper, which would make text contrast a property of somebody's desktop
  picture, so a dark tint is pinned underneath and every ratio holds against a
  white background image.

  Behind it, the system the tokens had promised and the call sites had been
  ignoring: one 4px spacing rhythm rather than 4/7/9/12/16/22, six type steps
  from 11px to 30px rather than eleven sizes inside a 5px band, dividers at an
  alpha that can be seen, one motion curve, and icons drawn on a 16-unit grid
  where the window controls had been font glyphs.

### Added

- **A group of agents on one prompt.** The composer's footer counts axio
  sessions (`1× axio`, up to eight) and ticks the other agents this machine
  has; Start with more than one becomes `start_group`, which starts each in a
  worktree of its own and tags them with one group id. The rail shows the
  group as one row with its members under it, and the group's tab is a compare
  view — one card per member with its state, branch and diff side by side, the
  card's header opening the member itself. The compare view opens on
  **Overview** — a wall of summary cards, as many across as fit: state, the
  last thing the agent said, an inline follow-up that sends (or types into a
  hosted agent's prompt), and model, cost and branch underneath. **Live**
  shows each member whole — a hosted member is its real terminal at card
  size, a session its streaming transcript — and **Changes** swaps every card
  to its diff. Each card has a size: drag its corner to any width and height,
  or pick small, wide, large or full from its ⋯ menu or the resize control in
  its header; cards flow around whatever mixture results. A member's pending
  question is answered on its card. **Add**
  in its header puts one more member in: an axio session or any agent this
  machine has, in a worktree of its own, asked the group's prompt. A card's
  header is inert — clicking it no longer pulls the view to that member — and
  its ⋯ (or right-click) carries Expand plus the rail row's own menu: rename,
  reveal, open in editor, copy path or branch, close, discard, stop. Nothing else is pooled: each
  member has its own approvals, transcript and branch, and is closed on its
  own. The group id is a field in the session index (`group`), recorded at
  start and never after. A hosted member is typed at only once its interface
  has painted — output arrived, then went quiet — and the Enter follows the
  text after a pause as its own write: sent at spawn, the text landed in the
  composer and the Enter was lost; sent in one burst, several agents treat it
  as a paste and submit nothing.

- **Landing, in the window.** Above a session's diff: the branch, the branch
  the repository is on, commits ahead and files uncommitted, and the three
  ways this surface lands work — merge into the repository's branch, push,
  or push and open a pull request with `gh` (the address is copied). Each
  commits uncommitted work first under the session's name, because an agent
  rarely commits and a merge that landed nothing would be the worst kind of
  success. The supervisor still stops at the branch, on purpose; this is the
  surface making the workflow choice it was told to leave to surfaces.
  Reveal opens the worktree in the file manager; Editor runs the command
  Settings names with the path.

- **Rows have names and a menu.** Right-click any rail row, or the ⋯ that
  appears on hover: rename (a session's name is a new index record, its
  first prompt stays as the label; a terminal's lives with the terminal),
  reveal, open in editor, copy path, copy branch, and close, discard or stop.
  A repository's row reveals or opens the repository.

- **Notifications and a badge.** A session that asks a question posts a
  system notification; a turn that ends while the window is not in front
  posts one too. The dock icon carries the number of questions waiting.

- **More of the keyboard.** `⌘⇧A` jumps to the oldest question waiting,
  `⌘⌥↑`/`⌘⌥↓` move through the rail's rows, `/` from anywhere lands in the
  composer and Escape leaves a field. The Keyboard settings page lists them.

- **First run explains itself.** Until a provider has a credential the
  opening pane shows which agents were found on this machine and which
  providers are signed in, and offers "Sign in with axio" — axio's own CLI in
  a terminal, where `/login` already knows every provider — instead of a bare
  "add a repository".

- **A light theme, and a system option.** Appearance → Theme. The palette is
  redefined in the tokens and only there, including what text sits on a
  tinted button, so no component carries a light rule of its own.

- **A settings dialog, and a file for it.** `⌘,` (or the control at the right
  of the status bar) opens Appearance, Terminal, Agents, Model, Repositories
  and Keyboard. Interface font, size and density, and the terminal's font,
  size, line height and scrollback, live in `~/.axio/app.toml` — the window's
  own file, written whole by Rust and read by nothing else. Per-agent
  arguments live there too and are prepended to every launch of that agent.
  The default model is the one setting shared with the command line, so it is
  written to `~/.axio/config.toml` through the same line edit `/model` makes,
  leaving every other byte of that file alone. Appearance previews live while
  the dialog is open and is put back on Cancel; a terminal reads its font when
  it opens, so a change reaches the terminals opened after it rather than
  re-creating a live one and losing its scrollback.

- **Hosted terminals are named, and get a worktree of their own.** A second
  Claude Code is `Claude Code 2` — numbered among the live ones of its kind,
  so the rail can tell them apart, with the lowest free number reused. Every
  agent opened from **New** runs in a fresh worktree on a fresh branch, cut
  by the same supervisor code a session's is, so several agents on one
  repository never share a checkout; the branch shows on the row, the tab
  and the title bar, and stays when the terminal ends, exactly as a closed
  session's does. Alt-click on the menu entry runs the agent in the checkout
  itself — a choice, never a fallback.

- **The window works the way a multi-agent desktop is expected to.** Sessions
  and hosted terminals open as tabs across the pane, and a tab is a place the
  window is looking rather than the work itself — closing one stops nothing.
  The keyboard reaches everything: new session, new terminal, next and
  previous tab, tab by number, close tab, add a repository, and a command
  palette that lists every session, terminal and action by name with fuzzy
  matching. Chords use the platform's own modifier and are named in one table
  the palette reads, so the menu cannot drift from the bindings.

  Agents say when they need you. A session with a question waiting turns its
  dot warm in the rail, on its tab and in the status bar; one that finishes a
  turn while another tab is in front is marked until it is looked at. Each
  session's questions sit above its own composer; the pooled queue shows when
  no session is in front. The rail rows carry a chip — working, needs you,
  done, idle, closed — so the state of several agents reads without opening
  any of them.

  The model's prose is rendered from its markdown — fenced code with its
  language, lists, headings, quotes, inline code and emphasis — by a
  hand-written renderer that produces elements rather than HTML, so nothing
  the model writes can become markup; links show their address rather than
  being clickable. And transcript and changes can sit side by side instead of
  behind tabs, toggled from the keyboard or the palette.

- **The window can drive a session, not only watch its diff.** Selecting one
  opens its transcript — what it said, what it ran and what each call returned,
  streaming as tokens arrive, with the diff beside it as a second tab that is
  split per file and re-read when a turn ends rather than per token. Under the
  transcript is a composer: a follow-up prompt is queued behind the running
  turn, Stop interrupts it, Close keeps the worktree and Discard deletes it,
  behind a confirmation. Every one of those is a command the CLI already had —
  `send_prompt`, `cancel_session`, `close_session` were exposed and nothing in
  the interface called them, so the surface that promised to hold no capability
  the command line lacks was lacking three the command line had.

  The transcript is Rust's. `axio-app` folds the supervisor's event stream into
  a per-session record as it arrives, so the window asks for a projection and
  never re-parses a session file per paint; a session this process did not
  watch — one the CLI ran, or one that ended before the window opened — is
  seeded from its file the first time it is asked for, and the view says so.

- **A refusal carries a note.** The Deny button sent `feedback: null`, and the
  field beside it now sends what was typed — which becomes the tool result the
  model reads, so "no, use the existing helper" steers the next step instead of
  ending it. This is the one line the roadmap's claim about review being the
  centre of gravity had been missing.

- **The pane is the composer, and a repository can be added from it.** Starting
  a session had been a one-line input in the rail while the pane — most of the
  screen — explained what a session was; and that input only rendered once a
  project existed, which only happened once a session had been started, so a
  fresh window had no way in. The opening state is now the composer: pick a
  repository, say what to do, start. The rail keeps a "New session" entry that
  leads back to it. Adding a repository is a native folder picker opened from
  Rust so the path that reaches the supervisor is one this side accepted; the
  picked repository is registered and selected, and not written anywhere, for
  the reason `Projects` gives.

- **An error is shown as its message.** A command's error is the tagged
  `AppError` object, and the first real failure the window met was displayed
  as `[object Object]`.

- **Closed sessions are history.** The rail hides them behind a toggle, keeps
  the one being looked at visible, and `SessionView` gained `open` so a
  session that is merely not live in this process — the CLI's, or a previous
  run's — is told apart from one somebody actually closed. Closing such a
  session works now: the supervisor closes from the index when it holds no
  live handle, which the CLI's `session close` had been doing on its own.

- **One window per machine.** A second launch brings the first forward instead
  of starting a second supervisor over the same index and worktrees.

- **On macOS the window wears its own chrome.** A platform configuration file
  keeps the native traffic lights, placed inside the custom title bar, and asks
  the compositor for the HUD material rather than acrylic, which it does not
  have; the drawn window controls are not rendered there. Before this the
  window on a Mac was transparent with no blur behind it and two sets of
  controls, one of which the platform ignored.

- **`axio-supervisor`: many sessions at once, across many repositories.** A
  session per task, each isolated in its own git worktree on its own branch, all
  reporting into one event stream and one pooled queue of approvals, with a
  sidecar index so "what is running on this repository" is answerable without
  opening every session file.

  Agents arrive through an injected `AgentFactory` rather than being built here.
  That keeps the crate free of every transport and every tool — its own tests run
  against a scripted provider in milliseconds — and, more to the point, it is
  what lets the CLI and any later surface drive the same supervisor with their
  own wiring. Neither can hold a capability the other lacks, by construction.

  Nothing in it is reachable from a tool. Worktrees, branches, the index and the
  queue are host state, so `ToolCx` stays closed at five fields and every tool
  goes on working identically in a one-shot run. `git` is shelled out to rather
  than linked, because libgit2 compiles C and a default install is promised not
  to need a C toolchain; the credential is stripped from its environment, since
  hooks are code the repository author wrote.

  Isolation is the default and never a fallback: a worktree that cannot be cut
  is an error, because falling back to the live checkout would hand an agent
  write access to the files someone is using at the moment isolation was most
  clearly wanted. `Isolation::Direct` exists and is chosen.

  One thing it deliberately does not do: landing work. Merge, pull request and
  cherry-pick are workflows, and picking one would be wrong for the other two,
  so the branch name, `status()` and `diff()` are what a caller gets instead.

- **`axio session`, and `/new` and `/sessions` in the interactive surface.**
  A session works in a git worktree on its own branch, so several run at once
  without treading on each other or on the checkout you are in. `start`, `list`,
  `diff` and `close` from the command line; `/new <prompt>` fires one off from
  the interface and `/sessions` says what exists.

  Background sessions report through notes rather than their raw event stream.
  The viewport belongs to the session being typed into, and a parallel turn
  streaming its tokens into it would make the foreground unreadable exactly when
  there is most to read — so what lands in scrollback is what a person needs in
  order to decide something: it started, it finished, this much changed, here is
  how to look at it.

  The enabling change is that the `axio` crate now has a library target. It was
  binary-only, so everything `prepare` resolves could not be reached by anything
  else and a desktop surface would have had to write a second copy of it.

- **`axio-pty`: other agents' tools, in terminals axio owns.** Claude Code,
  Codex and Pi run as themselves - their own interface, their own approvals,
  their own idea of a session - in a pseudo-terminal this process holds, beside
  axio's own supervised sessions. The desktop surface lists them, colours each
  by its harness, and gives one a real terminal pane.

  Nothing parses them. A hosted agent's output is bytes on their way to a
  terminal emulator, and interpreting them to guess what it is doing would be a
  second, worse implementation of what already works on screen.

  The executable is allowlisted and only arguments are configurable, because
  "run whatever this string says" in a desktop application is remote code
  execution wearing the word *preference*. Output is a bounded byte ring read by
  cursor rather than pushed, so a webview reload asks for everything after the
  position it held and gets exactly the gap. Killing takes the whole tree.

  Terminal output reaches the window because Rust said something happened, not
  because a timer fired — sixteen IPC round trips a second per open terminal was
  the cost of the alternative. The signal carries no bytes: those still come
  back through a cursor, so a listener that missed one is late rather than out
  of sync, and a slow fallback timer covers the case.

  Verified against a real pseudo-terminal: spawn, output reaching the ring, the
  exit being noticed, kill, and resize. Getting there found a bug worth the
  trip — closing a ConPTY waits for its output pipe to drain, the reader thread
  is blocked on that pipe, and the child exiting does not break the cycle
  because the terminal outlives its process. Dropping a session would have hung
  the application on the click that closed a terminal.

- **The TypeScript boundary is generated rather than mirrored.** Every shape
  that crosses into the webview is derived from the Rust with ts-rs and written
  into `ui/src/generated/` when the crate's tests run, so a Rust change with no
  regeneration leaves a dirty tree rather than a silent mismatch.

  It earned itself on the first run: a `u64` the hand-written mirror declared as
  `number` came out as `bigint`, which is neither what the field means nor what
  JSON IPC delivers. Those fields now say so explicitly.

- **A release build of the window loaded a dev server that was not running.**
  Tauri decides dev-versus-production from a feature rather than from the cargo
  profile, so without `tauri/custom-protocol` a `--release` build still pointed
  its webview at `devUrl` — compiling, linking, launching, and showing
  ERR_CONNECTION_REFUSED against 127.0.0.1 with nothing else wrong. The sibling
  crate had documented this exact trap; this one shipped without it anyway, and
  running the binary is what found it.

- **`axio-app`: a desktop surface over the same supervisor.** A window showing
  what is running across every repository — the rail, the session list, the
  pooled approval queue with its previews, a diff view, and a custom frame.

  Rust owns all of the state. The webview is sent projections and given no way
  to hold any: no session store in TypeScript, no settings schema there, and no
  reconciliation between what the interface believes and what is actually
  running. Every command is `async`, because a Tauri command declared without it
  runs on the thread that paints — and the shapes that cross the boundary live
  in one module outside the `app` feature, so the entire surface behind the
  glass is exercised by ordinary unit tests with no webview.

  Window controls are a typed command rather than a granted capability, which
  keeps a place that can refuse; both close guards are native, because a
  `beforeunload` listener does not fire for Alt+F4 or a taskbar close; and a
  real CSP is set. Tauri is behind the `app` feature and verified absent from
  the default tree, so `cargo install axio` is unchanged.

- **`[worktree]` configuration**, with `enabled` defaulting to **on** and
  `branch_prefix` to `axio/`.

  `enabled = false` is refused from a project config, like `[permissions] allow`.
  It does not look like a permission, which is exactly why it is worth stating:
  turning it off moves an agent out of an isolated checkout and into the one you
  are working in, so a repository that could set it would be deciding, for
  everyone who cloned it, that its agents may write to your working tree.

- **`axio cost`, and the `axio-cost` crate behind it.** What the coding agents on
  this machine have spent, read from the session transcripts they already write.
  No network, no credentials. Group with `--by model|client|session|day|workspace`,
  `--json` for scripting, `--diagnose` for what each parser found and skipped.

  Twenty-three agents are covered. Four have hand-written parsers — Claude Code,
  Codex, Grok and opencode — because each carries knowledge a generic walk
  cannot infer: which of two token figures is cumulative, which repeated events
  must be suppressed, which vendor reports its own cost, which convention
  counts reasoning twice. The other twelve are rows in `sources::catalog`,
  driven by one table-driven parser, because these formats log the same event
  and differ only in where they put it.

  Eight of them keep sessions in SQLite rather than files and sit behind the
  non-default `sqlite` feature, because reaching them means compiling SQLite's C
  source and `cargo install axio` must not need a C toolchain. Seven reuse the
  same walker: nearly every one keeps a JSON blob in a column, so the database
  part is only getting to the JSON. A default build therefore covers fifteen,
  and the Settings tab lists the absent ones greyed rather than omitting them —
  *not installed* is an answer, and a list that hides them cannot distinguish it
  from *found nothing*.

  opencode is the fourth hand-written parser and the one that could not be a
  table row: its own total proves that `total = input + output + reasoning +
  cache.read`, so **reasoning is added to output there where Codex contains it
  within output**. `TokenBreakdown` holds reasoning as a subset, so the parser
  folds it in and keeps a copy for reporting. Mapped straight across it would
  vanish from every total — one observed message carries 9,459 reasoning tokens
  against 171 of output, so that row would have read 98% short.

  An agent whose directory is absent reports *not installed*, which is a
  different answer from *recorded nothing* and is what `--diagnose` prints.

  The scan runs across threads — agents in parallel, and each agent's files split
  across workers again, because one agent usually holds most of the files and
  per-agent parallelism alone leaves a single thread doing nearly all the work.
  Measured on this machine: 6.02s to 1.42s, with byte-identical totals.
  `AXIO_COST_THREADS` pins it to one thread for debugging.

  The desktop app's Cost tab shows the same table, grouped by model, agent, day,
  workspace or session. The scan is cached because reading every transcript takes
  tens of seconds; regrouping is instant, and a `rescan` button drops the cache
  when a figure needs to be current. The view distinguishes an unpriced row from
  a cheap one exactly as the CLI does.

  Every token rule in it was verified against real transcripts, and three of them
  contradict what the formats look like at a glance:

  - Codex reports `input_tokens` **including** cached reads. One real turn is
    71,375 input of which 67,456 is cache — billing the reported figure at the
    fresh rate overcharges eighteenfold.
  - Codex's `total_token_usage` is **cumulative for the session**; only
    `last_token_usage` may be summed, and consecutive repeats must be dropped.
    Across the 396 local sessions with usage, the deduplicated sum reproduces the
    session's own final total exactly in 390 and within 1% in 4 more.
  - Claude Code writes `requestId: null` on proxied turns, so the deduplication
    key falls back to the message id alone rather than collapsing every
    null-request line in a file into one row.

  Prices are a compiled-in table with a documented source per vendor, and a
  refresh overlay for the caller to populate. Anthropic's cache rates derive from
  the input price by fixed multipliers (0.1x read, 1.25x and 2x write) and
  OpenAI's tier above 272K input tokens is honoured per request. Grok records what a turn
  cost it, and that figure is preferred over the table wherever it appears — it is the only number here computed by the party doing
  the charging.

  Every model observed in the local transcripts is priced, so the totals carry
  no coverage caveat. Three vendors needed their cache-read rate listed rather
  than derived: the tenth-of-input that Anthropic and OpenAI share is nowhere
  near right for DeepSeek (2% of input), Z.ai (19%) or xAI (25%).

  Every total is broken down by **provider** and by **harness**, whichever the
  table is grouped by. The two are different questions — who is going to invoice
  me, and which tool spent it — and they genuinely diverge: $449 of the Claude
  Code usage on this machine was billed by OpenAI, because the CLI was pointed at
  a proxy. The provider is derived from the model, never from the directory the
  log sits in.

  `--import-prices` takes a models.dev-shaped feed so a model the bundled table
  has never heard of can still be costed. It is a file rather than a fetch:
  `axio-provider` is the only crate here that links HTTP, and one convenience is
  not worth spending that boundary on. The bundle outranks the feed for models it
  knows, because a bundled row carries tier and promotional structure a flat feed
  row cannot express and would silently drop.

  **A model with no known rate is reported unpriced, never as zero**, and no total
  is printed without the share of tokens it accounts for. That rule has teeth: an
  early build summed the one Codex message in 77,525 whose model it knew and
  printed `$0.30` next to the word *Codex*. `Totals` now hands back a `Cost` that
  cannot be formatted without confronting its own coverage.

- **`axio quota`, and a desktop app behind it.** How much of each provider's
  limit is left and when it resets, across ten providers — Codex, Claude,
  OpenRouter, z.ai, DeepSeek, xAI, Grok, Cursor, Ollama and opencode. `--json`
  emits one object per provider, `--diagnose` prints where each probe looks and
  whether the credential is there.

  They arrive by three different routes, because the vendors offer three. Six
  are read from a credential another tool already wrote — the files `codex`,
  `claude`, `grok` and Cursor keep for themselves — so those need no
  configuration at all. Three are API-key providers with one endpoint each.
  The rest have no API for it, and are reached with the browser session
  described below. Nothing here reads axio's own stored credentials; a quota
  probe and an agent turn are different trust boundaries.

  Each vendor needed one thing decided rather than copied. z.ai reports windows
  in its own vocabulary of unit codes, and stamps them in milliseconds — read as
  seconds that lands in 1970 and reports the window as permanently resetting; an
  unrecognised unit keeps its percentage under a vague label rather than being
  dropped, because the one dropped could be the one about to run out. DeepSeek
  returns a balance per currency as strings, and USD is preferred rather than
  summed, since adding CNY to USD produces a number that is not money in any
  currency. xAI posts an inverted ledger in string cents, so $10 arrives as
  `-1000`. Cursor's percentages are percentages even below one — `0.36` means
  0.36%, and reading it as a fraction would turn a reading the dashboard rounds
  to zero into a third of the plan. Ollama is scraped, because the Cloud Usage
  bars are not on the API at all, and an unrecognised layout is an error rather
  than a guess.

  **A response with no parseable figure is an error, never a zero.** "Nothing
  left" and "we could not tell" must not render the same.

- **Signing in to a cookie provider, in a window.** Pasting a `cookie:` header
  asks someone to know which of thirty cookies is the session, and both obvious
  ways to copy one produce something every server refuses: a bare value with no
  name, or the header with its own name still attached. The pastes on this
  machine were all the first kind.

  So the app signs in instead. A button opens the provider's own page in a
  window, the user signs in to that vendor as they would in a browser, and the
  cookies land in that webview where they are read back. The whole jar for the
  origin is kept, because a dashboard request carries all of them and some of
  these endpoints check a CSRF or region cookie alongside the session.

  Two properties this shape has that the alternatives do not. The credential
  goes only to the vendor — no relay, no extension, and the password is typed
  into the provider's own page over TLS. And **nothing is decrypted**: reading
  the browser's own jar would mean DPAPI and AES-GCM against a key the OS holds
  for another application, which is a lot of machinery to take a credential the
  user can simply grant.

  The window is deliberately decorated, so somebody else's sign-in form does not
  appear inside chrome that looks like ours — that is the shape of a phishing
  screen. It is absent from `capabilities/default.json`, which is the
  load-bearing part: a remote page must not reach axio's command surface.

  Capture is polled rather than pushed, because a site can set its session on a
  redirect, on a background request, or after a second factor, and none of those
  is a navigation event. **A cookie's name is not proof it is a session**: the
  candidate is used once against the provider's own endpoint, and only a
  credential that actually authenticates is accepted. Only Unauthorized means
  keep waiting — a missing workspace or a network blip both mean the session
  *was* recognised, and treating either as "not yet" would leave the window open
  forever on an account that is signed in and has nothing to report.

- **Cursor's session is imported from Cursor's own store.** Anyone with Cursor
  installed and signed in already holds the credential its dashboard uses, so
  there is nothing to sign in to and nothing to paste. Worth being precise about
  what this reads, because the neighbouring idea is much worse: it opens
  Cursor's own state database, not a browser's cookie store. No decryption, no
  DPAPI, no key belonging to another application — the token is sitting there in
  plain text because Cursor put it there for itself. The database is copied
  before it is read, since opening a live SQLite file either fails or recovers a
  journal into a file another program is still using. A pasted header still
  wins: that is very likely a second account, and silently preferring the local
  one would report the wrong account's usage.

- **Refresh intervals are computed, not chosen.** The five-minute constant this
  replaces documented its own problem — far more often than a weekly window
  needs, far too slow to watch a session window drain under load — and the cost
  of being wrong is asymmetric in both directions: too often and the usage
  endpoint rate-limits the tray that was reading it, too rarely and a window
  empties, resets and refills without ever being shown.

  Adaptive paces at a twentieth of the time remaining on the nearest window,
  which gives about twenty readings before it turns over, clamped to half an
  hour; inside ten minutes of a reset or above 90% used it drops to a
  one-minute floor. A failed probe *shortens* the interval, because treating an
  error as 0% used would slow the loop down at exactly the moment a retry is
  wanted. The policy takes the current time as an argument rather than reading
  it, so every branch is a test rather than something you find out by waiting.

- **The year as a calendar, and a Stats tab.** The tables answer what was spent;
  a year of days answers when the work actually happened, which is a shape
  rather than a number and no grouping of a table reaches it. `axio cost
  --calendar` and the app's Stats tab run over the same `summarise()`, so a day
  is shaded identically in each.

  Levels are quartiles of the active days, not a ramp scaled against the busiest
  one. Both scalings were tried against real data: with a peak thirty times the
  median, linear puts nearly every day in the lowest bucket and logarithmic puts
  nearly every day in the highest — 50M against 1.8B is 83% of the way up a log
  scale. Quartiles compare a day to a typical day instead, which is what someone
  reading their own calendar means.

  Stats also carries daily spend, tokens by hour and weekday, the token mix, and
  the top providers, harnesses and workspaces as ranked bars. The token mix
  earns its place: cache reads are most of a coding agent's volume and a tenth
  of its price, so a total that does not separate them points at the wrong
  culprit. Reasoning is shown as a share of output rather than beside it,
  because it is billed as output and listing the two side by side reads as
  double counting. Hours are UTC and labelled UTC rather than converted — a
  histogram that shifts when you travel is worse than one honest about its
  clock.

- **Derived columns, and coarser groupings.** Rows carry the share of their
  tokens that were cache reads, the blended dollars per million priced tokens,
  and their share of the total. Groupings gain week, month and hour — hour
  pooled across every day, which answers when the work happens rather than when
  it happened.

  The cache column began as a multiple of fresh input and was wrong to: measured
  against real data it put one model at 31x and another at 121,077x, because the
  vendors do not mean the same thing by "input" — OpenAI counts the whole
  prompt, Anthropic counts only what missed cache. Two numbers in one column
  that cannot be compared to each other is worse than no column. As a share of
  the row's tokens it means the same thing whoever reported it, and reads 90-99%
  across every vendor. Dollars per million divides by the tokens that were
  *priced*, since dividing a partial cost by a whole volume understates the rate
  by exactly the share that had no price.

  The wide table is behind `--wide` in the CLI, where eighty columns is still a
  constraint, and always on in the window, which has the room.

- **The scan is saved, and reads back in a fraction of the time.** Nothing about
  it changes between runs — the transcripts are append-only and mostly untouched
  — so paying half a minute to rediscover it on every launch was the whole cost
  of not writing it down. JSONL, one record per line. Reading it back is 0.67s
  against 2.3s to scan, and the totals round-trip exactly. `axio cost --cached`
  reads the same file the app does, so the CLI and the window share one scan and
  either can produce it. It lives under `LOCALAPPDATA` rather than the roaming
  profile: it is tens of megabytes of rebuildable data describing files on this
  machine only.

  Flattening the scan onto one pool came with it. It had spawned a thread per
  agent and split each agent's files across workers again, so the thread count
  was the product of the two — twenty-two agents against sixteen workers
  reserves stacks for over a hundred threads on a machine that runs eight — and
  it balanced badly, because one agent here holds 449 files and another holds 2,
  and the second got a whole worker while the first queued. Gathering the file
  lists first and splitting once took the speedup against
  `AXIO_COST_THREADS=1` from 2.2x to 3x, with totals unchanged run for run.

  `crates/axio-quota` is the fifth workspace member, and the reason is
  dependency isolation rather than size. Behind its `app` feature it also
  carries a Tauri desktop surface — tray icon, HTML flyout and window in one
  process, with a React and TypeScript frontend built by Vite — which pulls 189
  packages and a webview runtime. None of that may reach a default `cargo
  install axio`: the feature is off, and `cargo tree -p axio -e normal` names
  no tauri, wry or webview2. The crate is a leaf, depending on no other crate
  in the workspace, and it reads *other tools'* credentials rather than axio's,
  which is a different trust boundary and belongs behind a different name.

  Provider protocol knowledge was derived by reading CodexBar (MIT). No CodexBar
  source is included; see `NOTICE`.
- **A website, at `apps/site`.** Next.js 16, deployed to axio.sh by the Umbra
  control plane as the `axio-site` stack. Built in the Umbra design language
  with amber as axio's product accent — that system already gives each product
  its own hue inside a shared dark shell. It is the first thing in this
  repository that is not Rust; it is not a Cargo workspace member and holds no
  `.rs` files, so `scripts/limits.sh` does not see it, while
  `scripts/firewall.sh` covers it like anything else tracked.
- **Install scripts**, served from the site and living at
  `apps/site/scripts/`:

  ```sh
  curl -fsSL https://axio.sh/install | sh      # macOS, Linux, WSL
  irm https://axio.sh/install.ps1 | iex        # Windows
  ```

  Both build from source with cargo, because nothing is tagged: there is no
  binary to download and no checksum to verify. They check for a toolchain and
  refuse if it is missing or older than 1.88, install into `CARGO_HOME` as the
  invoking user, use no `sudo`, and change no shell profile. `AXIO_INSTALL_REF`
  builds a branch, tag or commit instead of the default. The site serves them as
  `text/plain` so a browser shows the source rather than downloading it.

  Two details in them are load-bearing. The version check compares major and
  minor numerically, because a string compare ranks `1.100` below `1.88` and
  would start rejecting every toolchain the day Rust reaches 1.100. And success
  is reported from the binary just installed rather than from whatever `axio`
  resolves to, because an older copy earlier on `PATH` would otherwise let the
  script confirm a build that never took effect.

- **The site generates its own icons and social card.** It had neither: the tab
  showed the browser's generic globe, `/favicon.ico` was a 404, and a pasted link
  unfurled as a bare title with no image — which for a pre-release whose only
  distribution is someone posting the URL was the whole first impression. A
  32×32 icon, a 180×180 home-screen icon and a 1200×630 card are now rendered at
  build time from the header wordmark and the hero headline, in real Geist, with
  no binary asset committed and no dependency added. `robots.txt` disallows the
  two install routes, which stay reachable for a shell but should not become a
  search result whose entire content is a script, and `sitemap.xml` exists so
  `robots.txt` points at something real. See `apps/site/README.md` for the three
  things in `brand.ts` that are easy to get wrong.
- **A copy button on each install command, and a skip link.** The page's primary
  call to action had been three commands you selected by hand. The button lives
  in the terminal bar rather than over the command, and renders only where the
  Clipboard API exists, so it is never a control that silently does nothing.

### Fixed

- **The traffic lights sat above the title bar's contents on macOS.** The
  configuration asked for them at (14, 13) in a 40px bar and macOS left them
  where it puts them for a 22pt overlay title bar; `trafficLightPosition` is
  applied from the content view's `drawRect`, which a webview covering the
  view never triggers, so it had never taken effect. The macOS title bar is
  now 22px with the wordmark row sized to it, and the inert setting is gone.

- **The window offered `axio` as a terminal and then could not start it.**
  Launching went by bare name through `PATH`, and a desktop application has
  the login shell's `PATH`, which need not include `~/.cargo/bin` — so the
  launcher failed with a page of directories. `Harness::locate` now finds the
  executable first: axio's own binary is looked for beside the running one,
  every harness on `PATH`, with the `.exe`/`.cmd`/`.bat` names on Windows.
  Only harnesses that are found are offered, and a found one is started by
  its full path.

- **The session pane laid itself out in three columns.** Its container shared
  the class name `.session` with the rail's row buttons and inherited their
  grid, so the session bar, the transcript and the composer sat side by side
  with the composer squeezed against the right edge — on every session, in the
  shipped build. Only the empty state had ever been looked at, because every
  other state needed a provider and a repository to reach. The pane is
  `.session-view`; `VITE_MOCK=1` now renders every state from `ui/src/mock.ts`
  so this class of thing is a screenshot away rather than a session away.

- **Closing the window over running work looked like a hang.** Rust refused
  the close and emitted an event saying why, and nothing listened: the button
  did nothing and the window stayed. The refusal is a question now — how many
  turns and terminals are live, keep working or close anyway — and "close
  anyway" is the one caller allowed to reach `destroy`.

- **An error from an action vanished before it could be read.** The window had
  one error slot, and the five-second fallback poll cleared it on every
  successful read — so "could not start that session" lasted at most five
  seconds. Read errors still clear themselves; action errors stay until
  dismissed.

- **Hosted agents ran where you asked, rather than in the Windows directory.**
  `canonicalize` returns the extended-length path form, the command interpreter
  refuses it — saying UNC paths are not supported — and then starts the agent
  anyway, in whatever directory it fell back to. So every hosted agent was
  running somewhere that could not see the repository, while the terminal opened
  and the harness started exactly as though it had worked.

- **A hosted agent no longer decides it is a copy of itself.** The session
  markers were being filtered out of a list that was then handed to a command
  builder which had already inherited the whole environment, and `env()` only
  adds. Nothing was ever removed. The visible symptom was an agent announcing
  that it had turned transcript saving off. The test that was supposed to catch
  this asserted the marker was on the strip list rather than that a built
  command had lost it, so it passed throughout.

- **A terminal opens at the size it will actually have.** The pty was created at
  a fixed 32×120 whatever the pane measured, and a harness paints its opening
  screen from the size it is given — so the resize arriving a moment later
  repainted the live area and left a mis-sized welcome in scrollback for the
  rest of the session. The pane is measured before the terminal exists, and the
  measurement waits for the real font: the fit addon sizes a cell by measuring
  one, and measuring the fallback face puts every column of every box-drawing
  character at the wrong x.

- **A busy terminal costs one read per burst rather than one per 8KB.** The pump
  signals on every successful read, and each signal started its own round trip
  with no check for one already running — so every read beginning during
  another's flight used the same stale cursor, came back with bytes already on
  screen, paid full serialisation for them and wrote them again. The cost grew
  with how chatty the agent was and multiplied by the number of open terminals.

- **Any repository can hold any number of agents.** An agent always started in
  the first project, so a second repository could hold sessions but never an
  agent and nothing let you say which one. The repository choice is one
  selection shared by the session composer and the agent launcher.

- **A terminal can be left, and can be stopped.** Opening one in the desktop
  surface replaced the pane that shows a session's diff, and nothing put it
  back — so the first terminal of a window made every diff unreachable for the
  rest of that window's life. Selecting a session now clears the terminal.

  Stopping a hosted agent was a command with no caller: `hosted_kill` existed,
  was registered, had a passing test, and no button anywhere reached it. That is
  the second command to ship that way after `start_session`, and both looked
  complete from the Rust side — which is where the tests are. Each hosted row
  now carries the control that reaches it.

- **The slash menu and the provider lists show every entry.** Opening `/` drew
  three of six commands, and the `/login` provider list could not show its last
  provider — with nothing on screen saying the rest were there.

  The live area is an inline viewport whose height is fixed when it is created;
  ratatui offers no way to change it after, and `Terminal::resize` recomputes the
  origin from the height already stored. Seven rows, minus the composer's frame
  and the status bar, left the overlays exactly three. Seven was chosen when the
  menu had three commands and the only provider list was three long, and both
  outgrew it — `openai-codex` arriving as the fourth provider is what pushed the
  list past the edge. It is now ten, so the overlays get six, and the streaming
  tail gets six rather than three as well.

  The credential form had a second bug behind the first: it drew the whole
  provider list and then trimmed the overflow **from the front**, so the question
  and the first providers scrolled silently off the top and the highlight could
  land on a row that was no longer drawn. It now windows around the selection
  like the menu does, and the question yields its row when the list needs it. The
  code had predicted this on itself — "if a fourth provider ever exists this
  needs the menu's treatment" — and nothing enforced the note, so the tests added
  with the fix iterate the real command and provider lists rather than asserting
  a count.

- **The app no longer freezes on the first Cost or Stats view**, and the cause
  was one keyword: a Tauri command declared `fn` rather than `async fn` runs on
  the main thread, which is also the thread that paints and handles input. The
  cache scanned inline on first use, so the window was dead for the whole scan —
  and it held the mutex throughout, so the second view queued behind the first
  and froze again. The scan now runs on a worker started at launch, in two
  phases: the saved scan is published within milliseconds so a tab has real
  figures before anyone reaches it, then the live scan replaces it and emits
  `cost://updated`. Refreshing returns as soon as the worker starts rather than
  blocking until it finishes, with the previous figures left on screen flagged
  as scanning — a stale number beats an empty table.

- **Opening the sign-in window no longer deadlocks the app.** Reading cookies
  posts a message to the event loop and blocks on the reply, so doing it from a
  synchronous command deadlocks — Tauri documents this on `Webview::cookies` and
  the first version ignored it. The capture is async now and reads on a thread
  of its own, a missing window is checked before anything blocking rather than
  after, and opening the window was moved off the main thread too: building a
  webview waits on the event loop, and a synchronous command *is* the event
  loop. The poll also waits for each answer before scheduling the next, rather
  than stacking a fresh check on top of a page still loading.

- **The "rate limited by provider" was ours.** Saving settings triggers a
  refresh, so does capturing a sign-in, so does the schedule, and so does the
  button — under a run of edits those pile onto an endpoint that rate limits,
  and the view reported the vendor's 429 as though the vendor were at fault.
  Twenty seconds between probes collapses a burst into one. The Refresh button
  bypasses it, because someone pressing it is asking a question the throttle
  would otherwise decline to answer silently.

- **The History tab stopped losing and duplicating series.** A series was keyed
  by joining the provider and the window label into one string and splitting it
  back on a space, which silently truncated every label containing one: "Weekly
  (Fable)" became a second "Weekly" and drew a duplicate chart against the wrong
  data, while "GPT-5.3-Codex-Spark Weekly" matched no reading at all and
  vanished from the tab entirely. It groups on the pair now, keyed as JSON
  rather than on a separator character.

- **The History charts are styled, and the file is no longer binary.** The tab
  had no CSS at all, so each chart fell back to its viewBox ratio and stood
  130px tall; they are 46px sparklines now, framed, dated at both ends. The line
  was also amber, which breaks the sheet's first rule — amber is the product
  accent and draws an edge or a mark, never a fill — so a line stating
  consumption now takes the ok/warn/crit ramp and agrees with the rail on the
  Providers tab for the same number. The same file carried two NUL bytes, which
  is why git had been treating it as binary and refusing to diff it; no other
  tracked source file has any. Also in it: the password field rendered as a
  white box on a black page, the cost table's total row fell below the fold and
  is now pinned, and the Stats cards ellipsed `18,374,235,696` to
  `18,374,235,6...` — which reads as a smaller number rather than a truncated
  one, and is now `18.37B` with the digits on hover.

- **The frameless window can be dragged again.** The app had no `capabilities/`
  directory at all, so Tauri v2 denied `core:window:allow-start-dragging` and
  the window simply did not move — no error, no warning. The minimise and close
  buttons kept working because Rust-side calls bypass the permission layer
  entirely, which made the two look like unrelated problems. The drag rule is
  inverted while there: everything drags except what responds to a click, so the
  next element somebody adds is draggable without anyone remembering to say so.

- **A refused cookie paste now says why it was refused.** The stored headers for
  all three cookie providers contained no `=` anywhere — they were bare cookie
  *values*, which is what the browser's cookie table gives you when you click a
  row, and much easier to find than the request headers. A bare value is now
  named rather than rejected, taken as the value of the provider's own session
  cookie; a leading `Cookie:` from the other copy route is stripped, since sent
  verbatim it produces `Cookie: Cookie: a=1`. Errors name the status and, on a
  redirect, where it pointed — minus the query string, which on a sign-in
  redirect carries the original URL and sometimes a token. `--diagnose` reports
  how many cookies a paste holds, whether any is a session cookie, and which
  names would count: that is the question someone actually has when a paste is
  refused.

- **opencode's usage fields are no longer guessed at one spelling.** The session
  and workspace lookup were both working, and the parser still reported "no
  rolling usage found — the session may have expired", which is a confident
  diagnosis of the wrong thing that sends anyone reading it to sign in again for
  no reason. The response is not a documented API but whatever the site's own
  server function returns, and it names things several ways. It now tries the
  full list of spellings for both the percentage and the reset countdown, and
  when it still finds nothing it reports the identifier-like keys the response
  actually contained — **names only, never values**, because these payloads can
  carry an account's email. A parser that fails silently on an undocumented
  shape is one nobody can fix.

- **The opencode workspace is found rather than asked for.** It had needed a
  workspace id typed into Settings before it would report anything, and a
  session that had just been granted still showed "no workspace" — which reads
  as a failed sign-in. The site already knows which workspaces a session can
  see, so it is asked; an explicitly configured id still wins and skips the
  lookup. The cookie is checked before the workspace now, because the other
  order reported every signed-out probe as a workspace problem and sent the fix
  in the wrong direction. Half-configured providers also stopped vanishing from
  the view: a provider with some credential keeps its error, where before the
  work was done and there was neither acknowledgement nor instruction.

- **A fresh install can reach `/login`.** The interactive surface used to build
  its provider before drawing the first frame, so an empty `~/.axio` failed for
  the missing credential and exited before the command that stores one could
  be typed. It now starts with an explicit unavailable-provider state; login
  refreshes the provider picker, and bare `/model` replaces that state in the
  same session. One-shot runs still fail closed.
- **Shell scripts stay LF on Windows checkouts.** Every documented Bash gate
  and the POSIX installer arrived as CRLF under the common `core.autocrlf`
  setting and failed at `set -o pipefail`. `.gitattributes` now makes their
  line ending part of the repository contract.
- **Relocating `AXIO_HOME` no longer reclassifies `~/.axio/config.toml`.** Both
  the active user configuration and the canonical default location are
  excluded from the project-config walk.
- **The site dependency graph is clear of the reported PostCSS and Sharp
  advisories.** Overrides keep Next.js 16.2.12 on patched transitive releases
  until its own dependency ranges catch up.
- **The README said three providers over two implementations.** There are four
  over three — `PROVIDERS` in `crates/axio-core/src/auth.rs` — and has been
  since `openai-codex` landed. The Responses transport was documented elsewhere
  in the same file while that section still described the state before it.

## [0.1.0] — unreleased

First release. A one-shot CLI and an interactive terminal surface, six tools, an
ordered permission engine, sessions on disk, and layered configuration.

### Added

- **The turn loop.** One user turn in, one `TurnEnded` out on every exit path,
  including cancellation and failure. Deterministic three-stage compaction,
  re-derived per step from the transcript so a resumed session compacts at the
  same point as the original.
- **Two providers.** The Anthropic Messages API, and an OpenAI-compatible
  dialect covering Ollama and anything speaking chat-completions. Selected by
  name, not by a plugin system.
- **Six tools** — `read`, `write`, `edit`, `glob`, `grep`, `bash` — behind a
  plan/authorise/execute split, so what was previewed is what runs.
- **An ordered permission engine** keyed on a canonical subject, with a built-in
  deny list for credentials and keys that no user rule and no `--yes` can
  override. A shell command's arguments are tested against it too.
- **Sessions.** Append-only JSONL with a versioned header, `--list`,
  `--resume` by unique id prefix, and `--ephemeral` to record nothing.
- **Layered configuration** — defaults, user file, project file, environment,
  flags — with per-section salvage and `--explain` for provenance. A section that
  will not parse is reset on its own and the file preserved; a section axio does
  not use is ignored quietly, because another tool's config is not a damaged
  one.
- **`--doctor`**, reporting credentials, the effective model and endpoint, the
  provider's real prices, the permission rules in force, and whatever the
  configuration loader complained about.
- **`--json`**, the event stream as one object per line. Unstable, and carries a
  protocol version so a consumer can refuse a stream it does not understand.
- **`axio auth`** — `login`, `status`, `logout`. Credentials are read from stdin
  and stored `0600`; an environment variable always wins.
- **Budgets.** `budget.max_steps` ends a turn that will not stop;
  `budget.max_usd_per_turn` ends one that has spent more than it was given.
  Usage is reported cumulatively per step rather than only at the end, and every
  turn carries its own cost.
- **An exit code for refused work.** A turn that completes with at least one
  action refused exits `5`. The model narrates a success it was never allowed to
  perform, and prose is not something a script can check.
- **Signal handling.** `SIGINT` cancels and flushes a partial answer; a second
  within two seconds gives up. `SIGTERM` and `SIGHUP` cancel and unwind on a
  deadline, taking the whole child process tree with them.
- **An interactive surface.** An inline viewport, so the finished transcript
  lands in the terminal's own scrollback rather than being owned by a
  full-screen application. Approval shows the diff or the raw command before it
  asks. The answer streams in a line at a time with its markdown rendered —
  headings, lists, quotes, rules, code and emphasis — while the unfinished tail
  waits in the viewport, wrapped rather than flattened onto one line.
- **A composer that behaves like a line editor.** Multi-line entry with
  shift-enter or ctrl-j, bracketed paste so a pasted paragraph stays one prompt,
  word-wise movement and deletion, ctrl-u and ctrl-k, and history recall that
  gives back the draft it was interrupted. It grows to three rows and then
  scrolls, keeping what is being typed on screen.
- **Paced drawing.** A fast stream marks the surface dirty far more often than a
  terminal can usefully repaint; frames are coalesced to at most one every 16ms.
- **A status line that says what is happening** — thinking, writing, or the
  subject of the tool being waited on — with a spinner, the turn's elapsed time
  and its token usage, so a quiet model looks different from a hung one.
- **Tables.** Rendered as aligned columns with the alignment the markdown asked
  for, narrowed to fit the terminal rather than overflowing it.
- **A session count in `--doctor`**, under paths, and a note beside any provider
  whose transport has never been run against its real endpoint.
- **The project's own instructions are read.** `AGENTS.md`, or `CLAUDE.md` when
  there is no `AGENTS.md`, from the workspace root — capped, and marked as
  outranking the model's general habits. A codebase that has written down how it
  works should not have to watch every model rediscover it.
- **Argument errors that are enough to fix the call.** Both halves name the tool
  and everything it takes, whether an argument was invented or left out, and a
  call that fails before it has a plan is still labelled with the tool it was —
  so a failure reads as `grep  invalid arguments: …` rather than as a blank.
- **A framed composer.** The prompt sits in a rounded frame carrying the model
  on its top rule and the turn's elapsed time and usage on the right of it, with
  a status bar beneath: what is happening on the left, what to press on the
  right. The frame changes colour for an approval, so the one moment that needs
  an answer does not look like the rest.
- **Tool calls as rows.** A coloured mark for the outcome, the tool's name in
  its own column, what it acted on, and the timing against the right margin.
- **Syntax highlighting in fenced code**, in the terminal's own sixteen colours
  rather than a bundled theme — a theme is chosen against a background, and the
  background belongs to the user. An unknown language renders as plain code.
- **Widths measured in columns.** A CJK character or an emoji occupies two
  columns and counts as one character; counting characters wrote every such line
  a column too wide and lost its end to the clip.
- **An optional sandbox**, Linux only: Landlock, applied before the runtime
  starts and inherited by every command axio spawns. `--sandbox`, or
  `[sandbox] enabled`.

### Dogfooding

axio was asked to add one small feature to its own repository — a line in
`--doctor` reporting how many sessions are on disk — running its interactive
surface against a real model, on a clone, with every approval displayed before
it was answered. Two approvals were asked for and granted: running the test
suite, and one edit to `crates/axio/src/doctor.rs`.

**It produced code that compiled, passed clippy, and was wrong.** The count read
`sessions/` directly, where it found the day directories that session files live
inside rather than the files themselves, so it returned zero and would have gone
on returning zero forever. Nothing about reading it suggested that; running it
did, immediately. It also failed `cargo fmt --check`, which CI would have caught
and no reviewer needed to.

What the surface got right is the part that is hard to test any other way. Four
malformed tool calls — `grep` with invented arguments, `bash` with `cmd` for
`command`, an `edit` whose `old` text was absent, another whose `old` text
matched six times — each came back as a specific, actionable message rather than
a failure, and the model corrected itself each time and carried on. The approval
prompts showed the diff and the command before asking. The turn ended cleanly,
the session recorded, and the transcript is readable.

The feature shipped, rewritten to ask `SessionStore` — which already knew where
session files live — with a regression test that fails against the original
implementation. The honest summary is that axio drove the whole loop competently
and the model's code needed a review it would not have survived without.

A second session ran the identical task from the identical commit, to see
whether the changes the first one prompted had helped. Failed calls were still
rendering against a blank name — the first fix had stopped a long message
truncating the name away, but three separate paths end a call before it has a
subject and only one of them had been patched. That is now fixed at all three,
and it is the clearest argument for doing this at all: two rounds of stub tests
and a pty harness never produced it, and two real sessions produced it twice.

The second session also settled what had *not* worked. Tool descriptions now
name their own parameters, and the invented-argument mistakes of the first
session did not recur — on one run each, which is weak evidence, and is recorded
as weak. The system prompt now asks for the project's own formatter and checks
to be run on what was changed, and **the model ignored it**: unformatted code
again, and the same non-recursive read of a directory that contains directories.
Prompt wording moved neither. What catches both is CI and review.

A third and fourth session settled what the first two could not tell apart:
whether the wrong code was the model's fault or axio's. A stronger cloud model
was given the identical task from the identical commit. Its tool calls were
flawless — no malformed arguments at all, where the first session made four —
and its formatting was clean without being asked. It then made **the same
mistake as both weaker models**, counting session files with a non-recursive
read of a directory that contains directories.

Three models, three identical wrong answers, is not three weak models. The fact
they needed is written down in this repository and axio never showed it to any
of them: it read no project instructions at all. It does now, and the fourth
session — same model, same task, one file visible — reached for
`SessionStore::files()` and got it right. That is the whole finding: the ceiling
was not the model.

**A human did not watch the approvals.** The criterion asks for one, and both
sessions were driven programmatically with each approval captured and answered
automatically. That half remains unverified.

### Known limitations

- **The sandbox is filesystem-only and Linux-only.** It says nothing about the
  network, and on other platforms asking for it is a warning rather than
  confinement. Off by default.
- **The Anthropic path is consistent with the documented wire format but has
  never met the real endpoint.** `scripts/live-check.sh` is what would change
  that, and it has now been run against the chat-completions path — thirteen
  checks, all passing — so the loop, the tools, the deny list and resume are
  proven against something nobody here wrote. Only the Anthropic transport
  remains unproven, and only because no credential exists for it.
- `budget.max_usd_per_turn` cannot fire on a provider that reports no prices.
  `--doctor` says so rather than leaving it looking enforced.

[Unreleased]: https://github.com/umbra-me/axio/commits/main
