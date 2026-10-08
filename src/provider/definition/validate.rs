//! Write-time validation of a provider definition payload (README §5).
//!
//! The SQL schema enforces only the repository-wide constraint boundary; the management boundary
//! enforces everything else once, at write time: descriptor structure, stage-available value
//! kinds, slot namespace agreement, envelope and control-request body shapes, OAuth section
//! agreement, and the per-format render-option sets. Runtime never re-validates — a violation
//! that surfaces anyway is stored drift and fails fast.

use super::{
    compaction::{
        CompactionSection, CountTokensOperation, Declaration, StandaloneCompactOperation,
    },
    credential::{CredentialAcquisition, Grant, OAuthRecipe, Pkce, TokenRefresh},
    discovery::{CapabilityExtraction, DiscoveryExtract, Pagination, ThinkingLevelsExtraction},
    expressions::{ExpressionMember, ExpressionSurface},
    provisioning::{ProvisionExtraction, ProvisioningStep},
    template::{
        ControlBody, ControlMethod, ControlRequest, ExtractionShape, ParamRow, ResponseDescriptor,
        ResponseEncoding, ResponsePath, ResponseTarget as Target, StringExpr, TemplateNode,
        ValueExpr,
    },
};
use crate::{
    compaction_model::mechanism::{Mechanism, MechanismSupport},
    format::{RenderOption, UpstreamFormat},
};
use std::collections::BTreeSet;
use vec1::Vec1;
/// A definition payload rejection: one message naming the offending descriptor element, rendered
/// verbatim to the management client as an invalid-request rejection.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub(crate) struct ValidationError {
    /// The human-readable rejection message.
    message: String,
}

impl ValidationError {
    /// Builds the rejection naming the offending element.
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The descriptor stages whose expressions carry distinct stage-available value vocabularies
/// (README §5.1: OAuth values are available only at stages that produce them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The authorize-URL building of a flow's begin: the base URL and its extra parameters.
    AuthorizeUrl,

    /// The inference dispatch: URLs, headers, and the envelope.
    Inference,

    /// The device authorization request of a flow's begin.
    DeviceAuthorization,

    /// The token exchange of an authorization-code grant.
    ExchangeAuthorizationCode,

    /// The token exchange and polling requests of a device grant.
    ExchangeDevice,

    /// The refresh grant.
    RefreshGrant,

    /// The provisioning steps and their long-running operation polls.
    Provisioning,

    /// The upstream model-list discovery.
    Discovery,

    /// The read-only quota query, with current credentials only.
    Quota,

    /// A native compaction operation of the definition's compaction section: the standalone
    /// compact operation and the Messages declarations' overlay headers.
    CompactionOperation,

    /// The compaction section's optional native token-count operation.
    CompactionCount,
}

impl Stage {
    /// Whether the stage's expressions may carry the value part; the constant configuration
    /// kinds — literals and the configured client credentials — are available exactly where the
    /// stage's evaluation environment carries them, and the flow-state kinds are available
    /// exactly at the stages that produce or consume them. This table and the per-stage
    /// [`ValueEnv`](super::eval::ValueEnv) constructors in `eval.rs` describe the same per-stage
    /// value sets from the two sides — write-time admission and runtime supply — and must move
    /// together: a part admitted here but never supplied is stored drift that fails every
    /// dispatch at runtime.
    const fn allows(self, part: &ValueExpr) -> bool {
        match part {
            ValueExpr::Literal { .. } | ValueExpr::Coalesce { .. } => true,

            // Only the stage constructors that carry the configured client credentials: the
            // authorize-URL build, the device authorization, both token exchanges, the refresh
            // grant, and provisioning. Inference, discovery, the quota query, and the compaction
            // operations never receive them.
            ValueExpr::ClientId | ValueExpr::ClientSecret => matches!(
                self,
                Self::AuthorizeUrl
                    | Self::DeviceAuthorization
                    | Self::ExchangeAuthorizationCode
                    | Self::ExchangeDevice
                    | Self::RefreshGrant
                    | Self::Provisioning
            ),

            ValueExpr::Base => !matches!(self, Self::AuthorizeUrl),

            ValueExpr::Secret => matches!(
                self,
                Self::Inference
                    | Self::Provisioning
                    | Self::Discovery
                    | Self::Quota
                    | Self::CompactionOperation
                    | Self::CompactionCount
            ),

            ValueExpr::Slot { .. } => !matches!(self, Self::AuthorizeUrl),

            ValueExpr::Model | ValueExpr::RequestUuid => matches!(
                self,
                Self::Inference | Self::CompactionOperation | Self::CompactionCount
            ),

            ValueExpr::AuthorizationCode | ValueExpr::CodeVerifier | ValueExpr::RedirectUri => {
                matches!(self, Self::ExchangeAuthorizationCode)
            }

            ValueExpr::OAuthState => matches!(self, Self::AuthorizeUrl),

            ValueExpr::DeviceCode => matches!(self, Self::ExchangeDevice),

            ValueExpr::RefreshToken => matches!(self, Self::RefreshGrant),
        }
    }
}

/// The payload of one provider definition version, whose structure this module validates; the
/// type itself lives beside its sibling descriptors in the parent module.
use super::DefinitionPayload;

/// The slot namespace of one definition: every name the OAuth recipe writes, against which every
/// slot reference resolves.
#[derive(Debug, Default)]
struct SlotNamespace {
    /// The defined slot names.
    names: BTreeSet<String>,
}

impl SlotNamespace {
    /// Defines one initial slot name, rejecting an empty name and a duplicate definition: the
    /// initial slots are one named map, so two initializers of one name could never both run.
    fn define(&mut self, name: &str, context: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(format!(
                "{context} slot name must not be empty"
            )));
        }

        if !self.names.insert(name.to_string()) {
            return Err(ValidationError::new(format!(
                "{context} defines the slot `{name}` twice"
            )));
        }

        Ok(())
    }

    /// Records one slot name a stage target writes. Several stages may write one slot —
    /// Antigravity's onboarding poll writes the project slot its lookup step may already have
    /// placed — so a repeated target name is recorded, not rejected.
    fn define_target(&mut self, name: &str, context: &str) -> Result<(), ValidationError> {
        if name.is_empty() {
            return Err(ValidationError::new(format!(
                "{context} slot name must not be empty"
            )));
        }

        self.names.insert(name.to_string());

        Ok(())
    }

    /// Rejects a reference to a slot the recipe never writes.
    fn require_defined(&self, name: &str, what: &str) -> Result<(), ValidationError> {
        if self.names.contains(name) {
            Ok(())
        } else {
            Err(ValidationError::new(format!(
                "{what} references the slot `{name}`, which no initial slot, exchange target, \
                 identity-token claim, or provisioning target defines"
            )))
        }
    }
}

/// The per-format closed render-option sets (README §5.2): a codec rejects options outside its
/// set at write time. Growing a set is the documented extension path.
#[must_use]
fn allowed_render_options(format: UpstreamFormat) -> &'static [RenderOption] {
    match format {
        UpstreamFormat::OpenAiResponse => &[RenderOption::OmitMaxOutputTokens],
        UpstreamFormat::OpenAiChatCompletion
        | UpstreamFormat::GeminiGenerateContent
        | UpstreamFormat::GeminiInteraction
        | UpstreamFormat::AnthropicMessages => &[],
    }
}

/// The authorize-URL parameter names the authorize-URL builder appends itself — the
/// counterpart list to `build_authorize_url` in `src/provider/oauth/flow.rs`, carrying the
/// names spelled exactly as the builder appends them and matched case-sensitively. An
/// `extra_params` row carrying one would render a duplicate query parameter whose resolution
/// is the upstream's, so the write boundary rejects the name.
const BUILDER_OWNED_AUTHORIZE_PARAMS: [&str; 6] = [
    "response_type",
    "client_id",
    "redirect_uri",
    "code_challenge",
    "code_challenge_method",
    "state",
];

impl DefinitionPayload {
    /// Validates the payload against every write-time rule (README §5), in document order of
    /// the descriptor sections.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError`] naming the first offending descriptor element.
    pub(crate) fn validate(&self) -> Result<(), ValidationError> {
        // The slot namespace every slot reference resolves against: the names the OAuth recipe
        // writes — initial slots, exchange slots, identity-claim slots, provisioning slots.
        let mut slots = SlotNamespace::default();

        if let CredentialAcquisition::OAuth(recipe) = &self.credential {
            let (initial_slots, exchange, id_token_claims) = (
                &recipe.initial_slots,
                &recipe.exchange,
                &recipe.id_token_claims,
            );

            for initializer in initial_slots {
                slots.define(&initializer.name, "initial slot")?;
            }

            for field in &exchange.response.fields {
                if let Target::Slot { name } = &field.target {
                    slots.define_target(name, "exchange target")?;
                }
            }

            // The slot names this one order-folded claim list writes: a repeat inside it is
            // the intra-stage duplicate target, while a name another stage also writes stays
            // allowed through `define_target`.
            let mut claimed_slots: BTreeSet<String> = BTreeSet::new();

            for claim in id_token_claims {
                // The empty chain addresses the whole JWT payload, whose text read is always
                // `None`: the claim could never land, the same dead configuration the other
                // root-addressing vocabularies reject, so the chain must name at least one
                // member.
                if claim.claim.0.is_empty() {
                    return Err(ValidationError::new(
                        "identity-token claim path must not be empty",
                    ));
                }

                validate_path(&claim.claim, "identity-token claim")?;

                // The claims apply in order and a later resolved claim silently overwrites
                // the earlier one, so the earlier mapping is dead configuration.
                if !claimed_slots.insert(claim.slot.clone()) {
                    return Err(ValidationError::new(format!(
                        "identity-token claims store into the slot `{}` twice: the claims apply \
                         in order and the earlier one is dead configuration",
                        claim.slot
                    )));
                }

                slots.define_target(&claim.slot, "identity-token claim slot")?;
            }
        }

        if let Some(steps) = &self.provisioning
            && self.credential.is_oauth()
        {
            for extraction in provision_extractions(steps) {
                if let Target::Slot { name } = &extraction.target {
                    slots.define_target(name, "provisioning target")?;
                }
            }
        }

        self.validate_inference(&slots)?;
        self.validate_credential(&slots)?;
        self.validate_provisioning(&slots)?;
        self.validate_discovery(&slots)?;

        if let Some(quota) = &self.quota {
            validate_control_request(&quota.request, Stage::Quota, &slots)?;
            super::quota_validation::validate(quota)?;
        }

        self.validate_compaction(&slots)?;

        Ok(())
    }

    /// Validates the inference descriptor: stage-available expressions, the render-option set,
    /// and the envelope's exactly-one-`Body` shape.
    fn validate_inference(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        let inference = &self.inference;

        require_non_empty_expr(&inference.url, "inference URL")?;
        if let Some(classifier) = &inference.quota_exhaustion {
            classifier.validate()?;
        }

        if let Some(stream_url) = &inference.stream_url {
            require_non_empty_expr(stream_url, "inference streaming URL")?;
        }

        for option in &inference.render_options {
            if !allowed_render_options(self.format).contains(option) {
                return Err(ValidationError::new(format!(
                    "inference render option `{}` is not part of the `{}` format's set",
                    option.as_str(),
                    self.format.as_str()
                )));
            }
        }

        let mut walk = ExpressionWalk::new(Stage::Inference, slots);

        walk.string_expr(&inference.url, "inference URL")?;

        if let Some(stream_url) = &inference.stream_url {
            walk.string_expr(stream_url, "inference streaming URL")?;
        }

        walk.headers(&inference.headers, "inference header")?;

        if let Some(envelope) = &inference.envelope {
            let body_count = walk.template_node(envelope, "inference envelope")?;

            if body_count != 1 {
                return Err(ValidationError::new(format!(
                    "inference envelope carries {body_count} body placeholders; \
                     exactly one is required"
                )));
            }
        }

        if let Some(response_unwrap) = &inference.response_unwrap
            && response_unwrap.is_empty()
        {
            return Err(ValidationError::new(
                "inference response unwrap member must not be empty",
            ));
        }

        Ok(())
    }

    /// Validates the credential-acquisition descriptor: the OAuth sections' expressions, their
    /// response mappings, and the grant/refresh agreement.
    fn validate_credential(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        let CredentialAcquisition::OAuth(recipe) = &self.credential else {
            return Ok(());
        };

        let OAuthRecipe {
            client_id,
            client_secret,
            grant,
            exchange,
            refresh,
            ..
        } = &**recipe;

        if client_id.is_empty() {
            return Err(ValidationError::new("OAuth client ID must not be empty"));
        }

        if client_secret.as_ref().is_some_and(String::is_empty) {
            return Err(ValidationError::new(
                "OAuth client secret must not be empty when present",
            ));
        }

        // The client credentials render verbatim wherever a header row references them
        // (README §5.1), so their bytes must be header-value bytes: a control byte — a pasted
        // trailing newline foremost — would fail every such dispatch at runtime as a
        // transport failure misclassified as upstream unavailability, the same
        // misclassification the header-name rule exists for. URL and form destinations
        // percent-encode their components, so the header contract is the strictest one the
        // values face, and the recipe carries it once.
        if http::HeaderValue::from_str(client_id).is_err() {
            return Err(ValidationError::new(
                "OAuth client ID carries text that is not a valid HTTP header value",
            ));
        }

        if let Some(client_secret) = client_secret
            && http::HeaderValue::from_str(client_secret).is_err()
        {
            return Err(ValidationError::new(
                "OAuth client secret carries text that is not a valid HTTP header value",
            ));
        }

        // A credential-stage expression referencing the `client_secret` part while the recipe
        // configures none is guaranteed drift: every stage constructor that admits the part
        // receives the configured secret, so a secret-less credential leaves the part absent
        // at every admitting stage and the reference fails every expression evaluation (README
        // §5.1) — the same stored-drift shape the provisioning `secret` rule at the end of
        // this method enforces for the flow's token. The check reads every admitting surface
        // in payload document order.
        if client_secret.is_none() {
            let mut stranded: Vec<(&str, ExpressionSurface<'_>)> = Vec::new();

            if let Grant::AuthorizationCode {
                authorize_url,
                extra_params,
                ..
            } = grant
            {
                stranded.push(("authorize URL", ExpressionSurface::String(authorize_url)));
                stranded.push((
                    "authorize-URL parameter",
                    ExpressionSurface::Parameters(extra_params),
                ));
            }

            if let Grant::Device { authorization, .. } = grant {
                stranded.push((
                    "device authorization",
                    ExpressionSurface::Control(&authorization.request),
                ));
            }

            stranded.push((
                "token exchange",
                ExpressionSurface::Control(&exchange.request),
            ));

            if let TokenRefresh::Grant { request, .. } = refresh {
                stranded.push(("token refresh", ExpressionSurface::Control(request)));
            }

            if let Some(steps) = &self.provisioning {
                for step in steps {
                    stranded.push((
                        "provisioning step",
                        ExpressionSurface::Control(&step.request),
                    ));

                    if let Some(poll) = &step.poll {
                        stranded.push((
                            "provisioning poll",
                            ExpressionSurface::Control(&poll.request),
                        ));
                    }
                }
            }

            for (what, surface) in stranded {
                if surface.references(&ValueExpr::ClientSecret) {
                    return Err(ValidationError::new(format!(
                        "{what} references the `client_secret` value part, but the credential \
                         configures no client secret: every stage that admits the part \
                         receives the configured secret, so the reference is unsatisfiable \
                         and fails every evaluation"
                    )));
                }
            }
        }

        match grant {
            Grant::AuthorizationCode {
                authorize_url,
                redirect_uri,
                extra_params,
                ..
            } => {
                require_non_empty_expr(authorize_url, "authorize URL")?;

                if redirect_uri.is_empty() {
                    return Err(ValidationError::new("OAuth redirect URI must not be empty"));
                }

                for row in extra_params {
                    if BUILDER_OWNED_AUTHORIZE_PARAMS.contains(&row.name.as_str()) {
                        return Err(ValidationError::new(format!(
                            "authorize-URL parameter `{}` duplicates a parameter the \
                             authorize-URL builder appends itself: the duplicate row renders \
                             two same-named query parameters whose resolution is the \
                             upstream's — a duplicated `redirect_uri` breaks RFC 6749 §4.1.3's \
                             identical-redirect_uri requirement on every exchange, and a \
                             duplicated `state` makes the flow's completion depend on which \
                             spelling the upstream echoes back",
                            row.name
                        )));
                    }
                }

                let mut walk = ExpressionWalk::new(Stage::AuthorizeUrl, slots);

                walk.string_expr(authorize_url, "authorize URL")?;
                walk.params(extra_params, "authorize-URL parameter")?;
            }

            Grant::Device {
                authorization,
                expires_in_fallback_seconds,
            } => {
                validate_control_request(
                    &authorization.request,
                    Stage::DeviceAuthorization,
                    slots,
                )?;

                validate_response_descriptor(&authorization.response, "device authorization")?;

                // The duplicate-target rule is the shared response-descriptor rule above; the
                // loop aggregates the mapped targets for the required-shape checks and holds
                // every carried target to its stage's produced vocabulary.
                let mut targets = BTreeSet::new();

                for field in &authorization.response.fields {
                    let target = match &field.target {
                        Target::DeviceCode => "device_code",
                        Target::UserCode => "user_code",
                        Target::VerificationUri => "verification_uri",
                        Target::VerificationUriComplete => "verification_uri_complete",
                        Target::ExpiresInSeconds => "expires_in_seconds",
                        Target::IntervalSeconds => "interval_seconds",
                        _ => {
                            return Err(ValidationError::new(
                                "device authorization response carries a target its stage \
                                 does not produce",
                            ));
                        }
                    };

                    targets.insert(target);

                    if matches!(field.target, Target::DeviceCode | Target::UserCode)
                        && !field.required
                    {
                        return Err(ValidationError::new(format!(
                            "device authorization response must mark the `{target}` target required"
                        )));
                    }
                }

                if !targets.contains("device_code") || !targets.contains("user_code") {
                    return Err(ValidationError::new(
                        "device authorization response must map both the `device_code` and the \
                         `user_code` targets",
                    ));
                }

                if !targets.contains("verification_uri")
                    && !targets.contains("verification_uri_complete")
                {
                    return Err(ValidationError::new(
                        "device authorization response must map at least one verification URI \
                         target",
                    ));
                }

                // A device flow's lifetime must have exactly one guaranteed source: a reported
                // `expires_in` the response is required to carry, or the definition's explicit
                // fallback. An optional (or absent) expiry mapping without a fallback would let
                // an upstream omission leave the flow with no lifetime at all, so the state is
                // unrepresentable for a valid definition (README §5.3, §7.2).
                let maps_required_expiry = authorization.response.fields.iter().any(|field| {
                    matches!(field.target, Target::ExpiresInSeconds) && field.required
                });

                if expires_in_fallback_seconds.is_none() && !maps_required_expiry {
                    return Err(ValidationError::new(concat!(
                        "device grant must configure `expires_in_fallback_seconds` when its ",
                        "authorization response maps no `expires_in_seconds` target, or maps it \
                         optional",
                    )));
                }

                // RFC 8628 §3.1: the device-authorization request carries the `client_id` —
                // "REQUIRED, if the client is not authenticating with the authorization server
                // as described in Section 3.2.1 of [RFC6749]" — and a credential with no
                // `client_secret` never authenticates, so an authorization request that never
                // references the part dies at the upstream with `invalid_client` on every
                // flow's begin: the same required-carry shape the exchange and refresh
                // client-ID rules below enforce. A credential with a client secret
                // authenticates with it — the Basic/body scheme is the definition's business.
                if client_secret.is_none()
                    && !ExpressionSurface::Control(&authorization.request)
                        .references(&ValueExpr::ClientId)
                {
                    return Err(ValidationError::new(
                        "device grant without a client secret requires the device authorization \
                         request to reference the `client_id` value part: RFC 8628 §3.1 makes \
                         the client ID REQUIRED when the client is not authenticating with the \
                         authorization server (RFC 6749 §3.2.1), and a credential with no \
                         client secret never does",
                    ));
                }
            }
        }

        let exchange_stage = match grant.is_device() {
            true => Stage::ExchangeDevice,
            false => Stage::ExchangeAuthorizationCode,
        };

        validate_control_request(&exchange.request, exchange_stage, slots)?;

        // RFC 7636 §4.6: a grant that sent a code challenge must present the verifier at the
        // token endpoint. The stage vocabulary already allows the `code_verifier` part here;
        // when `s256` is configured the exchange request must actually reference it — the same
        // required-carry shape the device grant's target checks above enforce — or every flow
        // dies at the upstream with `invalid_grant`.
        if let Grant::AuthorizationCode {
            pkce: Pkce::S256, ..
        } = grant
            && !ExpressionSurface::Control(&exchange.request).references(&ValueExpr::CodeVerifier)
        {
            return Err(ValidationError::new(
                "authorization-code grant with `pkce: s256` requires the token exchange request \
                 to reference the `code_verifier` value part: RFC 7636 §4.6 presents the verifier \
                 at the token endpoint whenever a challenge was sent",
            ));
        }

        // The converse of the s256 rule: the begin mints the verifier exactly when the grant
        // declares PKCE S256 — its presence is the challenge pair's marker (RFC 7636 §4.2) —
        // so an exchange under `pkce: none` referencing the `code_verifier` part references a
        // value the stage environment never carries: stored drift failing every exchange's
        // evaluation (README §5.1), the same guaranteed-absent shape the client-secret rule
        // above enforces.
        if let Grant::AuthorizationCode {
            pkce: Pkce::None, ..
        } = grant
            && ExpressionSurface::Control(&exchange.request).references(&ValueExpr::CodeVerifier)
        {
            return Err(ValidationError::new(
                "authorization-code grant with `pkce: none` must not reference the \
                 `code_verifier` value part in its token exchange request: the begin mints \
                 the verifier exactly when the grant declares `s256`, so the reference is \
                 unsatisfiable and fails every exchange",
            ));
        }

        // RFC 6749 §4.1.3: the token request repeats the `redirect_uri`, identically, whenever
        // the authorization request included it — and the builder appends the definition's
        // `redirect_uri` to every authorize URL — so an exchange request that never references
        // the part dies at the upstream with `invalid_request` on every flow: the same
        // required-carry shape the verifier rule above enforces.
        if let Grant::AuthorizationCode { .. } = grant
            && !ExpressionSurface::Control(&exchange.request).references(&ValueExpr::RedirectUri)
        {
            return Err(ValidationError::new(
                "authorization-code grant requires the token exchange request to reference the \
                 `redirect_uri` value part: RFC 6749 §4.1.3 repeats the redirect URI at the token \
                 endpoint whenever the authorization request included it, and the builder appends \
                 it to every authorize URL",
            ));
        }

        // RFC 6749 §4.1.3: the token request presents the pasted authorization code — `code`
        // is REQUIRED — and an authorization-code flow is consumed by its first completion
        // attempt even when the exchange then fails (README §7.2), so an exchange request that
        // never references the part burns every flow on that attempt: the same required-carry
        // shape the verifier and redirect-URI rules above enforce.
        if let Grant::AuthorizationCode { .. } = grant
            && !ExpressionSurface::Control(&exchange.request)
                .references(&ValueExpr::AuthorizationCode)
        {
            return Err(ValidationError::new(
                "authorization-code grant requires the token exchange request to reference the \
                 `authorization_code` value part: RFC 6749 §4.1.3 makes the authorization code \
                 REQUIRED in the token request, and the flow is consumed by its first exchange \
                 attempt even when that attempt fails",
            ));
        }

        // RFC 8628 §3.4: the token request carries the device code — `device_code` is REQUIRED
        // — and under a device grant this same request doubles as the polling request, so an
        // exchange request that never references the part would ride non-terminal rejections
        // until the flow's own expiry and never exchange: the same required-carry shape the
        // authorization-code rules above enforce.
        if let Grant::Device { .. } = grant
            && !ExpressionSurface::Control(&exchange.request).references(&ValueExpr::DeviceCode)
        {
            return Err(ValidationError::new(
                "device grant requires the token exchange request to reference the \
                 `device_code` value part: RFC 8628 §3.4 makes the device code REQUIRED in the \
                 token request this grant re-posts as its polling request",
            ));
        }

        // RFC 6749 §4.1.3: the token request carries the `client_id` — "REQUIRED, if the client
        // is not authenticating with the authorization server as described in Section 3.2.1" —
        // and a credential with no `client_secret` never authenticates at the token endpoint,
        // so an exchange request that never references the part dies at the upstream with
        // `invalid_client` on every flow: the same required-carry shape the rules above
        // enforce. A credential with a client secret authenticates with it — the Basic/body
        // scheme is the definition's business.
        if client_secret.is_none()
            && !ExpressionSurface::Control(&exchange.request).references(&ValueExpr::ClientId)
        {
            return Err(ValidationError::new(
                "OAuth credential without a client secret requires the token exchange request \
                 to reference the `client_id` value part: RFC 6749 §4.1.3 makes the client ID \
                 REQUIRED when the client is not authenticating with the authorization server \
                 (§3.2.1), and a credential with no client secret never does",
            ));
        }

        // RFC 6749 §6: renewing an access token is a `grant_type=refresh_token` request
        // carrying the stored refresh token — `refresh_token` is REQUIRED — and the stage
        // vocabulary already allows the part exactly at the refresh grant, so a grant whose
        // request never references it dies at the upstream on every renewal. The rejection
        // code lands in the grant's `terminal_codes`, so the flow then misdiagnoses the
        // healthy provider as one whose grant is revoked or expired and needs
        // re-authorization — an endless re-authorize/refresh-fail loop: the same
        // required-carry shape the exchange rules above enforce. Scope: the grant mode only —
        // `reprovision` reuses the provisioning extraction contract and `none` carries no
        // request.
        if let TokenRefresh::Grant { request, .. } = refresh
            && !ExpressionSurface::Control(request).references(&ValueExpr::RefreshToken)
        {
            return Err(ValidationError::new(
                "refresh grant requires the token refresh request to reference the \
                 `refresh_token` value part: RFC 6749 §6 makes the refresh token REQUIRED in \
                 a refresh-token grant request",
            ));
        }

        // RFC 6749 §6: a refresh-token grant request re-authenticates the client the same way
        // the token endpoint requires (§4.1.3's §3.2.1 rule), so the secret-less credential's
        // renewal presents its client ID too — a grant whose request never references the part
        // dies at the upstream with `invalid_client` on every renewal: the same required-carry
        // shape the refresh-token rule above enforces. A credential with a client secret
        // authenticates its renewal with it.
        if let TokenRefresh::Grant { request, .. } = refresh
            && client_secret.is_none()
            && !ExpressionSurface::Control(request).references(&ValueExpr::ClientId)
        {
            return Err(ValidationError::new(
                "OAuth credential without a client secret requires the token refresh request \
                 to reference the `client_id` value part: RFC 6749 §6 re-authenticates the \
                 client as the token endpoint does, making the client ID REQUIRED when no \
                 secret authenticates (§4.1.3, §3.2.1)",
            ));
        }

        validate_response_descriptor(&exchange.response, "token exchange")?;

        // The runtime fold consumes exactly the exchange stage's target set (README §5.3): a
        // device-authorization or provisioning target in an exchange mapping is stored drift
        // the fold misclassifies as an internal error on every flow, so the boundary rejects
        // the spelling. The defaulted refresh mapping reuses this descriptor, so the check
        // covers the renewal's fold input too.
        for field in &exchange.response.fields {
            if !is_exchange_stage_target(&field.target) {
                return Err(ValidationError::new(format!(
                    "token exchange response carries the `{}` target its stage does not produce",
                    target_name(&field.target)
                )));
            }
        }

        validate_terminal_codes(&exchange.terminal_codes, "token exchange")?;

        // The access token must exist before the provider write: the exchange maps it, or a
        // later provisioning step supplies it.
        let provisions_access_token = exchange
            .response
            .fields
            .iter()
            .any(|field| matches!(field.target, Target::AccessToken))
            || self
                .provisioning
                .as_ref()
                .is_some_and(|steps| extractions_target(steps, &Target::AccessToken));

        if !provisions_access_token {
            return Err(ValidationError::new(
                "OAuth credential must receive an access token: map the `access_token` target \
                 in the exchange response or extract it in a provisioning step",
            ));
        }

        // The refresh token must have a guaranteed source before the refresh grant presents
        // it: a required exchange mapping, or a required extraction in an unconditional
        // provisioning step. The grant's request references the part mandatorily (the rule
        // below), so an optional-only source would let an upstream legitimately omit the
        // token — RFC 6749 §5.1 leaves it optional in a success response — and strand every
        // renewal on the absent stored value, while a required source fails the producing
        // flow outright and keeps the stored token's presence guaranteed: the same
        // guaranteed-source shape the device grant's expiry rule above enforces. A
        // conditional step's extraction cannot guarantee the token — the step runs only
        // while its guard slot is absent, which may coincide with the optional mapping's
        // omission.
        let produces_required_refresh_token = exchange
            .response
            .fields
            .iter()
            .any(|field| matches!(field.target, Target::RefreshToken) && field.required)
            || self.provisioning.as_ref().is_some_and(|steps| {
                steps.iter().any(|step| {
                    step.skip_if.is_none()
                        && (step.extract.iter().any(|extraction| {
                            matches!(extraction.target, Target::RefreshToken) && extraction.required
                        }) || step.poll.as_ref().is_some_and(|poll| {
                            poll.result_extract.iter().any(|extraction| {
                                matches!(extraction.target, Target::RefreshToken)
                                    && extraction.required
                            })
                        }))
                })
            });

        match refresh {
            TokenRefresh::Grant {
                request,
                response,
                terminal_codes,
            } => {
                if !produces_required_refresh_token {
                    return Err(ValidationError::new(
                        "refresh grant requires the credential's refresh token to have a \
                         guaranteed source: map the `refresh_token` target `required` in the \
                         exchange response or extract it through a `required` extraction in an \
                         unconditional provisioning step — an optional-only source admits an \
                         upstream that omits the token (RFC 6749 §5.1 leaves it optional in a \
                         success response), and the grant's mandatory `refresh_token` \
                         reference then fails every renewal on the absent stored token",
                    ));
                }

                validate_control_request(request, Stage::RefreshGrant, slots)?;

                if let Some(response) = response {
                    validate_response_descriptor(response, "token refresh")?;

                    // The declared mapping wholly replaces the exchange's as the renewal's
                    // fold input, so its targets are held to the same stage vocabulary: a
                    // target outside the set is stored drift the runtime fold misclassifies
                    // as an internal error on every renewal.
                    for field in &response.fields {
                        if !is_exchange_stage_target(&field.target) {
                            return Err(ValidationError::new(format!(
                                "token refresh response carries the `{}` target its stage \
                                 does not produce",
                                target_name(&field.target)
                            )));
                        }
                    }

                    // RFC 6749 §6: the refresh response carries the access token — REQUIRED —
                    // and a declared mapping wholly replaces the exchange's default as the
                    // renewal's fold input, so a declared mapping with no `access_token`
                    // target folds every renewal into a tokenless outcome: the lease is
                    // acquired and cleared first, and the renewal then dies on the missing
                    // token — a permanent failure on every dispatch of the provider past its
                    // first expiry.
                    if !response
                        .fields
                        .iter()
                        .any(|field| matches!(field.target, Target::AccessToken))
                    {
                        return Err(ValidationError::new(
                            "token refresh response must map the `access_token` target: a \
                             declared mapping replaces the exchange's default, and RFC 6749 §6 \
                             makes the access token REQUIRED in a refresh response",
                        ));
                    }
                } else {
                    // The renewal's fold input defaults to the exchange's response mapping, so
                    // that mapping is the effective refresh mapping and must carry the token
                    // row. The general access-token rule earlier in this method cannot stand in:
                    // it also admits a provisioning-supplied token, which a refresh grant never
                    // renews — a provisioning-only token source would wedge every renewal after
                    // that token's first expiry.
                    if !exchange
                        .response
                        .fields
                        .iter()
                        .any(|field| matches!(field.target, Target::AccessToken))
                    {
                        return Err(ValidationError::new(
                            "token refresh without a declared response reuses the exchange \
                             response's mapping, which must map the `access_token` target: \
                             RFC 6749 §6 makes the access token REQUIRED in a refresh response, \
                             and a token supplied only by provisioning is never renewed",
                        ));
                    }
                }

                validate_terminal_codes(terminal_codes, "token refresh")?;
            }

            TokenRefresh::Reprovision => {
                if self.provisioning.as_ref().is_none_or(Vec::is_empty) {
                    return Err(ValidationError::new(
                        "refresh mode `reprovision` requires a non-empty provisioning list",
                    ));
                }

                // The renewal rewrites the stored credentials' expiry, so the sequence must
                // produce a fresh absolute one: without an extraction the renewal would
                // persist `null` — the no-scheduled-refresh state — over a known expiry, and
                // the credential would silently never refresh again (README §5.4).
                if !self
                    .provisioning
                    .as_ref()
                    .is_some_and(|steps| extractions_target(steps, &Target::ExpiresAtUnixSeconds))
                {
                    return Err(ValidationError::new(
                        "refresh mode `reprovision` requires the provisioning sequence to \
                         extract an `expires_at_unix_seconds` target: a renewal without a \
                         renewed expiry would unschedule every future refresh",
                    ));
                }
            }

            TokenRefresh::None => {}
        }

        // The provisioning stage's `secret` value is the exchange's access token — the flow's
        // current token threaded through the completion (README §5.1, §5.4) — so an exchange
        // that maps no `access_token` target passes it absent, and a provisioning expression
        // referencing the part fails every flow's step with the stored-drift evaluation
        // error. A token supplied by provisioning is reachable through the slot its
        // extraction writes instead. The check lands after the refresh rules because the
        // provisioning section follows the credential sections in the payload document.
        if !exchange
            .response
            .fields
            .iter()
            .any(|field| matches!(field.target, Target::AccessToken))
            && let Some(steps) = &self.provisioning
            && steps.iter().any(|step| {
                ExpressionSurface::Control(&step.request).references(&ValueExpr::Secret)
                    || step.poll.as_ref().is_some_and(|poll| {
                        ExpressionSurface::Control(&poll.request).references(&ValueExpr::Secret)
                    })
            })
        {
            return Err(ValidationError::new(
                "provisioning references the `secret` value part, but the exchange response \
                 maps no `access_token` target: the provisioning stage's secret is the \
                 exchange's access token, so reference the slot a provisioning extraction \
                 writes instead",
            ));
        }

        Ok(())
    }

    /// Validates the provisioning descriptor: the OAuth-only rule, the non-empty sequence, the
    /// stage-available expressions, control-request shapes, extraction shapes, and the poll
    /// configurations. Provisioning runs after a token exchange, so an API-key recipe never
    /// carries one.
    fn validate_provisioning(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        if let Some(steps) = &self.provisioning {
            if steps.is_empty() {
                return Err(ValidationError::new(
                    "provisioning must carry at least one step when present",
                ));
            }

            if !self.credential.is_oauth() {
                return Err(ValidationError::new(
                    "provisioning requires an OAuth credential: it runs after the token exchange",
                ));
            }
        }

        for step in self.provisioning.as_deref().unwrap_or_default() {
            validate_control_request(&step.request, Stage::Provisioning, slots)?;

            if let Some(skip_if) = &step.skip_if {
                slots.require_defined(skip_if, "provisioning step guard")?;
            }

            reject_duplicate_targets(
                step.extract.iter().map(|extraction| &extraction.target),
                "provisioning step",
            )?;

            for extraction in &step.extract {
                validate_provision_extraction(extraction, &step.response_encoding)?;
            }

            if let Some(poll) = &step.poll {
                validate_control_request(&poll.request, Stage::Provisioning, slots)?;

                // An empty done path resolves the poll response's root, whose truthy read is
                // always false: the poll would end only at budget exhaustion, never
                // completing — so the chain must name at least one member.
                if poll.done_path.0.is_empty() {
                    return Err(ValidationError::new(
                        "provisioning poll done path must not be empty",
                    ));
                }

                validate_path(&poll.done_path, "provisioning poll done")?;

                // The error path's empty chain is the configured no-error-inspection
                // (README §5.4); a carried chain still names only non-empty members.
                validate_path(&poll.error_path, "provisioning poll error")?;

                reject_duplicate_targets(
                    poll.result_extract
                        .iter()
                        .map(|extraction| &extraction.target),
                    "provisioning poll",
                )?;

                for extraction in &poll.result_extract {
                    validate_provision_extraction(extraction, &poll.response_encoding)?;
                }
            }
        }

        Ok(())
    }

    /// Validates the discovery descriptor: stage-available expressions and the extraction and
    /// pagination shapes.
    fn validate_discovery(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        let Some(discovery) = &self.discovery else {
            return Ok(());
        };

        validate_control_request(&discovery.request, Stage::Discovery, slots)?;

        match &discovery.extract {
            DiscoveryExtract::Array {
                array_at,
                id_field,
                strip_prefix,
                capabilities,
            } => {
                validate_path(array_at, "discovery array path")?;

                if id_field.0.is_empty() {
                    return Err(ValidationError::new(
                        "discovery identifier field path must not be empty",
                    ));
                }

                validate_path(id_field, "discovery identifier field")?;

                if strip_prefix.as_ref().is_some_and(String::is_empty) {
                    return Err(ValidationError::new(
                        "discovery identifier prefix must not be empty when present",
                    ));
                }

                if let Some(capabilities) = capabilities {
                    validate_capability_extraction(capabilities)?;
                }
            }

            DiscoveryExtract::MapKeys { at } => validate_path(at, "discovery object path")?,
        }

        match &discovery.pagination {
            None | Some(Pagination::None) => {}

            Some(Pagination::Token {
                request_param,
                response_path,
                ..
            }) => {
                if request_param.is_empty() {
                    return Err(ValidationError::new(
                        "discovery pagination request parameter must not be empty",
                    ));
                }

                // An empty chain addresses the page root, which can never continue the walk:
                // the root object reads as a malformed token on every page. The walk would
                // fail on page one where the budget guard was the designed ending.
                if response_path.0.is_empty() {
                    return Err(ValidationError::new(
                        "discovery pagination response path must not be empty",
                    ));
                }

                validate_path(response_path, "discovery pagination response")?;
            }

            Some(Pagination::Cursor {
                has_more_path,
                last_id_path,
                request_param,
                ..
            }) => {
                if request_param.is_empty() {
                    return Err(ValidationError::new(
                        "discovery pagination request parameter must not be empty",
                    ));
                }

                // An empty has-more chain addresses the page root, whose truthy read is
                // always false: the walk would end after page one with the budget guard never
                // firing — the silent truncation README §5.5 forbids — so the chain must name
                // at least one member.
                if has_more_path.0.is_empty() {
                    return Err(ValidationError::new(
                        "discovery has-more path must not be empty",
                    ));
                }

                validate_path(has_more_path, "discovery has-more")?;

                if last_id_path.0.is_empty() {
                    return Err(ValidationError::new(
                        "discovery last-identifier path must not be empty",
                    ));
                }

                validate_path(last_id_path, "discovery last-identifier")?;
            }
        }

        Ok(())
    }

    /// Validates the compaction section: the per-format mechanism gating, the
    /// supported-requires-operation shape, the classifier rows, and the operation expressions'
    /// staging.
    fn validate_compaction(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        let Some(compaction) = &self.compaction else {
            return Ok(());
        };

        let CompactionSection { mechanisms, .. } = compaction;

        // The format gating: a Responses mechanism speaks only through an `openai_response`
        // definition and a Messages mechanism only through an `anthropic_messages` one — a
        // declaration on the wrong format is an operation the definition can never render
        // (README §5).
        for (mechanism, declaration) in mechanisms.enumerated() {
            let required_format = required_format(mechanism);

            if declaration.is_some() && self.format != required_format {
                return Err(ValidationError::new(format!(
                    "the `{}` compaction declaration requires the `{}` format, but the \
                     definition speaks `{}`",
                    mechanism.as_str(),
                    required_format.as_str(),
                    self.format.as_str()
                )));
            }
        }

        if compaction.count_tokens.is_some() && self.format != UpstreamFormat::AnthropicMessages {
            return Err(ValidationError::new(format!(
                "the `count_tokens` compaction operation requires the `anthropic_messages` \
                 format, but the definition speaks `{}`",
                self.format.as_str()
            )));
        }

        // A definition cannot advertise native support without configuring how to execute it:
        // the standalone declaration requires its operation, the Messages declarations their
        // overlay headers. The operation data is validated whenever present, regardless of the
        // declared default — an administrator model override can select a mechanism whose
        // definition default is unknown.
        for (mechanism, declaration) in mechanisms.enumerated() {
            let Some(declaration) = declaration else {
                continue;
            };

            let what = format!("the `{}` compaction declaration", mechanism.as_str());

            match declaration {
                Declaration::Standalone(standalone) => {
                    if standalone.default == MechanismSupport::Supported
                        && standalone.operation.is_none()
                    {
                        return Err(ValidationError::new(format!(
                            "{what} declares `supported` without an operation; a definition \
                             cannot advertise native support without configuring how to execute it"
                        )));
                    }

                    if let Some(operation) = &standalone.operation {
                        validate_compaction_operation(operation, slots)?;
                    }

                    validate_unsupported_classifier(standalone.unsupported.as_ref(), &what)?;
                }

                Declaration::RideAlong(ride_along) => {
                    validate_unsupported_classifier(ride_along.unsupported.as_ref(), &what)?;
                }

                Declaration::Overlay(overlay) => {
                    validate_overlay_headers(
                        overlay.overlay_headers.as_deref(),
                        overlay.default,
                        mechanism.as_str(),
                        slots,
                    )?;

                    validate_unsupported_classifier(overlay.unsupported.as_ref(), &what)?;
                }
            }
        }

        if let Some(count_tokens) = &compaction.count_tokens {
            validate_compaction_count(count_tokens, slots)?;
        }

        Ok(())
    }
}

/// The one upstream wire format a compaction mechanism's declaration may ride (README §5): the
/// Responses mechanisms speak `openai_response` and the Messages mechanisms
/// `anthropic_messages`. The exhaustive match keeps the format gating aligned with the
/// mechanism vocabulary.
fn required_format(mechanism: Mechanism) -> UpstreamFormat {
    match mechanism {
        Mechanism::ResponsesStandaloneCompact
        | Mechanism::ResponsesExplicitTrigger
        | Mechanism::ResponsesAutomaticCompaction => UpstreamFormat::OpenAiResponse,

        Mechanism::MessagesOnDemandCompaction | Mechanism::MessagesThresholdCompaction => {
            UpstreamFormat::AnthropicMessages
        }
    }
}

/// Validates one standalone compact operation: the stage-available expressions of its URL,
/// header rows, and envelope, the envelope's exactly-one-`Body` shape, the response-unwrap
/// member, and the operation's own unsupported classifier.
fn validate_compaction_operation(
    operation: &StandaloneCompactOperation,
    slots: &SlotNamespace,
) -> Result<(), ValidationError> {
    let what = "standalone compact operation";

    validate_operation_surface(
        &operation.url,
        &operation.headers,
        operation.envelope.as_ref(),
        operation.response_unwrap.as_deref(),
        what,
        Stage::CompactionOperation,
        slots,
    )?;

    validate_unsupported_classifier(operation.unsupported.as_ref(), what)
}

/// Validates the token-count operation: the stage-available expressions of its URL, header
/// rows, and envelope, the envelope's exactly-one-`Body` shape, and the response-unwrap
/// member.
fn validate_compaction_count(
    operation: &CountTokensOperation,
    slots: &SlotNamespace,
) -> Result<(), ValidationError> {
    let what = "token-count operation";

    validate_operation_surface(
        &operation.url,
        &operation.headers,
        operation.envelope.as_ref(),
        operation.response_unwrap.as_deref(),
        what,
        Stage::CompactionCount,
        slots,
    )
}

/// Validates one operation-shaped connection surface: the non-empty URL, the header rows'
/// names and stage-available values, the envelope's exactly-one-`Body` shape, and the
/// response-unwrap member — the same rules the inference descriptor's surface obeys.
fn validate_operation_surface(
    url: &StringExpr,
    headers: &[ParamRow],
    envelope: Option<&TemplateNode>,
    response_unwrap: Option<&str>,
    what: &str,
    stage: Stage,
    slots: &SlotNamespace,
) -> Result<(), ValidationError> {
    require_non_empty_expr(url, &format!("{what} URL"))?;

    let mut walk = ExpressionWalk::new(stage, slots);

    walk.string_expr(url, &format!("{what} URL"))?;
    walk.headers(headers, &format!("{what} header"))?;

    if let Some(envelope) = envelope {
        let body_count = walk.template_node(envelope, &format!("{what} envelope"))?;

        if body_count != 1 {
            return Err(ValidationError::new(format!(
                "{what} envelope carries {body_count} body placeholders; exactly one is required"
            )));
        }
    }

    if let Some(response_unwrap) = response_unwrap
        && response_unwrap.is_empty()
    {
        return Err(ValidationError::new(format!(
            "{what} response unwrap member must not be empty"
        )));
    }

    Ok(())
}

/// Validates one Messages declaration's overlay headers: present and non-empty when the
/// declaration declares support, valid header names, stage-available values.
fn validate_overlay_headers(
    headers: Option<&[ParamRow]>,
    default: MechanismSupport,
    mechanism: &str,
    slots: &SlotNamespace,
) -> Result<(), ValidationError> {
    let requires_headers = default == MechanismSupport::Supported;

    let Some(headers) = headers else {
        if requires_headers {
            return Err(ValidationError::new(format!(
                "the `{mechanism}` compaction declaration declares `supported` without overlay \
                 headers; a definition cannot advertise native support without configuring how \
                 to execute it"
            )));
        }

        return Ok(());
    };

    if headers.is_empty() && requires_headers {
        return Err(ValidationError::new(format!(
            "the `{mechanism}` compaction declaration declares `supported` without overlay \
             headers; a definition cannot advertise native support without configuring how to \
             execute it"
        )));
    }

    let mut walk = ExpressionWalk::new(Stage::CompactionOperation, slots);

    walk.headers(headers, "compaction overlay header")
}

/// Validates one unsupported-response classifier: the code and parameter rows non-empty. The
/// statuses are [`super::quota_exhaustion::RejectedStatus`] values at parse, so a success
/// status — or a value outside the HTTP status range — is unrepresentable and needs no check
/// here.
fn validate_unsupported_classifier(
    classifier: Option<&super::compaction::UnsupportedClassifier>,
    what: &str,
) -> Result<(), ValidationError> {
    let Some(classifier) = classifier else {
        return Ok(());
    };

    for code in &classifier.codes {
        if code.is_empty() {
            return Err(ValidationError::new(format!(
                "{what} carries an unsupported classifier with an empty code"
            )));
        }
    }

    for parameter in &classifier.parameters {
        if parameter.is_empty() {
            return Err(ValidationError::new(format!(
                "{what} carries an unsupported classifier with an empty parameter"
            )));
        }
    }

    Ok(())
}

/// Whether any extraction of the provisioning sequence stores into a target of the given kind.
fn extractions_target(steps: &[ProvisioningStep], target: &Target) -> bool {
    provision_extractions(steps).any(|extraction| extraction.target == *target)
}

/// Every extraction of the provisioning sequence: the steps' own extractions and the polls'
/// result extractions, in order.
fn provision_extractions(steps: &[ProvisioningStep]) -> impl Iterator<Item = &ProvisionExtraction> {
    steps.iter().flat_map(|step| {
        step.extract
            .iter()
            .chain(step.poll.iter().flat_map(|poll| poll.result_extract.iter()))
    })
}

/// Rejects an expression that must carry at least one part.
fn require_non_empty_expr(expr: &StringExpr, what: &str) -> Result<(), ValidationError> {
    if expr.0.is_empty() {
        Err(ValidationError::new(format!(
            "{what} must carry at least one expression part"
        )))
    } else {
        Ok(())
    }
}

/// Validates one header-row value part's operator-authored text: the literal segments —
/// directly or inside a coalesce's arguments — must be bytes an HTTP header value can carry.
/// Every other part is a dynamic value source whose runtime fold carries its own contract.
fn check_header_value_part(part: &ValueExpr, what: &str) -> Result<(), ValidationError> {
    match part {
        ValueExpr::Literal { value } => {
            if http::HeaderValue::from_str(value).is_err() {
                return Err(ValidationError::new(format!(
                    "{what} carries an invalid HTTP header value: the literal text `{value}`"
                )));
            }

            Ok(())
        }

        ValueExpr::Coalesce { parts } => parts
            .iter()
            .try_for_each(|part| check_header_value_part(part, what)),

        _ => Ok(()),
    }
}

/// Validates one response path: every member non-empty.
fn validate_path(path: &ResponsePath, what: &str) -> Result<(), ValidationError> {
    for member in &path.0 {
        if member.is_empty() {
            return Err(ValidationError::new(format!(
                "{what} path carries an empty member"
            )));
        }
    }

    Ok(())
}

/// Validates one capability extraction's paths (README §5.5): every declared path
/// well-formed, and every member-path — the level-naming, flag, and compaction-object paths —
/// non-empty, so a conforming descriptor never names the addressed value's root by accident.
fn validate_capability_extraction(
    capabilities: &CapabilityExtraction,
) -> Result<(), ValidationError> {
    match &capabilities.thinking_levels {
        Some(ThinkingLevelsExtraction::MemberArray {
            array_at,
            level_field,
        }) => {
            validate_path(array_at, "discovery thinking-level array")?;

            if level_field.0.is_empty() {
                return Err(ValidationError::new(
                    "discovery thinking-level member path must not be empty",
                ));
            }

            validate_path(level_field, "discovery thinking-level member path")?;
        }

        Some(ThinkingLevelsExtraction::FlaggedKeys {
            object_at,
            flag_field,
        }) => {
            validate_path(object_at, "discovery thinking-level object")?;

            if flag_field.0.is_empty() {
                return Err(ValidationError::new(
                    "discovery thinking-level flag path must not be empty",
                ));
            }

            validate_path(flag_field, "discovery thinking-level flag path")?;
        }

        None => {}
    }

    // The token-cap paths name a member inside each entry: an empty chain addresses the entry
    // itself, which is never an integer, so the extraction would fail the refresh on every
    // entry — the chains must name at least one member.
    if let Some(path) = &capabilities.max_input_tokens {
        if path.0.is_empty() {
            return Err(ValidationError::new(
                "discovery max-input-tokens member path must not be empty",
            ));
        }

        validate_path(path, "discovery max-input-tokens")?;
    }

    if let Some(path) = &capabilities.max_output_tokens {
        if path.0.is_empty() {
            return Err(ValidationError::new(
                "discovery max-output-tokens member path must not be empty",
            ));
        }

        validate_path(path, "discovery max-output-tokens")?;
    }

    if let Some(compaction) = &capabilities.compaction {
        for (mechanism, extraction) in [
            ("messages_on_demand", &compaction.messages_on_demand),
            ("messages_threshold", &compaction.messages_threshold),
        ] {
            let Some(extraction) = extraction else {
                continue;
            };

            if extraction.object_at.0.is_empty() {
                return Err(ValidationError::new(format!(
                    "discovery `{mechanism}` compaction object path must not be empty"
                )));
            }

            validate_path(
                &extraction.object_at,
                &format!("discovery `{mechanism}` compaction object"),
            )?;

            if extraction.flag_field.0.is_empty() {
                return Err(ValidationError::new(format!(
                    "discovery `{mechanism}` compaction flag path must not be empty"
                )));
            }

            validate_path(
                &extraction.flag_field,
                &format!("discovery `{mechanism}` compaction flag"),
            )?;
        }
    }

    Ok(())
}

/// Validates one terminal-code list: every code non-empty (README §5.1). The list's own
/// non-emptiness is the carrying `Vec1` type's, rejected at parse.
fn validate_terminal_codes(codes: &Vec1<String>, what: &str) -> Result<(), ValidationError> {
    for code in codes {
        if code.is_empty() {
            return Err(ValidationError::new(format!(
                "{what} terminal codes carry an empty code"
            )));
        }
    }

    Ok(())
}

/// Validates one control request against its stage: stage-available expressions, non-empty
/// parameter names, a body only on `POST`, and — in a JSON body — no body placeholder.
fn validate_control_request(
    request: &ControlRequest,
    stage: Stage,
    slots: &SlotNamespace,
) -> Result<(), ValidationError> {
    let what = "control request";

    require_non_empty_expr(&request.url, what)?;

    let mut walk = ExpressionWalk::new(stage, slots);

    walk.string_expr(&request.url, what)?;
    walk.headers(&request.headers, "control request header")?;
    walk.params(&request.query, "control request query parameter")?;

    if request.body.is_some() && request.method == ControlMethod::Get {
        return Err(ValidationError::new(
            "a GET control request never carries a body",
        ));
    }

    match &request.body {
        None => {}

        Some(ControlBody::Form { fields }) => {
            walk.params(fields, "control request form field")?;
        }

        Some(ControlBody::Json { node }) => {
            let body_count = walk.template_node(node, "control request JSON body")?;

            if body_count != 0 {
                return Err(ValidationError::new(
                    "a control request body never carries the inference body placeholder",
                ));
            }
        }
    }

    Ok(())
}

/// Whether one response target is one the exchange and refresh folds consume (README §5.3):
/// the tokens, the lifetime, the identity token, the base URL, and the named slots.
/// [`crate::provider::oauth::exchange::apply_exchange_target`] holds the runtime half of this
/// table — a target outside the set is stored drift its catch-all arm fails fast on — so the
/// two must move together.
fn is_exchange_stage_target(target: &Target) -> bool {
    matches!(
        target,
        Target::AccessToken
            | Target::RefreshToken
            | Target::ExpiresInSeconds
            | Target::IdToken
            | Target::BaseUrl
            | Target::Slot { .. }
    )
}

/// Whether one response target is one the provisioning fold consumes (README §5.4): the
/// tokens, the base URL, the absolute expiry, and the named slots.
/// [`crate::provider::oauth::provisioning::apply_target`] holds the runtime half of this
/// table — a target outside the set is stored drift its catch-all arm fails fast on — so the
/// two must move together.
fn is_provision_stage_target(target: &Target) -> bool {
    matches!(
        target,
        Target::AccessToken
            | Target::RefreshToken
            | Target::BaseUrl
            | Target::ExpiresAtUnixSeconds
            | Target::Slot { .. }
    )
}

/// Validates one response descriptor: non-empty paths, no target stored into twice, and — for
/// a form-encoded response, whose fields are flat — exactly one path member. The
/// duplicate-target rule covers every stage-level descriptor (README §5.1): the fold applies
/// the fields in order and a later present value silently overwrites the earlier one, so the
/// earlier mapping is dead configuration. Cross-stage repeats stay allowed — several stages
/// may write one slot, each from its own response.
fn validate_response_descriptor(
    descriptor: &ResponseDescriptor,
    what: &str,
) -> Result<(), ValidationError> {
    for field in &descriptor.fields {
        validate_path(&field.path, what)?;

        if descriptor.encoding == ResponseEncoding::Form && field.path.0.len() != 1 {
            return Err(ValidationError::new(format!(
                "{what} paths address flat form fields and carry exactly one member"
            )));
        }
    }

    reject_duplicate_targets(descriptor.fields.iter().map(|field| &field.target), what)
}

/// Rejects one extraction list that stores into one target twice: the fold applies the
/// mappings in order and a later present value silently overwrites the earlier one, so the
/// earlier mapping is dead configuration the definition can never observe.
fn reject_duplicate_targets<'a>(
    targets: impl IntoIterator<Item = &'a Target>,
    what: &str,
) -> Result<(), ValidationError> {
    let mut seen: Vec<&Target> = Vec::new();

    for target in targets {
        if seen.contains(&target) {
            return Err(ValidationError::new(format!(
                "{what} carries the `{}` target twice: the mappings apply in order and the \
                 earlier one is dead configuration",
                target_name(target)
            )));
        }

        seen.push(target);
    }

    Ok(())
}

/// The human-readable name of one response target, rendered by rejections: the wire tag, with
/// a slot target carrying its name.
fn target_name(target: &Target) -> String {
    match target {
        Target::AccessToken => "access_token".to_string(),
        Target::RefreshToken => "refresh_token".to_string(),
        Target::ExpiresInSeconds => "expires_in_seconds".to_string(),
        Target::IdToken => "id_token".to_string(),
        Target::BaseUrl => "base_url".to_string(),
        Target::Slot { name } => format!("slot:{name}"),
        Target::DeviceCode => "device_code".to_string(),
        Target::UserCode => "user_code".to_string(),
        Target::VerificationUri => "verification_uri".to_string(),
        Target::VerificationUriComplete => "verification_uri_complete".to_string(),
        Target::IntervalSeconds => "interval_seconds".to_string(),
        Target::ExpiresAtUnixSeconds => "expires_at_unix_seconds".to_string(),
    }
}

/// Validates one provisioning extraction: its target, path, shape, and the form-encoding rule.
fn validate_provision_extraction(
    extraction: &ProvisionExtraction,
    encoding: &ResponseEncoding,
) -> Result<(), ValidationError> {
    // The runtime fold consumes exactly the provisioning stage's target set (README §5.4): an
    // exchange- or device-stage target in an extraction is stored drift the fold fails the
    // step on. The check covers the steps' own extractions and the polls' result extractions
    // alike, every caller routing through here.
    if !is_provision_stage_target(&extraction.target) {
        return Err(ValidationError::new(format!(
            "provisioning extraction carries the `{}` target its stage does not produce",
            target_name(&extraction.target)
        )));
    }

    validate_path(&extraction.path, "provisioning extraction")?;

    if *encoding == ResponseEncoding::Form && extraction.path.0.len() != 1 {
        return Err(ValidationError::new(
            "provisioning extraction paths address flat form fields and carry exactly one member",
        ));
    }

    match &extraction.shape {
        ExtractionShape::String | ExtractionShape::Integer => {}

        ExtractionShape::StringOrField { field } => {
            if field.is_empty() {
                return Err(ValidationError::new(
                    "provisioning extraction string-or-field shape names an empty member",
                ));
            }
        }
    }

    Ok(())
}

/// The expression walk: validates every [`ValueExpr`] of one descriptor section against its
/// stage's available kinds and the definition's slot namespace, and counts the body placeholders
/// of template trees.
struct ExpressionWalk<'a> {
    /// The stage whose expressions are walked.
    stage: Stage,

    /// The definition's slot namespace.
    slots: &'a SlotNamespace,
}

impl<'a> ExpressionWalk<'a> {
    /// Opens a walk over one stage.
    fn new(stage: Stage, slots: &'a SlotNamespace) -> Self {
        Self { stage, slots }
    }

    /// Validates one string expression using the shared structural owner.
    fn string_expr(&mut self, expr: &StringExpr, what: &str) -> Result<(), ValidationError> {
        self.surface(ExpressionSurface::String(expr), what)
            .map(|_| ())
    }

    /// Validates percent-encoded parameter rows using the shared structural owner.
    fn params(&mut self, rows: &[ParamRow], what: &str) -> Result<(), ValidationError> {
        self.surface(ExpressionSurface::Parameters(rows), what)
            .map(|_| ())
    }

    /// Validates literal HTTP header rows using the shared structural owner, holding each
    /// row's operator-authored value text to the HTTP header-value byte contract (README
    /// §5.1): the row reaches the wire as a literal header value, and a control byte — a
    /// trailing newline in a literal part foremost — would fail every dispatch at runtime as
    /// a transport failure misclassified as upstream unavailability, the same
    /// misclassification the header-name rule exists for. Dynamic value sources keep their
    /// fold-time contracts; the configured client credentials carry the byte contract at the
    /// recipe.
    fn headers(&mut self, rows: &[ParamRow], what: &str) -> Result<(), ValidationError> {
        for row in rows {
            for part in &row.value.0 {
                check_header_value_part(part, what)?;
            }
        }

        self.surface(ExpressionSurface::Headers(rows), what)
            .map(|_| ())
    }

    /// Validates template members and counts concrete codec body placeholders.
    fn template_node(&mut self, node: &TemplateNode, what: &str) -> Result<usize, ValidationError> {
        self.surface(ExpressionSurface::Template(node), what)
    }

    /// Applies write-boundary rules to structural facts in their original diagnostic order.
    fn surface(
        &mut self,
        surface: ExpressionSurface<'_>,
        what: &str,
    ) -> Result<usize, ValidationError> {
        let mut body_count = 0;
        for member in surface.members() {
            match member {
                ExpressionMember::Value(value) => self.value_expr(value, what)?,
                ExpressionMember::Body => body_count += 1,
                ExpressionMember::HeaderName(name) | ExpressionMember::ParameterName(name) => {
                    if name.is_empty() {
                        return Err(ValidationError::new(format!(
                            "{what} carries an empty name"
                        )));
                    }
                    if matches!(member, ExpressionMember::HeaderName(_))
                        && http::HeaderName::try_from(name).is_err()
                    {
                        return Err(ValidationError::new(format!(
                            "{what} carries an invalid HTTP header name `{name}`"
                        )));
                    }
                }
                ExpressionMember::ObjectMemberName("") => {
                    return Err(ValidationError::new(format!(
                        "{what} carries an empty object member name"
                    )));
                }
                ExpressionMember::ObjectMemberName(_) => {}
                ExpressionMember::EmptyCoalesce => {
                    return Err(ValidationError::new(format!(
                        "{what} carries an empty coalesce"
                    )));
                }
                ExpressionMember::NestedCoalesce => {
                    return Err(ValidationError::new(format!(
                        "{what} nests a coalesce inside a coalesce; coalesce arguments \
                         carry direct value parts only"
                    )));
                }
            }
        }
        Ok(body_count)
    }

    /// Validates one direct value part against the stage's available kinds.
    fn value_expr(&mut self, expr: &ValueExpr, what: &str) -> Result<(), ValidationError> {
        match expr {
            ValueExpr::Literal { .. } | ValueExpr::Coalesce { .. } => Ok(()),

            ValueExpr::Slot { name } => {
                if !self.stage.allows(expr) {
                    return Err(ValidationError::new(format!(
                        "{what} references a slot, which the {} stage does not carry",
                        stage_name(self.stage)
                    )));
                }

                self.slots.require_defined(name, what)
            }

            other => {
                if self.stage.allows(other) {
                    Ok(())
                } else {
                    Err(ValidationError::new(format!(
                        "{what} references `{}`, which the {} stage does not carry",
                        other.display_name(),
                        stage_name(self.stage)
                    )))
                }
            }
        }
    }
}

/// The human-readable name of a stage, rendered by rejections.
fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::AuthorizeUrl => "authorize URL",
        Stage::Inference => "inference",
        Stage::DeviceAuthorization => "device authorization",
        Stage::ExchangeAuthorizationCode | Stage::ExchangeDevice => "token exchange",
        Stage::RefreshGrant => "token refresh",
        Stage::Provisioning => "provisioning",
        Stage::Discovery => "discovery",
        Stage::Quota => "quota",
        Stage::CompactionOperation => "compaction operation",
        Stage::CompactionCount => "compaction token count",
    }
}
