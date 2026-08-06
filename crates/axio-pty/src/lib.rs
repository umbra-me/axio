//! Running another agent's command-line tool in a terminal axio owns.
//!
//! Claude Code, Codex and Pi are not things axio drives through an API — they
//! are interactive programs with their own interfaces, their own approval
//! prompts and their own idea of a session. Hosting one means giving it a real
//! pseudo-terminal and getting out of the way, not wrapping it.
//!
//! Three rules the rest follows from.
//!
//! 1. **The executable is allowlisted; only arguments are configurable.** "Run
//!    whatever this string says", in a desktop application, is remote code
//!    execution wearing the word *preference*.
//! 2. **Output is bytes in a bounded ring, read by cursor.** Not pushed-only:
//!    something is always not listening — every webview reload — and a reader
//!    that asks "everything after N" is correct whether it has been away for a
//!    frame or a minute. Not decoded per chunk either, because a read lands
//!    wherever the kernel decided and a character split across that boundary is
//!    destroyed before anything can reassemble it.
//! 3. **Killing means killing the tree.** The direct child is `cmd.exe` on
//!    Windows and may have started a build on unix; either way, stopping it
//!    alone leaves the thing somebody asked to stop still running.

#![forbid(unsafe_code)]

mod buffer;
mod harness;
mod session;

pub use buffer::{MAX_BYTES, Ring};
pub use harness::{Harness, child_env, split_args};
pub use session::{HarnessSession, HarnessStatus, PtyError};
