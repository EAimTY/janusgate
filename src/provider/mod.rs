//! Provider entities: stored credentials, provider rows, model rows, the routing resolution
//! they back, and the control engines' shared failure vocabulary.
//!
//! The descriptor types a provider's definition carries live in [`definition`]; the storage tier
//! holding every database statement — including the providers' own rows — lives beside this
//! module in [`crate::storage`].

use crate::storage::Error as StorageError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
mod engine_tests;

pub(crate) mod control;
pub(crate) mod definition;
pub(crate) mod discovery;
pub(crate) mod oauth;

pub(crate) use definition::{DefinitionPayload, ProviderDefinition};

/// The provider control engine's shared failure vocabulary: the device authorization, the token
/// exchange, the refresh grants, reprovisioning, the lease-guarded renewal loop over them, and
/// the upstream model discovery — which runs the same stages for static-API-key providers.
///
/// Every upstream-facing variant renders the complete human message both HTTP surfaces display;
/// the surfaces only decide the status and the machine-readable code. Upstream rejections on
/// these paths always pin 502, so the foreign status rides in the message and the variant
/// payload.
#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum Error {
    /// The upstream endpoint answered a non-2xx status. These paths never forward foreign
    /// statuses, so the status rides in the message.
    #[error("Upstream request failed with status {status}: {detail}")]
    Rejected {
        /// Upstream status code, embedded in the message.
        status: u16,

        /// Truncated upstream body excerpt.
        detail: String,
    },

    /// The upstream endpoint failed without a usable HTTP response: a transport failure, a
    /// malformed body, or a missing required field.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The upstream endpoint was cut off by the HTTP client's timeout budget and produced no
    /// usable response. Rendered exactly like every other transport failure; carried typed so
    /// the device poller's backoff can match the fact instead of parsing the message.
    #[error("Upstream request failed: {detail}")]
    Timeout {
        /// Human-readable failure detail.
        detail: String,
    },

    /// The addressed provider row vanished mid-refresh. Every management consumer classifies
    /// this as a 404 `not_found` — the reauthorize surface's vanished-provider precedent, which
    /// `models:discover` now shares because its refresh runs inside the same token-refresh
    /// engine; the dispatch surface renders it as an internal failure.
    #[error("{message}")]
    NotFound {
        /// Human-readable failure message.
        message: String,
    },

    /// Token acquisition failed on JanusGate's side: a lease transaction could not be driven, or
    /// storage reported drift.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

impl From<StorageError> for Error {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::NotFound { message } => Self::NotFound { message },

            // Constraint and busy contention cannot arise from the refresh path's unconditional
            // writes; the surfaces render them as gateway-internal failures.
            other => Self::Internal {
                message: other.to_string(),
            },
        }
    }
}

impl From<crate::database::Error> for Error {
    fn from(error: crate::database::Error) -> Self {
        Self::Internal {
            message: error.to_string(),
        }
    }
}

impl From<control::ControlFailure> for Error {
    fn from(failure: control::ControlFailure) -> Self {
        match failure {
            control::ControlFailure::Rejected { status, body } => Self::Rejected {
                status,
                detail: crate::http_body::truncate_excerpt(&body),
            },

            control::ControlFailure::Unavailable { detail } => Self::Unavailable { detail },

            control::ControlFailure::Timeout { detail } => Self::Timeout { detail },
        }
    }
}

/// The plaintext credentials a provider row stores (README §3): a tagged JSON enum whose
/// `type` discriminator is the only stored credential-kind representation, and which must match
/// the credential kind of the provider's definition version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StoredCredentials {
    /// A static API key.
    #[serde(rename = "api_key")]
    ApiKey {
        /// The API key presented to the upstream.
        api_key: String,
    },

    /// An OAuth account's live credential state: the access token, its expiry and refresh
    /// schedule, and the durable slots the definition's recipe placed.
    #[serde(rename = "oauth")]
    OAuth {
        /// The OAuth access token.
        access_token: String,

        /// The refresh token, when the credential renews through a refresh grant; `null` when
        /// the credential never refreshes.
        #[serde(default)]
        refresh_token: Option<String>,

        /// The durable slots the flow initialized and the exchange or provisioning placed.
        #[serde(default)]
        slots: BTreeMap<String, String>,

        /// Unix timestamp in microseconds when the access token expires; `null` means no
        /// time-based refresh is scheduled, never already-expired. The storage form always
        /// carries the member; an absent one reads as `null`, which the SQL merge-patch renewal
        /// of a lease stamp relies on.
        #[serde(default)]
        expires_at: Option<i64>,

        /// The token-refresh lease stamp in microseconds; `null` when no refresh is in flight.
        #[serde(default)]
        refresh_started_at: Option<i64>,
    },
}

impl StoredCredentials {
    /// The secret these credentials present to an upstream: the OAuth access token for OAuth
    /// accounts, the static API key for key-based accounts.
    #[must_use]
    pub(crate) fn secret(&self) -> &str {
        match self {
            Self::ApiKey { api_key } => api_key,
            Self::OAuth { access_token, .. } => access_token,
        }
    }

    /// The credential kind this variant carries, shared with the definition's acquisition
    /// discriminator.
    #[must_use]
    pub(crate) const fn kind(&self) -> CredentialKind {
        match self {
            Self::ApiKey { .. } => CredentialKind::ApiKey,
            Self::OAuth { .. } => CredentialKind::OAuth,
        }
    }

    /// Renders the credentials into their plaintext JSON storage form.
    ///
    /// # Panics
    ///
    /// Panics if serialization fails, which cannot occur: the stored form is a struct of strings
    /// and numbers, a shape serde_json always renders.
    pub(crate) fn to_json(&self) -> String {
        #[expect(
            clippy::expect_used,
            reason = "the stored credentials form always serializes: it is a struct of \
                      strings and numbers"
        )]
        serde_json::to_string(self).expect("stored credentials serialize")
    }

    /// The durable slots of the OAuth variant; empty for an API key.
    #[must_use]
    pub(crate) fn slots(&self) -> &BTreeMap<String, String> {
        static EMPTY: std::sync::OnceLock<BTreeMap<String, String>> = std::sync::OnceLock::new();

        match self {
            Self::ApiKey { .. } => EMPTY.get_or_init(BTreeMap::new),
            Self::OAuth { slots, .. } => slots,
        }
    }
}

/// The credential kind of a provider definition or row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// A static API key.
    ApiKey,

    /// An OAuth account.
    OAuth,
}

impl CredentialKind {
    /// Renders the canonical credential-kind name; the storage form and the wire form are one
    /// vocabulary.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::OAuth => "oauth",
        }
    }
}

/// The discovery lifecycle state of a model row: which role produced the row and what the most
/// recent successful discovery said about it.
///
/// The serde and TypeScript names are the management wire names, shared with
/// `DiscoveryState::as_str` — the storage form and the wire form are one vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub enum DiscoveryState {
    /// Listed by the most recent successful discovery of its provider.
    #[serde(rename = "discovered")]
    Discovered,

    /// Was listed by an earlier discovery but missing from the most recent one: service is
    /// suspended (the three-input effective status is false) until the upstream lists it again or
    /// an administrator claims it through `models:create`.
    #[serde(rename = "vanished")]
    Vanished,

    /// Created — or claimed out of a vanished row — through `models:create`; a discovery that
    /// lists its identifier flips it to [`Discovered`](Self::Discovered), one that does not leaves
    /// it alone: a manual row is never automatically marked `vanished`.
    #[serde(rename = "manual")]
    Manual,
}

impl DiscoveryState {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The words
    /// live once each, in the exhaustive [`Self::as_str`] match. A new variant joins `ALL`; a
    /// partial table update is a compile error (the length pin), and the exhaustive match is
    /// what forces the variant's spelling to be stated.
    const ALL: [Self; 3] = [Self::Discovered, Self::Vanished, Self::Manual];

    /// Parses the canonical discovery state name.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == value)
    }

    /// Renders the canonical discovery state name.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Vanished => "vanished",
            Self::Manual => "manual",
        }
    }
}

/// A stored upstream provider: one account instance bound to one exact, immutable definition
/// version (README §3).
#[derive(Debug, Clone)]
pub(crate) struct Provider {
    /// Row identifier.
    pub(crate) id: i64,

    /// Unique, renameable label; the immutable `id` is the provider's public identifier.
    pub(crate) name: String,

    /// Prefix prepended to every public model name.
    pub(crate) model_prefix: String,

    /// Upstream base URL; the only persisted instance URL after creation. Definitions whose URLs
    /// reference the base require it; a present exchange or provisioning extraction may replace
    /// it.
    pub(crate) base_url: Option<String>,

    /// Parsed plaintext credentials.
    pub(crate) credentials: StoredCredentials,

    /// Whether the provider serves traffic.
    pub(crate) is_enabled: bool,

    /// The exact definition version the provider's requests are interpreted against.
    pub(crate) definition: ProviderDefinition,
}

impl Provider {
    /// The credential kind this row carries.
    #[must_use]
    pub(crate) fn credential_kind(&self) -> CredentialKind {
        self.credentials.kind()
    }
}

/// A stored model row.
#[derive(Debug, Clone)]
pub(crate) struct Model {
    /// Row identifier: an `AUTOINCREMENT` number that is never recycled, so a deleted row's
    /// identity can never be reattached to a later row.
    pub(crate) id: i64,

    /// Exact upstream model identifier.
    pub(crate) upstream_id: String,

    /// Public routing ID, maintained at write time.
    pub(crate) public_id: String,

    /// Non-`None` explicitly sets the public routing ID directly, bypassing the provider's
    /// `model_prefix`.
    pub(crate) id_override: Option<String>,

    /// Whether the model is enabled to serve traffic under its provider.
    pub(crate) is_enabled: bool,

    /// Discovery lifecycle state of the row.
    pub(crate) discovery_state: DiscoveryState,
}

/// A provider row joined with the model row a public ID resolved to.
#[derive(Debug, Clone)]
pub(crate) struct ModelRouting {
    /// Owning provider.
    pub(crate) provider: Provider,

    /// Resolved model row.
    pub(crate) model: Model,

    /// Whether the model effectively serves traffic ([`Model::is_effectively_enabled`]).
    pub(crate) is_effectively_enabled: bool,
}

impl Model {
    /// The public routing ID the model carries: an explicit override directly replaces the ID
    /// without prefix, else `model_prefix + upstream_id`.
    pub(crate) fn public_id(
        model_prefix: &str,
        upstream_id: &str,
        id_override: Option<&str>,
    ) -> String {
        match id_override {
            Some(id) => id.to_string(),
            None => format!("{model_prefix}{upstream_id}"),
        }
    }

    /// The model's effective serving status — the one definition routing and the enabled-model
    /// listing share, and the definition a management client derives from the row's exposed
    /// inputs rather than reading it off the wire: true if and only if the owning provider is
    /// enabled, the model itself is enabled, and the row is not `vanished`. The provider's
    /// `enabled` flag serves as the master switch, and a vanished row has its service suspended
    /// regardless of its configured switch — without the switch ever being written (discovery
    /// only writes `discovery_state`).
    pub(crate) fn is_effectively_enabled(&self, is_provider_enabled: bool) -> bool {
        is_provider_enabled && self.is_enabled && self.discovery_state != DiscoveryState::Vanished
    }
}
