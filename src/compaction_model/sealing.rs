//! Authenticated encryption of gateway envelopes into the namespaced gateway
//! carrier format.
//!
//! A sealed carrier is the ASCII string
//! `janusgate.compaction.v1.<key id>.<base64url(nonce || ciphertext)>`. The
//! plaintext is the envelope JSON; the additional authenticated data binds the
//! compaction domain, the key identifier, and — when the carrier binds visible
//! content — the hex digest of that content, so a carrier can never be
//! consumed with content other than what it authenticated.
//!
//! Key material is stable configuration: the configuration loader deserializes
//! the typed key set and resolves it into initialized ciphers exactly once per
//! configuration generation, so the keys ride the swapped configuration
//! generation and every operation reads them without re-validation. The first
//! configured key seals, and every configured key unseals by its
//! carrier-carried identifier, so keys can rotate while existing artifacts
//! stay readable. Removing a key intentionally invalidates its artifacts.
//! There is no random process-local key and no derivation from a provider
//! access token.

use crate::{compaction_model::envelope::EnvelopeV1, config::schema::CompactionConfig};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload},
};
use serde_json::Error as JsonError;
use std::fmt;
use thiserror::Error;
use vec1::Vec1;

/// The carrier namespace: every gateway carrier starts with it, which is what
/// lets a recognizable gateway marker be distinguished from native state — and
/// what keeps a damaged marker from ever being reclassified as native state.
pub(crate) const CARRIER_NAMESPACE: &str = "janusgate.compaction.v1.";

/// The authenticated-data domain string, versioned separately from the
/// carrier prefix.
const AAD_DOMAIN: &[u8] = b"janusgate.compaction.v1";

/// Nonce length of XChaCha20-Poly1305 in bytes; a fresh nonce is drawn for
/// every seal.
const NONCE_BYTES: usize = 24;

/// The separator between the key id and the encoded payload inside a carrier.
/// Key ids never contain it — the configuration contract enforces exactly that
/// by rejecting a key id carrying this constant — so [`unseal`]'s first
/// separator split always ends the id. The configuration's key-id deserializer
/// imports this constant rather than restating the value, keeping the carrier
/// format the single owner of the fact.
pub(crate) const KEY_ID_SEPARATOR: char = '.';

/// Errors of unsealing a gateway carrier.
#[derive(Debug, Clone, Error)]
pub(crate) enum UnsealError {
    /// The carrier's key identifier names no configured key. Existing
    /// artifacts stay readable while their key remains configured; removing
    /// the key intentionally invalidates them.
    #[error("the gateway compaction carrier names an unknown key")]
    UnknownKey,

    /// The carrier is structurally malformed, or its payload does not decode
    /// as a known envelope version.
    #[error("the gateway compaction carrier is malformed")]
    Malformed,

    /// Authentication failed: the ciphertext was tampered with, the carrier
    /// was sealed under a different binding, or the key does not match.
    #[error("the gateway compaction carrier failed authentication")]
    AuthenticationFailed,

    /// No sealing keys are configured at all.
    #[error("no compaction sealing keys are configured")]
    ConfigurationMissing,
}

/// One configured sealing key: its carrier identifier and its initialized
/// cipher.
struct SealingKey {
    /// The key's carrier identifier.
    id: String,

    /// The initialized AEAD instance.
    cipher: XChaCha20Poly1305,
}

/// The set of configured sealing keys: the first seals, every one unseals.
pub(crate) struct SealingKeys {
    /// Configured keys in declaration order; the first is the sealing key.
    entries: Vec1<SealingKey>,
}

impl fmt::Debug for SealingKeys {
    // The key material is secret: the rendering names the configured
    // identifiers, never the ciphers' key material.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_list()
            .entries(self.entries.iter().map(|entry| &entry.id))
            .finish()
    }
}

impl SealingKeys {
    /// Builds the sealing keys from the typed configuration section, whose
    /// deserializer enforces the key-set contract: every cipher initializes
    /// exactly once, per configuration generation, in the configuration
    /// loader's load pipeline. The first entry seals, every entry unseals by
    /// its carrier identifier.
    ///
    /// # Panics
    ///
    /// Panics if the typed section carries no key — impossible, its
    /// deserializer rejects an empty set.
    pub(crate) fn build(config: &CompactionConfig) -> Self {
        let entries: Vec<SealingKey> = config
            .keys
            .iter()
            .map(|key| SealingKey {
                id: key.id.clone(),
                cipher: XChaCha20Poly1305::new((&key.key).into()),
            })
            .collect();

        Self {
            #[expect(
                clippy::expect_used,
                reason = "the typed configuration section declares at least one key, which its \
                          deserializer enforces"
            )]
            entries: Vec1::try_from_vec(entries)
                .expect("the typed configuration section declares at least one key"),
        }
    }
}

/// Builds the additional authenticated data binding the domain, the key id,
/// and the optional visible-content digest.
fn aad(key_id: &str, binding: Option<&[u8]>) -> Vec<u8> {
    let mut bound =
        Vec::with_capacity(AAD_DOMAIN.len() + key_id.len() + 2 + binding.map_or(0, <[u8]>::len));

    bound.extend_from_slice(AAD_DOMAIN);
    bound.push(0);
    bound.extend_from_slice(key_id.as_bytes());
    bound.push(0);
    bound.extend_from_slice(binding.unwrap_or_default());

    bound
}

/// Whether a string is a recognizable gateway carrier marker: any string in
/// the gateway compaction namespace, sealed or damaged. A marker must never be
/// forwarded upstream as native state.
#[must_use]
pub(crate) fn is_gateway_marker(value: &str) -> bool {
    value.starts_with(CARRIER_NAMESPACE)
}

/// Seals an envelope into the gateway carrier string, binding the optional
/// visible-content digest into the authentication.
///
/// # Panics
///
/// Panics if serializing the envelope fails — impossible for concrete serde
/// types — or if AEAD encryption fails under a constructed key and a fresh
/// nonce.
fn seal(keys: &SealingKeys, envelope: &EnvelopeV1, binding: Option<&[u8]>) -> String {
    let active = keys.entries.first();

    #[expect(
        clippy::expect_used,
        reason = "envelope serialization over concrete serde types cannot fail"
    )]
    let plaintext = serde_json::to_vec(envelope).expect("an envelope serializes");

    let mut nonce_and_ciphertext = Vec::with_capacity(NONCE_BYTES + plaintext.len());
    let nonce: [u8; NONCE_BYTES] = rand::random();

    nonce_and_ciphertext.extend_from_slice(&nonce);

    #[expect(
        clippy::expect_used,
        reason = "AEAD encryption with a constructed key and a fresh nonce cannot fail"
    )]
    let ciphertext = active
        .cipher
        .encrypt(
            #[expect(clippy::expect_used, reason = "the nonce slice is NONCE_BYTES long")]
            (&nonce[..]).try_into().expect("the nonce is NONCE_BYTES"),
            Payload {
                msg: &plaintext,
                aad: &aad(&active.id, binding),
            },
        )
        .expect("the AEAD seal succeeds");

    nonce_and_ciphertext.extend_from_slice(&ciphertext);

    format!(
        "{CARRIER_NAMESPACE}{}{KEY_ID_SEPARATOR}{}",
        active.id,
        URL_SAFE_NO_PAD.encode(nonce_and_ciphertext)
    )
}

/// Verifies whether a string unseals as a gateway envelope under the given
/// visible-content binding.
///
/// # Errors
///
/// Returns [`UnsealError`] for an unknown key, a malformed carrier, failed
/// authentication, or a payload that does not decode as a known envelope
/// version.
///
/// # Panics
///
/// Panics if the nonce slice conversion fails — impossible, the length check
/// above bounds the nonce slice.
fn unseal(
    keys: &SealingKeys,
    carrier: &str,
    binding: Option<&[u8]>,
) -> Result<EnvelopeV1, UnsealError> {
    let encoded = carrier
        .strip_prefix(CARRIER_NAMESPACE)
        .ok_or(UnsealError::Malformed)?;
    let (key_id, payload) = encoded
        .split_once(KEY_ID_SEPARATOR)
        .ok_or(UnsealError::Malformed)?;
    let key = keys
        .entries
        .iter()
        .find(|entry| entry.id == key_id)
        .ok_or(UnsealError::UnknownKey)?;

    let nonce_and_ciphertext = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(|_| UnsealError::Malformed)?;

    if nonce_and_ciphertext.len() < NONCE_BYTES {
        return Err(UnsealError::Malformed);
    }

    let (nonce, ciphertext) = nonce_and_ciphertext.split_at(NONCE_BYTES);

    let plaintext = key
        .cipher
        .decrypt(
            #[expect(
                clippy::expect_used,
                reason = "the length check bounds the nonce slice"
            )]
            nonce.try_into().expect("the nonce is NONCE_BYTES"),
            Payload {
                msg: ciphertext,
                aad: &aad(key_id, binding),
            },
        )
        .map_err(|_| UnsealError::AuthenticationFailed)?;

    decode_envelope(&plaintext)
}

/// Seals under the applied configuration generation's keys: the resolved keys
/// ride the swapped generation, so a reload applies at the very next
/// operation.
///
/// # Errors
///
/// Returns [`UnsealError::ConfigurationMissing`] when no `compaction`
/// configuration section is present: sealing requires the section, and its
/// absence is a server configuration gap, not a panic.
pub(crate) fn seal_with_live_config(
    envelope: &EnvelopeV1,
    binding: Option<&[u8]>,
) -> Result<String, UnsealError> {
    let keys = crate::CONFIG
        .sealing_keys()
        .ok_or(UnsealError::ConfigurationMissing)?;

    Ok(seal(&keys, envelope, binding))
}

/// Unseals under the applied configuration generation's keys.
///
/// # Errors
///
/// Returns [`UnsealError`] as [`unseal`], plus
/// [`UnsealError::ConfigurationMissing`] when no `compaction` configuration
/// section is present.
pub(super) fn unseal_with_live_config(
    carrier: &str,
    binding: Option<&[u8]>,
) -> Result<EnvelopeV1, UnsealError> {
    let keys = crate::CONFIG
        .sealing_keys()
        .ok_or(UnsealError::ConfigurationMissing)?;

    unseal(&keys, carrier, binding)
}

/// Decodes the authenticated plaintext into the envelope, rejecting an
/// unknown envelope version explicitly.
fn decode_envelope(plaintext: &[u8]) -> Result<EnvelopeV1, UnsealError> {
    let envelope: EnvelopeV1 =
        serde_json::from_slice(plaintext).map_err(|_failure: JsonError| UnsealError::Malformed)?;

    if envelope.version != EnvelopeV1::VERSION {
        return Err(UnsealError::Malformed);
    }

    Ok(envelope)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{
        CARRIER_NAMESPACE, SealingKeys, UnsealError, decode_envelope, is_gateway_marker, seal,
        unseal,
    };
    use crate::{
        compaction_model::envelope::{
            EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, ReplacementMode,
        },
        config::schema::CompactionConfig,
        test_support::{self, SEALING_KEY_PRIMARY, SEALING_KEY_SECONDARY},
    };
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    use std::error::Error as StdError;

    /// The fixture envelope every sealing scenario round-trips.
    fn fixture_envelope() -> EnvelopeV1 {
        EnvelopeV1::new(
            String::from("cmp_fixture"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: String::from("Sealed summary."),
                preserved_instructions: Vec::new(),
                retained_units: Vec::new(),
                retained_state_items: Vec::new(),
                overlap_identities: Vec::new(),
            }),
            None,
        )
    }

    /// Builds the sealing keys of the given `(id, standard-base64 material)` pairs; the first
    /// pair is the sealing key.
    fn keys_of(pairs: &[(&str, &str)]) -> Result<SealingKeys, Box<dyn StdError>> {
        let body = serde_json::json!({
            "keys": pairs
                .iter()
                .map(|(id, material)| serde_json::json!({"id": id, "key": material}))
                .collect::<Vec<_>>()
        });

        let config: CompactionConfig = serde_json::from_value(body)?;

        Ok(SealingKeys::build(&config))
    }

    /// The encoded payload segment of a well-formed carrier: the text after the second `.` of the
    /// namespaced form.
    fn encoded_payload_of(carrier: &str) -> Result<&str, Box<dyn StdError>> {
        let rest = carrier
            .strip_prefix(CARRIER_NAMESPACE)
            .ok_or("the carrier lacks the gateway namespace")?;

        rest.split_once('.')
            .map(|(_id, payload)| payload)
            .ok_or_else(|| "the carrier lacks the key-id separator".into())
    }

    /// A sealed carrier carries the namespace and the sealing key's identifier, unseals to the
    /// exact envelope under the live key set, and is recognizable as a gateway marker — while
    /// ordinary state strings are not.
    #[test]
    fn sealed_carrier_round_trips_and_carries_the_key_id() -> Result<(), Box<dyn StdError>> {
        let keys = keys_of(&[
            ("alpha", SEALING_KEY_PRIMARY),
            ("beta", SEALING_KEY_SECONDARY),
        ])?;

        let carrier = seal(&keys, &fixture_envelope(), None);

        assert!(is_gateway_marker(&carrier), "a sealed carrier is a marker");

        assert!(
            carrier.starts_with("janusgate.compaction.v1.alpha."),
            "the first configured key seals and its id rides in the clear"
        );

        let unsealed = unseal(&keys, &carrier, None).map_err(|error| error.to_string())?;

        assert_eq!(unsealed, fixture_envelope());

        assert!(
            !is_gateway_marker("native opaque state"),
            "ordinary native state is never a marker"
        );

        Ok(())
    }

    /// Rotation keeps artifacts readable while the sealing key is retained and invalidates them
    /// once it is removed: a carrier sealed under `alpha` unseals under any set that still knows
    /// `alpha`, and names an unknown key once `alpha` is gone.
    #[test]
    fn rotation_unseals_while_the_key_is_retained_and_fails_after_removal()
    -> Result<(), Box<dyn StdError>> {
        let sealing = keys_of(&[
            ("alpha", SEALING_KEY_PRIMARY),
            ("beta", SEALING_KEY_SECONDARY),
        ])?;

        let carrier = seal(&sealing, &fixture_envelope(), None);

        let rotated = keys_of(&[
            ("beta", SEALING_KEY_SECONDARY),
            ("alpha", SEALING_KEY_PRIMARY),
        ])?;

        assert!(
            unseal(&rotated, &carrier, None).is_ok(),
            "the old key still unseals after a newer key prepends"
        );

        let removed = keys_of(&[("beta", SEALING_KEY_SECONDARY)])?;

        assert!(
            matches!(
                unseal(&removed, &carrier, None),
                Err(UnsealError::UnknownKey)
            ),
            "removing the sealing key intentionally invalidates its artifacts"
        );

        Ok(())
    }

    /// The visible-content binding is authenticated: a carrier unseals only under the exact
    /// binding it sealed with, and a tampered ciphertext fails authentication rather than
    /// decoding to attacker-chosen state.
    #[test]
    fn binding_and_ciphertext_are_authenticated() -> Result<(), Box<dyn StdError>> {
        let keys = keys_of(&[("alpha", SEALING_KEY_PRIMARY)])?;

        let carrier = seal(&keys, &fixture_envelope(), Some(b"visible content"));

        assert!(unseal(&keys, &carrier, Some(b"visible content")).is_ok());

        assert!(
            matches!(
                unseal(&keys, &carrier, None),
                Err(UnsealError::AuthenticationFailed)
            ),
            "the unbound consumption of a bound carrier fails authentication"
        );

        assert!(matches!(
            unseal(&keys, &carrier, Some(b"other content")),
            Err(UnsealError::AuthenticationFailed)
        ));

        let payload = encoded_payload_of(&carrier)?;

        let mut nonce_and_ciphertext = URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|error| format!("the fixture carrier decodes: {error}"))?;

        let (last, _head) = nonce_and_ciphertext
            .split_last_mut()
            .ok_or("the fixture carrier carries a payload")?;

        *last ^= 0xff;

        let tampered = format!(
            "{CARRIER_NAMESPACE}alpha.{}",
            URL_SAFE_NO_PAD.encode(&nonce_and_ciphertext)
        );

        assert!(
            matches!(
                unseal(&keys, &tampered, None),
                Err(UnsealError::AuthenticationFailed)
            ),
            "flipping one ciphertext bit fails authentication"
        );

        Ok(())
    }

    /// Structurally broken carriers fail with the malformed or unknown-key error — never with a
    /// panic and never as plausible state.
    #[test]
    fn broken_carriers_fail_explicitly() -> Result<(), Box<dyn StdError>> {
        let keys = keys_of(&[("alpha", SEALING_KEY_PRIMARY)])?;

        assert!(
            matches!(
                unseal(&keys, "native opaque state", None),
                Err(UnsealError::Malformed)
            ),
            "a string outside the namespace is malformed"
        );

        assert!(
            matches!(
                unseal(&keys, "janusgate.compaction.v1.no-separator", None),
                Err(UnsealError::Malformed)
            ),
            "a namespaced string without a key-id separator is malformed"
        );

        assert!(
            matches!(
                unseal(&keys, "janusgate.compaction.v1.other-key.Zm9v", None),
                Err(UnsealError::UnknownKey)
            ),
            "a carrier naming an unconfigured key is unknown, not malformed"
        );

        assert!(matches!(
            unseal(
                &keys,
                "janusgate.compaction.v1.alpha.!!!not-base64!!!",
                None
            ),
            Err(UnsealError::Malformed)
        ));

        // A payload shorter than the nonce cannot carry an authenticated encryption.
        assert!(matches!(
            unseal(&keys, "janusgate.compaction.v1.alpha.YQ", None),
            Err(UnsealError::Malformed)
        ));

        Ok(())
    }

    /// The authenticated plaintext must decode as the known envelope version: a future or
    /// corrupted version inside a validly sealed carrier is malformed, never half-decoded.
    #[test]
    fn unknown_envelope_versions_are_malformed() -> Result<(), Box<dyn StdError>> {
        let mut body = serde_json::to_value(fixture_envelope())?;

        let version = body
            .get_mut("version")
            .ok_or("the fixture envelope carries its version")?;

        *version = 2.into();

        let plaintext = serde_json::to_vec(&body)?;

        assert!(matches!(
            decode_envelope(&plaintext),
            Err(UnsealError::Malformed)
        ));

        assert!(matches!(
            decode_envelope(b"not json"),
            Err(UnsealError::Malformed)
        ));

        Ok(())
    }

    /// The keys' rendering names the configured identifiers and never the key material.
    #[test]
    fn key_debug_renders_identifiers_only() -> Result<(), Box<dyn StdError>> {
        let keys = keys_of(&[
            ("alpha", SEALING_KEY_PRIMARY),
            ("beta", SEALING_KEY_SECONDARY),
        ])?;

        let rendered = format!("{keys:?}");

        assert!(rendered.contains("alpha"));
        assert!(rendered.contains("beta"));

        assert!(
            !rendered.contains(SEALING_KEY_PRIMARY),
            "the secret material never enters the rendering"
        );

        Ok(())
    }

    /// Under the installed shared test configuration the live-config path seals with the first
    /// configured key and round-trips through unsealing.
    #[test]
    fn live_configuration_seals_with_the_first_key_and_unseals() -> Result<(), Box<dyn StdError>> {
        test_support::install_configuration();

        let lock = test_support::configuration_lock();

        let carrier = super::seal_with_live_config(&fixture_envelope(), None)
            .map_err(|error| error.to_string())?;

        assert!(
            carrier.starts_with("janusgate.compaction.v1.test-primary."),
            "the first configured shared key seals"
        );

        let unsealed =
            super::unseal_with_live_config(&carrier, None).map_err(|error| error.to_string())?;

        assert_eq!(unsealed, fixture_envelope());

        drop(lock);

        Ok(())
    }
}
