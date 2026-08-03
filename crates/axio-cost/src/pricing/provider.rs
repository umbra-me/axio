//! Which company charged for a model.
//!
//! Distinct from the harness — the tool that was run — and the two genuinely diverge here.
//! The Claude Code transcripts on this machine bill `gpt-5.6-terra`, `deepseek-v4-flash`
//! and `glm-5.2` alongside `claude-opus-5`, because the CLI was pointed at a proxy.
//! Grouping spend by the directory a log sits in would credit Anthropic for OpenAI's
//! tokens; grouping by provider answers *who is going to invoice me*.
//!
//! Two signals, in order:
//!
//! 1. an explicit vendor prefix, when a router or gateway wrote one — `anthropic/…`;
//! 2. the model family, which is unambiguous in practice because vendors do not name
//!    models after each other.
//!
//! An id matching neither is reported as unknown rather than guessed into the nearest
//! family, for the same reason an unknown model is unpriced rather than free.

/// A model whose vendor could not be determined.
pub const UNKNOWN: &str = "(unknown)";

/// `(prefix, provider)`, longest prefix first so a more specific family wins.
///
/// Prefixes rather than exact ids: the table would otherwise need a row per model and go
/// stale the day a vendor ships one, which is exactly the failure this avoids.
const FAMILIES: &[(&str, &str)] = &[
    ("claude-", "Anthropic"),
    ("gpt-", "OpenAI"),
    ("o1-", "OpenAI"),
    ("o3-", "OpenAI"),
    ("o4-", "OpenAI"),
    ("codex-", "OpenAI"),
    ("grok-", "xAI"),
    ("deepseek", "DeepSeek"),
    ("glm-", "Z.ai"),
    ("gemini-", "Google"),
    ("gemma-", "Google"),
    ("kimi-", "Moonshot"),
    ("moonshot", "Moonshot"),
    ("qwen", "Alibaba"),
    ("llama-", "Meta"),
    ("mistral", "Mistral"),
    ("mixtral", "Mistral"),
    ("command-", "Cohere"),
    ("nova-", "Amazon"),
    ("phi-", "Microsoft"),
];

/// Vendor prefixes a gateway may write, mapped to the company they name.
const VENDOR_PREFIXES: &[(&str, &str)] = &[
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("x-ai", "xAI"),
    ("xai", "xAI"),
    ("deepseek", "DeepSeek"),
    ("z-ai", "Z.ai"),
    ("zhipuai", "Z.ai"),
    ("google", "Google"),
    ("meta-llama", "Meta"),
    ("moonshotai", "Moonshot"),
    ("mistralai", "Mistral"),
    ("qwen", "Alibaba"),
    ("cohere", "Cohere"),
    ("amazon", "Amazon"),
    ("microsoft", "Microsoft"),
];

/// The company that billed for `raw_model`, as spelled in the log.
///
/// Takes the raw id rather than a normalized one: normalization strips the vendor prefix,
/// which is the strongest signal available when a gateway wrote one.
pub fn provider_of(raw_model: &str) -> &'static str {
    let id = raw_model.trim().to_ascii_lowercase();

    // A gateway's own prefix is authoritative — it names who it is reselling.
    if let Some((vendor, _)) = id.split_once('/')
        && let Some((_, provider)) = VENDOR_PREFIXES
            .iter()
            .find(|(prefix, _)| vendor == *prefix || vendor.ends_with(*prefix))
    {
        return provider;
    }

    // Bedrock writes `anthropic.claude-…` with a dot rather than a slash.
    if let Some((vendor, _)) = id.split_once('.')
        && let Some((_, provider)) = VENDOR_PREFIXES.iter().find(|(prefix, _)| vendor == *prefix)
    {
        return provider;
    }

    let bare = super::normalize(raw_model);
    FAMILIES
        .iter()
        .find(|(prefix, _)| bare.starts_with(*prefix))
        .map(|(_, provider)| *provider)
        .unwrap_or(UNKNOWN)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every model actually seen in the local transcripts.
    #[test]
    fn locally_observed_models_resolve_to_their_vendor() {
        for (model, expected) in [
            ("claude-opus-5", "Anthropic"),
            ("claude-haiku-4-5-20251001", "Anthropic"),
            ("claude-fable-5", "Anthropic"),
            ("gpt-5.6-sol", "OpenAI"),
            ("gpt-5.5", "OpenAI"),
            ("gpt-5.3-codex-spark", "OpenAI"),
            ("grok-4.5-build", "xAI"),
            ("grok-4.5-build-free", "xAI"),
            ("deepseek-v4-flash", "DeepSeek"),
            ("glm-5.2", "Z.ai"),
        ] {
            assert_eq!(provider_of(model), expected, "{model}");
        }
    }

    /// The case this distinction exists for: a Claude Code transcript billing OpenAI.
    /// The harness is the directory; the provider is the model.
    #[test]
    fn a_proxied_model_is_credited_to_the_company_that_charged() {
        assert_eq!(provider_of("gpt-5.6-terra"), "OpenAI");
        assert_ne!(provider_of("gpt-5.6-terra"), "Anthropic");
    }

    /// A gateway's prefix outranks the family, because it names the reseller's upstream.
    #[test]
    fn an_explicit_vendor_prefix_wins() {
        assert_eq!(provider_of("anthropic/claude-fable-5"), "Anthropic");
        assert_eq!(provider_of("openrouter/anthropic/claude-opus-5"), "Anthropic");
        assert_eq!(provider_of("anthropic.claude-opus-5"), "Anthropic");
        assert_eq!(provider_of("z-ai/glm-5.2"), "Z.ai");
    }

    #[test]
    fn casing_and_padding_do_not_matter() {
        assert_eq!(provider_of("  Claude-Opus-5 "), "Anthropic");
        assert_eq!(provider_of("GPT-5.6-Sol"), "OpenAI");
    }

    /// Guessing a vendor would be the same mistake as guessing a price.
    #[test]
    fn an_unrecognised_family_is_unknown_rather_than_guessed() {
        assert_eq!(provider_of("some-model-nobody-ships"), UNKNOWN);
        assert_eq!(provider_of(""), UNKNOWN);
    }
}
