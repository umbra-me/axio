//! The provider transport. The only crate in the workspace that links an HTTP
//! or TLS stack.

#![forbid(unsafe_code)]

pub mod anthropic;
mod catalog;
pub mod client;
pub mod openai;
pub mod sse;

pub use anthropic::{API_URL, DEFAULT_MODEL, build_body, classify};
pub use client::AnthropicProvider;
pub use openai::{OLLAMA_BASE, OpenAiProvider};
pub use sse::{SseDecoder, SseFrame};
