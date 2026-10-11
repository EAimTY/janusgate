//! Provider definitions: the immutable, one-format connection recipes every provider instance and
//! OAuth flow references (`docs/design/storage.md` and `docs/design/providers.md`).
//!
//! A definition decomposes into wire format (code, the five format codecs), connection
//! configuration (data, the descriptors in this module), and credentials (data, the provider
//! rows). The engines never special-case a provider; these descriptor vocabularies are the only
//! extension surface, and the management write boundary validates a payload once, at write time
//! ([`DefinitionPayload::validate`]).

pub(crate) mod compaction;
pub(crate) mod credential;
pub(super) mod discovery;
pub(crate) mod eval;
mod expressions;
pub(crate) mod inference;
pub(super) mod provisioning;
pub(super) mod quota;
pub(super) mod quota_exhaustion;
mod quota_validation;
pub(crate) mod template;
mod validate;

#[cfg(test)]
mod tests;

use crate::format::UpstreamFormat;
use serde::{Deserialize, Serialize};
use std::num::NonZeroI64;
pub(crate) use validate::ValidationError;

/// The payload of one provider definition version: the data `provider-definitions:create` and
/// `provider-definitions:update` carry, and what the `format`, `inference`, `credential`,
/// `provisioning`, `discovery`, and `compaction` columns of the `provider_definitions` table
/// store.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(crate) struct DefinitionPayload {
    /// The one upstream wire format every inference of this definition speaks.
    pub(crate) format: UpstreamFormat,

    /// The inference request descriptor.
    pub(crate) inference: inference::Inference,

    /// The credential-acquisition descriptor.
    pub(crate) credential: credential::CredentialAcquisition,

    /// The optional ordered provisioning sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) provisioning: Option<Vec<provisioning::ProvisioningStep>>,

    /// The optional upstream model-list discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) discovery: Option<discovery::Discovery>,

    /// The optional read-only upstream account quota query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) quota: Option<quota::QuotaDescriptor>,

    /// The optional remote-compaction section (`docs/design/providers.md`): strategy, budgets,
    /// per-mechanism declarations, and the operations executing them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) compaction: Option<compaction::CompactionSection>,
}

impl DefinitionPayload {
    /// Whether any descriptor expression of this payload references the instance base URL — the
    /// [`template::ValueExpr::Base`] part (`docs/design/storage.md`). A definition so addressed
    /// evaluates every such expression against the bound provider row's stored base URL, so a
    /// provider bound to it must carry one.
    ///
    /// The walk covers every expression member whose evaluation environment carries the base
    /// URL — the inference and streaming URLs, the inference header-row values, the envelope's
    /// string leaves, every control request's full expression surface (URL, header-row
    /// values, query-row values, form-field values, and JSON string leaves) across the
    /// credential section's device-authorization, token-exchange, and refresh-grant requests,
    /// the provisioning steps and their polls, the discovery and quota requests, and the
    /// compaction section's declared mechanisms through [`compaction::MechanismDeclarations::
    /// enumerated`] (a declaration member joins this walk through it) plus the token-count
    /// operation — and must be extended alongside any new member of the manually walked set:
    /// the requirement checks consult only this predicate, so a missed expression is accepted
    /// with no base URL and every later evaluation of it fails as stored drift. The authorize
    /// URL and its extra parameters are excluded: the write-time validation bars the base
    /// reference at the authorize-URL stage (`docs/design/providers.md`), so those expressions can
    /// never evaluate against a base URL.
    #[must_use]
    pub(crate) fn references_base(&self) -> bool {
        self.base_capable_surfaces()
            .into_iter()
            .any(|surface| surface.references(&template::ValueExpr::Base))
    }

    /// Enumerates each descriptor surface that can use a stored or working base URL.
    /// Authorization URLs and extra parameters are excluded because their stage forbids Base.
    fn base_capable_surfaces(&self) -> Vec<expressions::ExpressionSurface<'_>> {
        use compaction::Declaration;
        use expressions::ExpressionSurface;
        let mut surfaces = vec![
            ExpressionSurface::String(&self.inference.url),
            ExpressionSurface::Headers(&self.inference.headers),
        ];
        surfaces.extend(
            self.inference
                .stream_url
                .as_ref()
                .map(ExpressionSurface::String),
        );
        surfaces.extend(
            self.inference
                .envelope
                .as_ref()
                .map(ExpressionSurface::Template),
        );
        if let credential::CredentialAcquisition::OAuth(recipe) = &self.credential {
            if let credential::Grant::Device { authorization, .. } = &recipe.grant {
                surfaces.push(ExpressionSurface::Control(&authorization.request));
            }
            surfaces.push(ExpressionSurface::Control(&recipe.exchange.request));
            if let credential::TokenRefresh::Grant { request, .. } = &recipe.refresh {
                surfaces.push(ExpressionSurface::Control(request));
            }
        }
        if let Some(steps) = &self.provisioning {
            for step in steps {
                surfaces.push(ExpressionSurface::Control(&step.request));
                surfaces.extend(
                    step.poll
                        .as_ref()
                        .map(|poll| ExpressionSurface::Control(&poll.request)),
                );
            }
        }
        surfaces.extend(
            self.discovery
                .as_ref()
                .map(|discovery| ExpressionSurface::Control(&discovery.request)),
        );
        surfaces.extend(
            self.quota
                .as_ref()
                .map(|quota| ExpressionSurface::Control(&quota.request)),
        );
        if let Some(compaction) = &self.compaction {
            for (_, declaration) in compaction.mechanisms.enumerated() {
                match declaration {
                    Some(Declaration::Standalone(standalone)) => {
                        if let Some(operation) = &standalone.operation {
                            surfaces.push(ExpressionSurface::String(&operation.url));
                            surfaces.push(ExpressionSurface::Headers(&operation.headers));
                            surfaces.extend(
                                operation.envelope.as_ref().map(ExpressionSurface::Template),
                            );
                        }
                    }

                    Some(Declaration::Overlay(overlay)) => {
                        if let Some(headers) = overlay.overlay_headers.as_deref() {
                            surfaces.push(ExpressionSurface::Headers(headers));
                        }
                    }

                    // The ride-along declarations carry no operation data: the trigger and the
                    // automatic controls ride the ordinary inference request.
                    Some(Declaration::RideAlong(_)) | None => {}
                }
            }

            if let Some(operation) = &compaction.count_tokens {
                surfaces.push(ExpressionSurface::String(&operation.url));
                surfaces.push(ExpressionSurface::Headers(&operation.headers));
                surfaces.extend(operation.envelope.as_ref().map(ExpressionSurface::Template));
            }
        }
        surfaces
    }
}

/// One immutable provider definition version: the stored `(name, version)` row plus its parsed
/// payload (`docs/design/storage.md`).
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

    /// The payload's OAuth recipe, when the credential acquisition is OAuth — the single
    /// deconstruction every OAuth engine stage shares. The accessor itself only reports
    /// absence: stored drift — an OAuth flow or renewal whose definition carries no OAuth
    /// credential — lives in the caller's error domain.
    #[must_use]
    pub(crate) fn oauth_recipe(&self) -> Option<&credential::OAuthRecipe> {
        match &self.payload.credential {
            credential::CredentialAcquisition::OAuth(recipe) => Some(recipe),
            credential::CredentialAcquisition::ApiKey => None,
        }
    }
}
