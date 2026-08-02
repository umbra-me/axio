use thiserror::Error;

/// Why a provider probe could not produce a snapshot.
///
/// The variants are deliberately coarse: the tray only needs to know whether to show a
/// "sign in" hint, back off, or surface a transient error. Detail lives in the message.
#[derive(Debug, Error)]
pub enum ProbeError {
    /// No usable local credential was found. The message is user-facing and should say
    /// exactly which command re-authenticates (e.g. "Run `codex` to log in").
    #[error("{0}")]
    NotAuthenticated(String),

    /// A credential was found but the provider rejected it (401/403).
    #[error("{0}")]
    Unauthorized(String),

    /// The provider asked us to slow down. `retry_after` is seconds, when the provider said.
    #[error("rate limited by provider")]
    RateLimited { retry_after: Option<u64> },

    #[error("HTTP {status} from {provider}: {body}")]
    Http {
        provider: &'static str,
        status: u16,
        body: String,
    },

    #[error("network error talking to {provider}: {detail}")]
    Network {
        provider: &'static str,
        detail: String,
    },

    #[error("could not read {what}: {detail}")]
    Decode { what: String, detail: String },

    #[error("could not read {path}: {detail}")]
    Io { path: String, detail: String },

    /// The provider is known but has nothing configured (no API key, disabled in config).
    #[error("{0}")]
    NotConfigured(String),
}

impl ProbeError {
    /// True when retrying immediately is pointless because the user must act.
    pub fn needs_user_action(&self) -> bool {
        matches!(
            self,
            ProbeError::NotAuthenticated(_)
                | ProbeError::Unauthorized(_)
                | ProbeError::NotConfigured(_)
        )
    }

    pub(crate) fn network(provider: &'static str, err: reqwest::Error) -> Self {
        ProbeError::Network {
            provider,
            detail: err.to_string(),
        }
    }

    pub(crate) fn decode(what: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        ProbeError::Decode {
            what: what.into(),
            detail: detail.to_string(),
        }
    }
}
