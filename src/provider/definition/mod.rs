//! Provider definitions: the immutable, one-format connection recipes every provider instance and
//! OAuth flow references (README §3, §5).
//!
//! A definition decomposes into wire format (code, the five format codecs), connection
//! configuration (data, the descriptors in this module), and credentials (data, the provider
//! rows). The engines never special-case a provider; these descriptor vocabularies are the only
//! extension surface, and the management write boundary validates a payload once, at write time
//! ([`validate::DefinitionPayload::validate`]).

pub(crate) mod credential;
pub(crate) mod discovery;
pub(crate) mod eval;
pub(crate) mod inference;
pub(crate) mod provisioning;
pub(crate) mod template;
pub(crate) mod validate;

use crate::format::UpstreamFormat;
use serde::{Deserialize, Serialize};
use std::num::NonZeroI64;
pub(crate) use validate::ValidationError;

/// The payload of one provider definition version: the data `provider-definitions:create` and
/// `provider-definitions:update` carry, and what the `format`, `inference`, `credential`,
/// `provisioning`, and `discovery` columns of the `provider_definitions` table store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DefinitionPayload {
    /// The one upstream wire format every inference of this definition speaks.
    pub format: UpstreamFormat,

    /// The inference request descriptor.
    pub inference: inference::Inference,

    /// The credential-acquisition descriptor.
    pub credential: credential::CredentialAcquisition,

    /// The optional ordered provisioning sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisioning: Option<Vec<provisioning::ProvisioningStep>>,

    /// The optional upstream model-list discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovery: Option<discovery::Discovery>,
}

/// One immutable provider definition version: the stored `(name, version)` row plus its parsed
/// payload (README §3).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderDefinition {
    /// The definition's name; versions accumulate under it.
    pub(crate) name: String,

    /// The definition's positive, monotonically increasing version.
    pub(crate) version: NonZeroI64,

    /// The definition's parsed payload.
    pub(crate) payload: DefinitionPayload,

    /// Creation timestamp in UTC microseconds.
    pub(crate) created_at: i64,
}

impl ProviderDefinition {
    /// The credential kind this definition's recipe describes, shared with the provider row's
    /// credential discriminator.
    #[must_use]
    pub(crate) fn credential_kind(&self) -> crate::provider::CredentialKind {
        match self.payload.credential {
            credential::CredentialAcquisition::ApiKey => crate::provider::CredentialKind::ApiKey,
            credential::CredentialAcquisition::OAuth { .. } => {
                crate::provider::CredentialKind::OAuth
            }
        }
    }
}
