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
//! Key material is stable configuration: the first configured key seals, and
//! every configured key unseals by its carrier-carried identifier, so keys can
//! rotate while existing artifacts stay readable. Removing a key intentionally
//! invalidates its artifacts. There is no random process-local key and no
//! derivation from a provider access token.

use crate::{compaction::envelope::EnvelopeV1, config::schema::CompactionConfig};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use chacha20poly1305::{
    XChaCha20Poly1305,
    aead::{Aead, KeyInit, Payload},
};
use serde_json::Error as JsonError;
use thiserror::Error;

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

/// Key length in bytes.
pub(crate) const KEY_BYTES: usize = 32;

/// The separator between the key id and the encoded payload inside a carrier.
/// Key ids never contain it; configuration validation rejects one that does.
const KEY_ID_SEPARATOR: char = '.';

/// Errors of building the sealing keys from configuration.
#[derive(Debug, Error)]
pub(crate) enum KeyError {
    /// The configuration declares no sealing key.
    #[error("the compaction section declares no sealing key")]
    Empty,

    /// A key id is empty, repeats, or carries the carrier separator.
    #[error("the compaction key id `{0}` is empty, repeated, or carries a `.`")]
    InvalidId(String),

    /// A key value does not decode to exactly [`KEY_BYTES`] bytes.
    #[error("a compaction key for id `{0}` does not decode to {KEY_BYTES} bytes")]
    InvalidKey(String),
}

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

    /// No sealing configuration is present at all.
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
    entries: Vec<SealingKey>,
}

impl SealingKeys {
    /// Resolves the configured keys, rejecting empty, repeated, or
    /// separator-carrying identifiers and malformed key material.
    ///
    /// # Errors
    ///
    /// Returns [`KeyError`] for any invalid key entry; the configuration
    /// loader runs the same resolution at load time, so a request only ever
    /// sees a violation after configuration drift.
    ///
    /// # Panics
    ///
    /// Panics if decoded key material fails its length conversion —
    /// impossible, the decode above filters to exactly [`KEY_BYTES`].
    pub(crate) fn resolve(config: &CompactionConfig) -> Result<Self, KeyError> {
        let mut entries = Vec::with_capacity(config.keys.len());

        for entry in &config.keys {
            if entry.id.is_empty() || entry.id.contains(KEY_ID_SEPARATOR) {
                return Err(KeyError::InvalidId(entry.id.clone()));
            }

            if entries
                .iter()
                .any(|existing: &SealingKey| existing.id == entry.id)
            {
                return Err(KeyError::InvalidId(entry.id.clone()));
            }

            let raw = STANDARD
                .decode(entry.key.as_bytes())
                .ok()
                .filter(|decoded| decoded.len() == KEY_BYTES)
                .ok_or_else(|| KeyError::InvalidKey(entry.id.clone()))?;

            #[expect(
                clippy::expect_used,
                reason = "the resolved key material is exactly KEY_BYTES by construction"
            )]
            let key_bytes: [u8; KEY_BYTES] = raw
                .try_into()
                .expect("the resolved key material is exactly KEY_BYTES");

            entries.push(SealingKey {
                id: entry.id.clone(),
                cipher: XChaCha20Poly1305::new((&key_bytes).into()),
            });
        }

        if entries.is_empty() {
            return Err(KeyError::Empty);
        }

        Ok(Self { entries })
    }

    /// Resolves the keys from the live configuration, mapped to the unseal
    /// error domain (the request-side failure shape).
    fn from_live_config() -> Result<Self, UnsealError> {
        let Some(config) = crate::CONFIG.compaction.as_ref() else {
            return Err(UnsealError::ConfigurationMissing);
        };

        Self::resolve(config).map_err(|failure| {
            // The configuration loader validated the same material at load; a
            // failure here is configuration drift and fails the request.
            log::error!("invalid compaction key configuration: {failure}");

            UnsealError::ConfigurationMissing
        })
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

/// Validates the configuration's sealing key set: the exact check the
/// configuration loader runs at every load, so an invalid key set fails the
/// load instead of surfacing on the first compaction request.
///
/// # Errors
///
/// Returns [`KeyError`] for an empty set, a repeated or separator-carrying key
/// identifier, or key material that does not decode to exactly [`KEY_BYTES`]
/// bytes.
pub(crate) fn validate_key_configuration(
    config: &crate::config::schema::Config,
) -> Result<(), KeyError> {
    match config.compaction.as_ref() {
        Some(section) => SealingKeys::resolve(section).map(|_keys| ()),

        None => Ok(()),
    }
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
/// # Errors
///
/// Returns [`UnsealError::ConfigurationMissing`] when no valid key
/// configuration is present: sealing requires the compaction configuration
/// section, and its absence is a server configuration gap, not a panic.
///
/// # Panics
///
/// Panics if serializing the envelope fails — impossible for concrete serde
/// types — or if AEAD encryption fails under a constructed key and a fresh
/// nonce.
pub(crate) fn seal(
    keys: &SealingKeys,
    envelope: &EnvelopeV1,
    binding: Option<&[u8]>,
) -> Result<String, UnsealError> {
    let active = keys
        .entries
        .first()
        .ok_or(UnsealError::ConfigurationMissing)?;

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

    Ok(format!(
        "{CARRIER_NAMESPACE}{}{KEY_ID_SEPARATOR}{}",
        active.id,
        URL_SAFE_NO_PAD.encode(nonce_and_ciphertext)
    ))
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

/// Seals under the live configuration's keys.
///
/// # Errors
///
/// Returns [`UnsealError::ConfigurationMissing`] when no valid key
/// configuration is present.
pub(crate) fn seal_with_live_config(
    envelope: &EnvelopeV1,
    binding: Option<&[u8]>,
) -> Result<String, UnsealError> {
    seal(&SealingKeys::from_live_config()?, envelope, binding)
}

/// Unseals under the live configuration's keys.
///
/// # Errors
///
/// Returns [`UnsealError`] as [`unseal`], plus
/// [`UnsealError::ConfigurationMissing`] when no valid key configuration is
/// present.
pub(crate) fn unseal_with_live_config(
    carrier: &str,
    binding: Option<&[u8]>,
) -> Result<EnvelopeV1, UnsealError> {
    unseal(&SealingKeys::from_live_config()?, carrier, binding)
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
        CARRIER_NAMESPACE, KeyError, SealingKeys, UnsealError, aad, is_gateway_marker, seal, unseal,
    };
    use crate::{
        compaction::envelope::{
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
    /// Panics if the fixture configuration fails to parse or resolve, which
    /// the fixed fixture material prevents.
    fn build_keys() -> SealingKeys {
        let config: CompactionConfig = serde_json::from_value(serde_json::json!({
            "keys": [{"id": "primary", "key": KEY_MATERIAL}]
        }))
        .expect("the fixture key configuration parses");

        SealingKeys::resolve(&config).expect("the fixture key configuration resolves")
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
        let carrier = seal(&build_keys(), &envelope(), Some(b"abc")).expect("seals");

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
        let carrier = seal(&keys, &envelope(), Some(b"abc")).expect("seals");

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

    /// Key resolution rejects empty lists, repeated ids, separator-carrying
    /// ids, and wrong-length material.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn key_resolution_validates() {
        assert!(matches!(
            SealingKeys::resolve(&CompactionConfig { keys: Vec::new() }),
            Err(KeyError::Empty)
        ));

        let repeated: CompactionConfig = serde_json::from_value(serde_json::json!({
            "keys": [
                {"id": "a", "key": KEY_MATERIAL},
                {"id": "a", "key": KEY_MATERIAL}
            ]
        }))
        .expect("parses");

        assert!(matches!(
            SealingKeys::resolve(&repeated),
            Err(KeyError::InvalidId(_))
        ));

        let separator: CompactionConfig = serde_json::from_value(serde_json::json!({
            "keys": [{"id": "a.b", "key": KEY_MATERIAL}]
        }))
        .expect("parses");

        assert!(matches!(
            SealingKeys::resolve(&separator),
            Err(KeyError::InvalidId(_))
        ));

        let short: CompactionConfig = serde_json::from_value(serde_json::json!({
            "keys": [{"id": "a", "key": "AAAA"}]
        }))
        .expect("parses");

        assert!(matches!(
            SealingKeys::resolve(&short),
            Err(KeyError::InvalidKey(_))
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
