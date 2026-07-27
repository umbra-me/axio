//! Proof Key for Code Exchange, and the random values the flow needs.
//!
//! PKCE exists because the authorization code comes back over a loopback
//! redirect, which any other process on the machine could race for. The code
//! alone is therefore not enough to redeem: the exchange must also present the
//! verifier whose hash was sent at the start, and only the process that made it
//! has that.
//!
//! Both random values come from `ring`'s system CSPRNG — the same crypto stack
//! rustls is already configured with, rather than a second one.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest;
use ring::rand::{SecureRandom, SystemRandom};

use axio_core::provider::ProviderError;

/// Bytes of entropy behind a verifier. 32 bytes is 43 base64url characters,
/// inside the 43–128 the spec allows and well past guessing.
const ENTROPY: usize = 32;

/// A verifier, and the challenge derived from it.
///
/// The verifier is the secret half. It never leaves the process until the
/// exchange, and the exchange is the only thing that can spend the code.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn generate() -> Result<Self, ProviderError> {
        let verifier = random_urlsafe(ENTROPY)?;
        // S256, not `plain`. A verifier sent in the clear at the start is a
        // verifier anyone watching the authorize request already has.
        let digest = digest::digest(&digest::SHA256, verifier.as_bytes());
        Ok(Self {
            challenge: URL_SAFE_NO_PAD.encode(digest.as_ref()),
            verifier,
        })
    }
}

/// A url-safe random string of `bytes` entropy.
///
/// Used for the verifier and for `state`, which is what stops a callback
/// arriving from a flow this process did not begin.
pub fn random_urlsafe(bytes: usize) -> Result<String, ProviderError> {
    let mut raw = vec![0u8; bytes];
    SystemRandom::new().fill(&mut raw).map_err(|_| {
        // Not retryable and not worth dressing up: if the OS will not produce
        // randomness there is nothing safe to fall back to.
        ProviderError::Configuration("the system random number generator failed".to_owned())
    })?;
    Ok(URL_SAFE_NO_PAD.encode(raw))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verifier_is_long_enough_to_be_worth_having() {
        let pkce = Pkce::generate().expect("randomness");
        // The spec's floor is 43 characters; 32 bytes base64url is exactly that.
        assert!(pkce.verifier.len() >= 43, "{}", pkce.verifier.len());
        assert!(pkce.verifier.len() <= 128);
    }

    #[test]
    fn two_flows_never_share_a_verifier() {
        let a = Pkce::generate().expect("randomness");
        let b = Pkce::generate().expect("randomness");
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.challenge, b.challenge);
    }

    /// The whole point of S256: what goes out at the start must not be what
    /// redeems the code at the end.
    #[test]
    fn the_challenge_is_not_the_verifier() {
        let pkce = Pkce::generate().expect("randomness");
        assert_ne!(pkce.challenge, pkce.verifier);
    }

    /// Checked against the worked example in RFC 7636 appendix B, so a wrong
    /// digest or a padded alphabet fails here rather than at the token
    /// endpoint, where the only symptom is `invalid_grant`.
    #[test]
    fn the_challenge_matches_the_specs_own_example() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = digest::digest(&digest::SHA256, verifier.as_bytes());
        assert_eq!(
            URL_SAFE_NO_PAD.encode(digest.as_ref()),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    /// Nothing here may need percent-encoding: these values go into a query
    /// string and into a form body, and a `+` or `/` that survives to one and
    /// not the other is a mismatch nobody can see.
    #[test]
    fn the_alphabet_is_url_safe_and_unpadded() {
        for _ in 0..16 {
            let value = random_urlsafe(32).expect("randomness");
            assert!(
                value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
                "{value}"
            );
        }
    }
}
