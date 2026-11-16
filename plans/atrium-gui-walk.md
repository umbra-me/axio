# Axio Atrium GUI walk with `axio acp` (open handoff)

Atrium's real ACP client has driven `axio acp` headlessly: approval cards,
messages, plan, commands, modes, options and its checked file writes all
work (receipt `docs/receipts/2026-11/2026-11-16-acp-round-2.md`). What is
left is a visual walk in the app itself. That is GUI work on the owner's Mac,
so the owner runs it through Codex computer use. Paste the prompt below.

## Prompt for Codex (computer use)

> You are checking that Axio Atrium can use the axio coding agent over ACP.
> Use the installed Axio Atrium app (prefer "Axio Atrium QA" if installed).
> Do not change any other app or setting, and do not sign in anywhere.
>
> 1. In Terminal, make a scratch folder and confirm the binary:
>    `mkdir -p ~/axio-atrium-walk && cd ~/axio-atrium-walk && git init -q && axio --version`
>    (expect `axio 0.2.0` or later).
> 2. In Atrium, open the settings for agent harnesses, choose ACP, and add a
>    custom agent with program `~/.cargo/bin/axio` (absolute path
>    `/Users/kian/.cargo/bin/axio`) and arguments `acp`. Give it access, not
>    full access.
> 3. Start a new chat with that agent in `~/axio-atrium-walk`. Before
>    sending, set the Model option to `anthropic/claude-haiku-4-5` (cheap)
>    and the mode to "Ask". Screenshot the options and mode picker.
> 4. Send: `Create hello.txt containing hi, run cat hello.txt, and keep a
>    short todo list.` Approve each card. Note how many cards each step
>    shows: in Ask mode a write is expected to show axio's question and then
>    Atrium's own file-write card.
> 5. Check, with a screenshot each: the streamed answer (paragraphs apart),
>    tool calls with titles and results, the todo plan, the diff for
>    hello.txt, and that `~/axio-atrium-walk/hello.txt` contains `hi`.
> 6. Switch the mode to "Accept edits", send `Append a second line "there"
>    to hello.txt.` and confirm only Atrium's write card appears.
> 7. Send `Count slowly from 1 to 200, one number per line.` and press Stop
>    after a few seconds. Confirm the turn ends as stopped.
> 8. Type `/` in the composer and screenshot the command list (expect
>    `compact`).
> 9. Quit nothing else. Report: pass or fail for each step, the screenshots,
>    any error text shown by Atrium, and the axio version.

## After the walk

Bring the results back; then a dated receipt records them and this file moves
to `docs/archive/` with that receipt as its successor.
