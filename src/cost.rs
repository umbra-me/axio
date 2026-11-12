//! What a session has cost: published per-million-token prices for the
//! models axio knows (checked 2026-11), applied to each reply's usage. An
//! estimate: it leaves out anything the provider bills that the reply does
//! not report.

use crate::provider::{Model, Usage};

/// US dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

fn per_million(input: f64, output: f64, cache_read: f64) -> Price {
    Price {
        input,
        output,
        cache_read,
        // Five-minute cache writes cost a quarter more than input.
        cache_write: input * 1.25,
    }
}

/// The price of a reply on `model`, with fast mode when it applied, and the
/// prompt size (Claude Haiku 5.5 costs more above 100k prompt tokens).
pub fn price(model: &Model, fast: bool, prompt_tokens: u64) -> Option<Price> {
    if model.provider != "anthropic" {
        return None;
    }
    let base = match model.id.as_str() {
        "claude-fable-5-2" | "claude-mythos-5-2" => per_million(8.0, 40.0, 0.20),
        "claude-fable-5-1" | "claude-mythos-5-1" | "claude-fable-5" => per_million(10.0, 50.0, 1.0),
        "claude-opus-5-5" => per_million(4.0, 20.0, 0.20),
        "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" => {
            per_million(5.0, 25.0, 0.50)
        }
        "claude-sonnet-5-5" => per_million(2.0, 10.0, 0.10),
        "claude-sonnet-5" => per_million(2.0, 10.0, 0.20),
        "claude-sonnet-4-6" => per_million(3.0, 15.0, 0.30),
        "claude-haiku-5-5" if prompt_tokens > 100_000 => per_million(0.50, 2.50, 0.05),
        "claude-haiku-5-5" => per_million(0.10, 0.50, 0.01),
        "claude-haiku-4-5" => per_million(1.0, 5.0, 0.10),
        _ => return None,
    };
    let fast = crate::provider::anthropic::fast_mode(&model.id).filter(|_| fast);
    Some(match fast {
        // Fast mode's published input and output prices; cache prices
        // scale with them.
        Some(f) => {
            let k = f64::from(f.price.0) / base.input;
            Price {
                input: f64::from(f.price.0),
                output: f64::from(f.price.1),
                cache_read: base.cache_read * k,
                cache_write: base.cache_write * k,
            }
        }
        None => base,
    })
}

/// The dollars one reply cost, when the model's price is known.
pub fn of(model: &Model, fast: bool, u: &Usage) -> Option<f64> {
    let p = price(model, fast, u.input + u.cache_read + u.cache_write)?;
    Some(
        (u.input as f64 * p.input
            + u.output as f64 * p.output
            + u.cache_read as f64 * p.cache_read
            + u.cache_write as f64 * p.cache_write)
            / 1_000_000.0,
    )
}

/// What a session has spent so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Spend {
    pub usd: f64,
    /// Tokens on models without a known price.
    pub unpriced_tokens: u64,
    /// Replies paid for by a ChatGPT plan rather than per token.
    pub on_plan: bool,
}

impl Spend {
    pub fn add(&mut self, model: &Model, fast: bool, u: &Usage) {
        if model.provider == crate::chatgpt::PROVIDER {
            self.on_plan = true;
        } else if let Some(usd) = of(model, fast, u) {
            self.usd += usd;
        } else {
            self.unpriced_tokens += u.input + u.output + u.cache_read + u.cache_write;
        }
    }

    pub fn merge(&mut self, other: Spend) {
        self.usd += other.usd;
        self.unpriced_tokens += other.unpriced_tokens;
        self.on_plan |= other.on_plan;
    }

    /// `~$0.42`, `ChatGPT plan`, or a token count when no price is known.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if self.usd > 0.0 {
            parts.push(if self.usd < 0.01 {
                "<$0.01".to_string()
            } else {
                format!("~${:.2}", self.usd)
            });
        }
        if self.unpriced_tokens > 0 {
            parts.push(format!(
                "{} tokens unpriced",
                crate::ui::tokens(self.unpriced_tokens)
            ));
        }
        if self.on_plan {
            parts.push("ChatGPT plan".into());
        }
        if parts.is_empty() {
            "$0.00".into()
        } else {
            parts.join(" + ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
        Usage {
            input,
            output,
            cache_read,
            cache_write,
        }
    }

    #[test]
    fn prices_follow_the_model_cache_and_fast_mode() {
        let opus = Model::parse("anthropic/claude-opus-5-5");
        let u = usage(100_000, 10_000, 0, 0);
        assert!((of(&opus, false, &u).unwrap() - 0.60).abs() < 1e-9);
        assert!(
            (of(&opus, true, &u).unwrap() - 1.20).abs() < 1e-9,
            "fast is 2x"
        );
        let cached = usage(0, 0, 1_000_000, 1_000_000);
        assert!((of(&opus, false, &cached).unwrap() - (0.20 + 5.0)).abs() < 1e-9);
        let sonnet = Model::parse("anthropic/claude-sonnet-5-5");
        assert!(
            (of(&sonnet, true, &u).unwrap() - 0.30).abs() < 1e-9,
            "no fast mode on Sonnet: standard price"
        );
        let haiku = Model::parse("anthropic/claude-haiku-5-5");
        assert!((of(&haiku, false, &usage(50_000, 0, 0, 0)).unwrap() - 0.005).abs() < 1e-9);
        assert!((of(&haiku, false, &usage(200_000, 0, 0, 0)).unwrap() - 0.10).abs() < 1e-9);
        assert_eq!(of(&Model::parse("openai/gpt-x"), false, &u), None);
    }

    #[test]
    fn spend_labels_say_what_is_known() {
        let mut s = Spend::default();
        assert_eq!(s.label(), "$0.00");
        s.add(
            &Model::parse("anthropic/claude-opus-5-5"),
            false,
            &usage(1_000, 100, 0, 0),
        );
        assert_eq!(s.label(), "<$0.01");
        s.add(
            &Model::parse("anthropic/claude-opus-5-5"),
            false,
            &usage(100_000, 10_000, 0, 0),
        );
        assert_eq!(s.label(), "~$0.61");
        s.add(
            &Model::parse("openrouter/x/y"),
            false,
            &usage(1_200, 0, 0, 0),
        );
        s.add(
            &Model::parse("chatgpt/gpt-6-luna"),
            false,
            &usage(5, 5, 0, 0),
        );
        assert_eq!(s.label(), "~$0.61 + 1.2k tokens unpriced + ChatGPT plan");
        let mut total = Spend::default();
        total.merge(s);
        assert_eq!(total, s);
    }
}
