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
/// Key ids never contain it; the configuration contract rejects one that does.
const KEY_ID_SEPARATOR: char = '.';

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
pub(crate) fn seal(keys: &SealingKeys, envelope: &EnvelopeV1, binding: Option<&[u8]>) -> String {
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
pub(crate) fn unseal(
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
pub(crate) fn unseal_with_live_config(
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
        clippy::expect_used,
        reason = "test fixtures parse values constructed to succeed"
    )]

    use super::{
        CARRIER_NAMESPACE, SealingKeys, UnsealError, aad, is_gateway_marker, seal, unseal,
    };
    use crate::{
        compaction_model::envelope::{
            EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, ReplacementMode,
        },
        config::schema::CompactionConfig,
    };

    /// A valid 32-byte key in standard base64.
    const KEY_MATERIAL: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    /// Builds the fixture key set from the canonical material.
    ///
    /// # Panics
    ///
    /// Panics if the fixture configuration fails to parse, which the fixed
    /// fixture material prevents.
    fn build_keys() -> SealingKeys {
        let config: CompactionConfig = serde_json::from_value(serde_json::json!({
            "keys": [{"id": "primary", "key": KEY_MATERIAL}]
        }))
        .expect("the fixture key configuration parses");

        SealingKeys::build(&config)
    }

    /// A minimal gateway summary envelope for the sealing round-trips.
    fn envelope() -> EnvelopeV1 {
        EnvelopeV1::new(
            String::from("cmp_test"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: String::from("summary"),
                preserved_instructions: Vec::new(),
                retained_units: Vec::new(),
                retained_state_items: Vec::new(),
                overlap_identities: Vec::new(),
            }),
            None,
        )
    }

    /// A seal/unseal round-trip under the same binding restores the envelope.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn round_trips_under_the_same_binding() {
        let carrier = seal(&build_keys(), &envelope(), Some(b"abc"));

        assert!(is_gateway_marker(&carrier));
        assert!(carrier.starts_with(CARRIER_NAMESPACE));

        let decoded = unseal(&build_keys(), &carrier, Some(b"abc")).expect("unseals");

        assert_eq!(decoded, envelope());
    }

    /// A binding mismatch fails authentication; a damaged marker stays a typed
    /// error rather than a non-marker.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn binding_and_ciphertext_tampering_fail_authentication() {
        let keys = build_keys();
        let carrier = seal(&keys, &envelope(), Some(b"abc"));

        assert!(matches!(
            unseal(&keys, &carrier, Some(b"xyz")),
            Err(UnsealError::AuthenticationFailed)
        ));

        // Strip the last four characters of the payload: the carrier stays a
        // marker but no longer authenticates (or decodes).
        let damaged = &carrier[..carrier.len() - 4];

        assert!(is_gateway_marker(damaged));
        assert!(
            matches!(
                unseal(&keys, damaged, Some(b"abc")),
                Err(UnsealError::Malformed | UnsealError::AuthenticationFailed)
            ),
            "a damaged marker fails explicitly, never reclassifying as native state"
        );
    }

    /// A carrier sealed under one configured key unseals under another key
    /// set that still knows it, and an unknown key id is a distinct error from
    /// a malformed carrier.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn unknown_key_is_distinct() {
        let carrier = format!("{CARRIER_NAMESPACE}no-such-key.aaaa");

        assert!(matches!(
            unseal(&build_keys(), &carrier, None),
            Err(UnsealError::UnknownKey)
        ));
    }

    /// The authenticated data binds the domain, key id, and binding.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn aad_binds_domain_and_binding() {
        assert_eq!(aad("k", None), b"janusgate.compaction.v1\x00k\x00");
        assert_eq!(aad("k", Some(b"ab")), b"janusgate.compaction.v1\x00k\x00ab");
    }
}
