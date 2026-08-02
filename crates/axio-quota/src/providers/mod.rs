pub mod claude;
pub mod codex;
pub mod openrouter;

use crate::model::ProviderId;
use crate::provider::Provider;

/// Every provider this build knows about, in display order.
pub fn all() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(codex::CodexProvider),
        Box::new(claude::ClaudeProvider),
        Box::new(openrouter::OpenRouterProvider),
    ]
}

pub fn by_id(id: ProviderId) -> Box<dyn Provider> {
    match id {
        ProviderId::Codex => Box::new(codex::CodexProvider),
        ProviderId::Claude => Box::new(claude::ClaudeProvider),
        ProviderId::Openrouter => Box::new(openrouter::OpenRouterProvider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_provider_id_has_an_implementation() {
        // Guards the match above: adding a ProviderId without a probe should fail here
        // rather than at runtime in the tray.
        for id in ProviderId::ALL {
            assert_eq!(by_id(*id).id(), *id);
        }
        assert_eq!(all().len(), ProviderId::ALL.len());
    }
}
