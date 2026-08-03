use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Stable provider identifier. The wire strings match upstream CodexBar's config `id` values
/// so a config file can move between the two without translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Codex,
    Claude,
    Openrouter,
    Zai,
    Deepseek,
    Xai,
    Grok,
    Cursor,
    Ollama,
}

impl ProviderId {
    pub const ALL: &'static [ProviderId] = &[
        ProviderId::Codex,
        ProviderId::Claude,
        ProviderId::Openrouter,
        ProviderId::Zai,
        ProviderId::Deepseek,
        ProviderId::Xai,
        ProviderId::Grok,
        ProviderId::Cursor,
        ProviderId::Ollama,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Codex => "codex",
            ProviderId::Claude => "claude",
            ProviderId::Openrouter => "openrouter",
            ProviderId::Zai => "zai",
            ProviderId::Deepseek => "deepseek",
            ProviderId::Xai => "xai",
            ProviderId::Grok => "grok",
            ProviderId::Cursor => "cursor",
            ProviderId::Ollama => "ollama",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Codex => "Codex",
            ProviderId::Claude => "Claude",
            ProviderId::Openrouter => "OpenRouter",
            ProviderId::Zai => "z.ai",
            ProviderId::Deepseek => "DeepSeek",
            ProviderId::Xai => "xAI",
            ProviderId::Grok => "Grok",
            ProviderId::Cursor => "Cursor",
            ProviderId::Ollama => "Ollama",
        }
    }

    pub fn parse(raw: &str) -> Option<ProviderId> {
        ProviderId::ALL
            .iter()
            .copied()
            .find(|p| p.as_str().eq_ignore_ascii_case(raw.trim()))
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One rate-limit window (a 5-hour session window, a weekly window, a monthly credit cap).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateWindow {
    /// Short human label used verbatim in the tray menu, e.g. "5h" or "Weekly".
    pub label: String,
    /// 0-100. Providers disagree on whether they report used or remaining; every probe
    /// normalizes to *used* here so the UI never has to ask.
    pub used_percent: f64,
    #[serde(
        default,
        with = "time::serde::rfc3339::option",
        skip_serializing_if = "Option::is_none"
    )]
    pub resets_at: Option<OffsetDateTime>,
    /// Nominal window length. Lets the UI distinguish a 5h window from a weekly one even
    /// when `resets_at` is missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<u32>,
}

impl RateWindow {
    pub fn new(label: impl Into<String>, used_percent: f64) -> Self {
        RateWindow {
            label: label.into(),
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at: None,
            window_minutes: None,
        }
    }

    pub fn with_reset(mut self, resets_at: Option<OffsetDateTime>) -> Self {
        self.resets_at = resets_at;
        self
    }

    pub fn with_window_minutes(mut self, minutes: Option<u32>) -> Self {
        self.window_minutes = minutes;
        self
    }

    pub fn remaining_percent(&self) -> f64 {
        (100.0 - self.used_percent).clamp(0.0, 100.0)
    }
}

/// Prepaid balance, for providers that sell credits rather than (or alongside) rate windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credits {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<f64>,
    pub unlimited: bool,
    pub has_credits: bool,
}

/// What a successful probe produces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub provider: ProviderId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    pub windows: Vec<RateWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<Credits>,
    #[serde(with = "time::serde::rfc3339")]
    pub fetched_at: OffsetDateTime,
}

impl UsageSnapshot {
    pub fn new(provider: ProviderId) -> Self {
        UsageSnapshot {
            provider,
            plan: None,
            account_label: None,
            windows: Vec::new(),
            credits: None,
            fetched_at: OffsetDateTime::now_utc(),
        }
    }

    /// The window the tray icon should show: the one closest to exhaustion.
    ///
    /// A user with a 90%-used weekly window and a 10%-used session window needs to see the
    /// 90% — showing the session window would imply everything is fine.
    pub fn headline(&self) -> Option<&RateWindow> {
        self.windows.iter().max_by(|a, b| {
            a.used_percent
                .partial_cmp(&b.used_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}
