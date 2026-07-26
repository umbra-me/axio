//! The six tools.
//!
//! Six is a deliberate number. Each needs only a filesystem, a subprocess and a
//! clock, which is what makes the command line a complete surface rather than a
//! degraded one — a tool that needs a window is a tool the CLI cannot offer.

pub mod bash;
pub mod fs;
pub mod search;

use std::sync::Arc;

use axio_core::tool::Tool;

/// Every tool, in a fixed order.
///
/// The order is part of the request bytes and therefore part of the cache
/// prefix, so it is fixed here rather than left to a map's iteration order.
pub fn all() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(fs::Read::new()),
        Arc::new(fs::Write::new()),
        Arc::new(fs::Edit::new()),
        Arc::new(search::GlobTool::new()),
        Arc::new(search::Grep::new()),
        Arc::new(bash::Bash::new()),
    ]
}
