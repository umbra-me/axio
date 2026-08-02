//! The command line, as clap sees it.
//!
//! Nothing here does anything: it is the shape of the invocation and its help
//! text, kept apart from the work so that reading either one does not mean
//! scrolling past the other.

use super::*;

pub(crate) const VERSION: &str =
    concat!(env!("CARGO_PKG_VERSION"), " (", env!("AXIO_BUILD_SHA"), ")");

#[derive(Subcommand, Debug)]
pub(crate) enum Command {
    /// Manage stored credentials.
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Show what the coding agents on this machine have spent.
    ///
    /// Reads session transcripts the agents already wrote to disk. Opens no socket and
    /// uses no credential. Every figure states how much of the volume it accounts for;
    /// a model with no known rate is reported unpriced rather than as zero.
    Cost {
        /// What to group rows by.
        #[arg(long, value_enum, default_value_t = crate::cost::GroupBy::Model)]
        by: crate::cost::GroupBy,

        /// Emit JSON instead of a table.
        #[arg(long)]
        json: bool,

        /// Print what was found per agent, including skipped and unreadable files.
        #[arg(long, conflicts_with = "json")]
        diagnose: bool,

        /// Maximum rows to print before summarising the remainder.
        #[arg(long, default_value_t = 25)]
        limit: usize,
    },
    /// Show how much of each provider's limit is left, and when it resets.
    ///
    /// Reads the credentials the providers' own CLIs already wrote; it does not use
    /// axio's stored credentials and cannot sign you in to anything.
    Quota {
        /// Only this provider. Defaults to every provider enabled in the quota config.
        #[arg(long, short)]
        provider: Option<String>,

        /// Emit one JSON object per provider instead of a table.
        #[arg(long)]
        json: bool,

        /// Print where each probe looks for credentials, and whether it is there.
        #[arg(long, conflicts_with_all = ["provider", "json"])]
        diagnose: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum AuthAction {
    /// Store a credential, read from stdin.
    Login {
        /// Which provider the credential is for.
        #[arg(long, default_value = "anthropic")]
        provider: String,
    },
    /// Show which providers have a credential, and where it came from.
    Status,
    /// Remove a stored credential.
    Logout {
        #[arg(long, default_value = "anthropic")]
        provider: String,
    },
}

#[derive(Parser, Debug)]
#[command(name = "axio", version = VERSION, about = "A coding agent for the terminal.")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Command>,

    /// Run one turn with this prompt and exit. Piped stdin is appended.
    #[arg(short, long)]
    pub(crate) prompt: Option<String>,

    /// Emit the event stream as one JSON object per line. Unstable.
    #[arg(long)]
    pub(crate) json: bool,

    /// Approve every action without asking. Unattended and unsandboxed.
    #[arg(long)]
    pub(crate) yes: bool,

    /// Report configuration, credentials, prices and permissions, then exit.
    #[arg(long)]
    pub(crate) doctor: bool,

    /// Ask the configured model whether it accepts tools, then exit. Unlike
    /// every other report-and-exit mode this one opens a socket and spends a
    /// few tokens.
    #[arg(long)]
    pub(crate) probe: bool,

    /// Continue a previous session. A unique prefix of its id is enough.
    #[arg(long, value_name = "ID", conflicts_with = "ephemeral")]
    pub(crate) resume: Option<String>,

    /// List recent sessions and exit.
    #[arg(long, conflicts_with_all = ["prompt", "resume"])]
    pub(crate) list: bool,

    /// Record nothing to disk.
    #[arg(long)]
    pub(crate) ephemeral: bool,

    /// Explain where a configuration key's value came from, then exit.
    #[arg(long, value_name = "KEY")]
    pub(crate) explain: Option<String>,

    /// Override the model for this run.
    #[arg(long, value_name = "NAME")]
    pub(crate) model: Option<String>,

    /// Confine every command to the workspace, using the kernel's own sandbox.
    /// Linux only. Also settable as `[sandbox] enabled`.
    #[arg(long)]
    pub(crate) sandbox: bool,
}
