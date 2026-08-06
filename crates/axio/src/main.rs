//! The binary.
//!
//! Three lines, deliberately. Everything it does lives in the library beside
//! it, because a desktop surface has to be able to build a session exactly the
//! way this does — and a binary-only crate cannot be reused, so whatever it
//! held would have to be written twice.

fn main() -> std::process::ExitCode {
    axio::main_entry()
}
