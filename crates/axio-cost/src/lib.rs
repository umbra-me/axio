//! What each AI coding agent's sessions cost, read from the logs they already write.
//!
//! A companion to `axio-quota`, and deliberately separate from it. Quota answers *how much
//! of my limit is left* by asking each vendor's usage API; cost answers *what have I spent
//! and on what* by reading session transcripts already on disk. They share a shape of
//! problem and nothing else: different inputs, different failure modes, and — once the
//! SQLite-backed agents are included — different dependencies.
//!
//! Three layers, each usable on its own:
//!
//! | Layer | Owns |
//! | --- | --- |
//! | [`tokens`] | The buckets a bill is computed from, and the normalizations that make them comparable across vendors |
//! | [`message`] | One billable message, however it was logged, plus the deduplication that keeps a streamed response from being billed once per chunk |
//! | [`pricing`] | Rates per model, bundled and refreshable, and the arithmetic |
//!
//! Nothing here reads the network. The refresh that populates [`pricing::Prices`] is
//! performed by the caller and handed in, so this crate stays testable without a socket
//! and usable without one.
//!
//! # What it deliberately does not do
//!
//! It does not estimate. A model with no published rate is reported *unpriced* and its
//! tokens are still counted — never priced at zero, which would be indistinguishable from
//! a free model once summed. The tokens are a fact; the price is a lookup that can miss.

pub mod message;
pub mod pricing;
pub mod sources;
pub mod tokens;
pub mod totals;

pub use message::{ClientId, CostMessage, DedupLedger};
pub use pricing::{ModelPricing, PriceSource, Prices, Resolved, provider_of};
pub use sources::{AgentReport, ScanReport, Source, registry, scan};
pub use tokens::TokenBreakdown;
pub use totals::{Cost, Totals};
