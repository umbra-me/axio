//! `read`, `write` and `edit`.
//!
//! Split on what they do rather than on width: `read` shares nothing with the
//! other two beyond the tool contract itself, so nothing crosses between the
//! children and `WRITE_EFFECTS` is private to the pair that carries it.

mod read;
mod write;

pub use read::Read;
pub use write::{Edit, Write};
