//! Storing a credential without leaving the surface.
//!
//! The CLI reads a credential from stdin and says out loud that it will be
//! visible as you type, because a terminal in cooked mode offers nothing
//! better. Raw mode does: every keystroke arrives here, so nothing is echoed
//! that this file did not choose to draw.
//!
//! Three rules hold the whole design:
//!
//! * The secret is never drawn, only its length.
//! * The secret never reaches scrollback. Everything else the surface prints
//!   survives the process in the terminal's own buffer, which is the last place
//!   a credential should end up.
//! * It is registered for redaction the moment it is stored, so a later error
//!   quoting a request body cannot spell it out.

use axio_core::auth::{self, Secret};

/// Which question is being answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Choosing which provider the credential is for.
    Provider,
    /// Typing it.
    Secret,
}

#[derive(Debug)]
pub struct Login {
    stage: Stage,
    provider: usize,
    /// Held only until it is saved or abandoned. Not `Secret`, because this one
    /// is still being edited and editing needs the characters.
    typed: String,
}

impl Default for Login {
    fn default() -> Self {
        Self {
            stage: Stage::Provider,
            // The provider the configuration selected would be the better
            // default, but guessing wrong here stores a credential under a name
            // no run will look up. The list is three long; picking is cheap.
            provider: 0,
            typed: String::new(),
        }
    }
}

/// How the attempt to store ended.
///
/// Two outcomes, because there are two. Cancelling never reaches here — it
/// drops the flow without calling `save` — and a variant for it would be a
/// state this type can describe and never be in.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Stored, with lines to print that contain no credential.
    Stored(Vec<String>),
    /// It went wrong, and this says how.
    Failed(String),
}

impl Login {
    pub fn stage(&self) -> Stage {
        self.stage
    }

    pub fn provider(&self) -> &'static str {
        auth::PROVIDERS[self.provider.min(auth::PROVIDERS.len() - 1)]
    }

    /// How many characters have been typed, which is all the painter may know.
    pub fn typed_len(&self) -> usize {
        self.typed.chars().count()
    }

    pub fn step_provider(&mut self, delta: isize) {
        let len = auth::PROVIDERS.len() as isize;
        self.provider = ((self.provider as isize + delta).rem_euclid(len)) as usize;
    }

    pub fn provider_index(&self) -> usize {
        self.provider
    }

    pub fn push(&mut self, c: char) {
        self.typed.push(c);
    }

    pub fn backspace(&mut self) {
        self.typed.pop();
    }

    /// Take a pasted credential, which is how one normally arrives.
    ///
    /// Control characters are dropped rather than kept. A key copied out of a
    /// file brings its trailing newline, and a credential with a newline in it
    /// is rejected by the provider as simply wrong — an authentication failure
    /// that looks like a bad key and is really a bad paste.
    pub fn paste(&mut self, text: &str) {
        self.typed.extend(text.chars().filter(|c| !c.is_control()));
    }

    /// Move from choosing a provider to typing the credential.
    pub fn confirm_provider(&mut self) {
        self.stage = Stage::Secret;
    }

    /// Whether the chosen provider is signed in to rather than pasted into.
    ///
    /// Asked of the transport crate, which is where the flows are, so this
    /// cannot come to a different answer and offer a paste box for a provider
    /// with nowhere to send what was typed.
    pub fn needs_browser(&self) -> bool {
        axio_provider::oauth::is_oauth(self.provider())
    }

    /// Store what was typed.
    ///
    /// Takes `self` because the credential must not outlive the attempt: on
    /// every path out of here the buffer holding it is dropped.
    pub fn save(self, home: &std::path::Path, env: &[(String, String)]) -> Outcome {
        let provider = self.provider();
        let secret = Secret::new(self.typed.trim());
        if secret.is_empty() {
            return Outcome::Failed("nothing was typed; no credential was stored".into());
        }

        // Registered before the save can fail. A credential that reached this
        // far has been typed into a live process, and an error path that
        // quotes it is exactly the path least likely to have been rehearsed.
        axio_core::redact::register_secret(secret.expose().to_owned());

        match auth::save(home, provider, auth::Credential::Key(secret)) {
            Ok(path) => {
                let mut said = vec![
                    format!("stored the credential for `{provider}`"),
                    format!("  {}", path.display()),
                    format!("  {}", auth::protection_note()),
                ];
                if let Some(var) = auth::env_var_for(provider)
                    && env.iter().any(|(k, v)| k == var && !v.trim().is_empty())
                {
                    said.push(format!(
                        "  note: {var} is set in this shell and takes precedence"
                    ));
                }
                // Said plainly, because it is the difference between a
                // credential that fixed something and one that appears to have
                // done nothing. The provider was built when the session opened
                // and is holding the key it was given then.
                said.push("  this session keeps the credential it started with".into());
                Outcome::Stored(said)
            }
            Err(e) => Outcome::Failed(format!("could not store the credential: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn the_provider_choice_wraps_and_stays_in_the_list() {
        let mut login = Login::default();
        assert_eq!(login.provider(), auth::PROVIDERS[0]);
        login.step_provider(-1);
        assert_eq!(login.provider(), auth::PROVIDERS[auth::PROVIDERS.len() - 1]);
        login.step_provider(1);
        assert_eq!(login.provider(), auth::PROVIDERS[0]);
    }

    #[test]
    fn a_stored_credential_round_trips_and_is_not_echoed_back() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut login = Login::default();
        login.confirm_provider();
        for c in "sk-typed-in-the-surface".chars() {
            login.push(c);
        }
        assert_eq!(login.typed_len(), "sk-typed-in-the-surface".len());

        let provider = login.provider().to_owned();
        let Outcome::Stored(said) = login.save(dir.path(), &[]) else {
            panic!("it should have stored");
        };
        // Whatever is printed goes to scrollback, which survives the process.
        for line in &said {
            assert!(!line.contains("sk-typed"), "the credential leaked: {line}");
        }
        assert_eq!(
            auth::resolve(&provider, dir.path(), &[])
                .expect("a credential")
                .0
                .bearer()
                .expose(),
            "sk-typed-in-the-surface"
        );
    }

    /// The regression this guards is a support ticket, not a crash: a key
    /// copied from a file carries a newline, the paste keeps it, the provider
    /// answers 401, and the credential looks wrong when the paste was.
    #[test]
    fn a_pasted_credential_loses_its_trailing_newline() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut login = Login::default();
        login.confirm_provider();
        login.paste("sk-from-a-file\n");
        assert_eq!(login.typed_len(), "sk-from-a-file".len());

        let provider = login.provider().to_owned();
        assert!(matches!(login.save(dir.path(), &[]), Outcome::Stored(_)));
        assert_eq!(
            auth::resolve(&provider, dir.path(), &[])
                .expect("a credential")
                .0
                .bearer()
                .expose(),
            "sk-from-a-file"
        );
    }

    #[test]
    fn an_empty_credential_stores_nothing() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut login = Login::default();
        login.confirm_provider();
        login.push(' ');
        assert!(matches!(login.save(dir.path(), &[]), Outcome::Failed(_)));
        assert!(!auth::auth_path(dir.path()).exists());
    }

    #[test]
    fn backspace_shortens_what_will_be_stored() {
        let mut login = Login::default();
        login.push('a');
        login.push('b');
        login.backspace();
        assert_eq!(login.typed_len(), 1);
        login.backspace();
        login.backspace();
        assert_eq!(login.typed_len(), 0, "backspace on empty is not a panic");
    }

    /// Otherwise the login appears to have failed: it stored a credential, the
    /// next request uses a different one, and nothing said why.
    #[test]
    fn a_shadowing_variable_is_named() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut login = Login::default();
        // The first provider is whichever `auth::PROVIDERS` lists first; the
        // variable is asked of the same source the run will ask.
        let var = auth::env_var_for(login.provider()).expect("a variable");
        login.confirm_provider();
        for c in "a-key".chars() {
            login.push(c);
        }
        let Outcome::Stored(said) = login.save(dir.path(), &env(&[(var, "from-the-shell")])) else {
            panic!("it should have stored");
        };
        assert!(
            said.iter()
                .any(|l| l.contains(var) && l.contains("precedence")),
            "{said:?}"
        );
    }

    /// A session's provider was constructed with the credential that existed
    /// when it opened. Saying nothing invites the reasonable conclusion that
    /// the new one is now in use.
    #[test]
    fn the_running_session_is_said_to_keep_its_own_credential() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut login = Login::default();
        login.confirm_provider();
        login.push('k');
        let Outcome::Stored(said) = login.save(dir.path(), &[]) else {
            panic!("it should have stored");
        };
        assert!(
            said.iter().any(|l| l.contains("this session keeps")),
            "{said:?}"
        );
    }
}
