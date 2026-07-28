//! Remembering a provider and model that turned out to work.
//!
//! Written only after a turn has come back, never when one is chosen. `/model`
//! says out loud that a name is not checked until the next request, and saving
//! at the moment of choosing would make a typo the default — the one state a
//! default must never be in, because the next session starts in it and the
//! failure has nothing left to point at.

use std::path::{Path, PathBuf};

use axio_core::config::edit;
use axio_core::protocol::TurnOutcome;

/// Whether an outcome proves the endpoint accepted the request.
///
/// Not "did it go well". A refusal arrives as a normal 200 and a step limit
/// means it ran repeatedly — both are proof the model exists and answers. A
/// transport failure is the case this whole function is for, and an interrupt
/// says nothing either way: the turn may have been stopped before the first
/// byte came back.
pub(crate) fn proves_it_works(outcome: &TurnOutcome) -> bool {
    match outcome {
        TurnOutcome::Completed
        | TurnOutcome::Refused { .. }
        | TurnOutcome::StepLimit { .. }
        | TurnOutcome::BudgetExceeded { .. } => true,
        TurnOutcome::Interrupted | TurnOutcome::Failed { .. } => false,
    }
}

/// Write the provider and model into the user's configuration.
///
/// A line edit rather than a serialise, so the comments, ordering and every
/// section axio does not use survive. Written to a sibling and renamed, so an
/// interrupted write cannot leave a configuration file half-replaced — losing
/// one to a crash is worse than never having saved.
pub(crate) fn save(path: &Path, provider: &str, model: &str) -> std::io::Result<()> {
    let before = std::fs::read_to_string(path).unwrap_or_default();
    let after = edit::set_model(&before, provider, model);
    if after == before {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = temp_beside(path);
    std::fs::write(&temp, after)?;
    std::fs::rename(&temp, path)
}

fn temp_beside(path: &Path) -> PathBuf {
    path.with_extension("toml.tmp")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_outcomes_that_reached_the_model_count() {
        assert!(proves_it_works(&TurnOutcome::Completed));
        // A refusal is a normal 200: the model answered, it declined.
        assert!(proves_it_works(&TurnOutcome::Refused {
            category: None,
            text: String::new()
        }));
        assert!(proves_it_works(&TurnOutcome::StepLimit { steps: 50 }));

        // The case this exists for.
        assert!(!proves_it_works(&TurnOutcome::Failed {
            message: "http 400: unsupported parameter".into()
        }));
        // Stopped, possibly before the first byte. It proves nothing.
        assert!(!proves_it_works(&TurnOutcome::Interrupted));
    }

    #[test]
    fn saving_keeps_the_rest_of_the_file() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# mine\n[model]\nprovider = \"ollama\"\nname = \"kimi\"\n\n[budget]\nmax_steps = 9\n",
        )
        .unwrap();

        save(&path, "openai-codex", "gpt-5.6-sol").expect("it saves");

        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("# mine"), "{after}");
        assert!(after.contains("max_steps = 9"), "{after}");
        assert!(after.contains("provider = \"openai-codex\""), "{after}");
    }

    #[test]
    fn a_missing_file_is_created() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("nested").join("config.toml");
        save(&path, "ollama", "kimi").expect("it saves");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("provider = \"ollama\""), "{after}");
    }

    /// Nothing to change means nothing is written, so a session that never
    /// switches does not rewrite the file on every turn.
    #[test]
    fn no_change_leaves_the_file_untouched() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        let before = "[model]\nprovider = \"ollama\"\nname = \"kimi\"\n";
        std::fs::write(&path, before).unwrap();
        let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();

        save(&path, "ollama", "kimi").expect("it saves");

        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), stamp);
    }

    #[test]
    fn no_temporary_file_is_left_behind() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let path = dir.path().join("config.toml");
        save(&path, "ollama", "kimi").expect("it saves");
        assert!(!temp_beside(&path).exists());
    }
}
