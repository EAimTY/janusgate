//! Write-time validation of a provider definition payload (README §5).
//!
//! The SQL schema enforces only the repository-wide constraint boundary; the management boundary
//! enforces everything else once, at write time: descriptor structure, stage-available value
//! kinds, slot namespace agreement, envelope and control-request body shapes, OAuth section
//! agreement, and the per-format render-option sets. Runtime never re-validates — a violation
//! that surfaces anyway is stored drift and fails fast.

use super::{
    credential::{CredentialAcquisition, Grant, OAuthRecipe, Pkce, TokenRefresh},
    discovery::{DiscoveryExtract, Pagination},
    provisioning::{ProvisionExtraction, ProvisioningStep},
    template::{
        ControlBody, ControlMethod, ControlRequest, ExtractionShape, ParamRow, ResponseDescriptor,
        ResponseEncoding, ResponsePath, ResponseTarget as Target, StringExpr, TemplateNode,
        ValueExpr, ValueKind,
    },
};
use crate::format::{RenderOption, UpstreamFormat};
use std::collections::BTreeSet;
/// A definition payload rejection: one message naming the offending descriptor element, rendered
/// verbatim to the management client as an invalid-request rejection.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ValidationError {
    /// The human-readable rejection message.
    pub message: String,
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
}

impl Stage {
    /// Whether the stage's expressions may carry the value kind; the constant configuration
    /// kinds — literals and the configured client credentials — are available everywhere, and the
    /// flow-state kinds are available exactly at the stages that produce or consume them.
    const fn allows(self, kind: ValueKind) -> bool {
        match kind {
            ValueKind::Literal | ValueKind::ClientId | ValueKind::ClientSecret => true,

            ValueKind::Base => !matches!(self, Self::AuthorizeUrl),

            ValueKind::Secret => {
                matches!(self, Self::Inference | Self::Provisioning | Self::Discovery)
            }

            ValueKind::Slot => !matches!(self, Self::AuthorizeUrl),

            ValueKind::Model | ValueKind::RequestUuid => matches!(self, Self::Inference),

            ValueKind::AuthorizationCode | ValueKind::CodeVerifier | ValueKind::RedirectUri => {
                matches!(self, Self::ExchangeAuthorizationCode)
            }

            ValueKind::OAuthState => matches!(self, Self::AuthorizeUrl),

            ValueKind::DeviceCode => matches!(self, Self::ExchangeDevice),

            ValueKind::RefreshToken => matches!(self, Self::RefreshGrant),
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
pub fn allowed_render_options(format: UpstreamFormat) -> &'static [RenderOption] {
    match format {
        UpstreamFormat::OpenAiResponse => &[RenderOption::OmitMaxOutputTokens],
        UpstreamFormat::OpenAiChatCompletion
        | UpstreamFormat::GeminiGenerateContent
        | UpstreamFormat::GeminiInteraction
        | UpstreamFormat::AnthropicMessages => &[],
    }
}

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

            for claim in id_token_claims {
                validate_path(&claim.claim, "identity-token claim")?;
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

        Ok(())
    }

    /// Validates the inference descriptor: stage-available expressions, the render-option set,
    /// and the envelope's exactly-one-`Body` shape.
    fn validate_inference(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        let inference = &self.inference;

        require_non_empty_expr(&inference.url, "inference URL")?;

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

                let mut targets = BTreeSet::new();

                for field in &authorization.response.fields {
                    let target = match &field.target {
                        Target::DeviceCode => "device_code",
                        Target::UserCode => "user_code",
                        Target::VerificationUri => "verification_uri",
                        Target::VerificationUriComplete => "verification_uri_complete",
                        Target::ExpiresInSeconds | Target::IntervalSeconds => {
                            continue;
                        }
                        _ => {
                            return Err(ValidationError::new(
                                "device authorization response carries a target its stage \
                                 does not produce",
                            ));
                        }
                    };

                    if !targets.insert(target) {
                        return Err(ValidationError::new(format!(
                            "device authorization response carries the `{target}` target twice"
                        )));
                    }

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
                    && !request_references(&authorization.request, &ValueExpr::ClientId)
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
            && !request_references(&exchange.request, &ValueExpr::CodeVerifier)
        {
            return Err(ValidationError::new(
                "authorization-code grant with `pkce: s256` requires the token exchange request \
                 to reference the `code_verifier` value part: RFC 7636 §4.6 presents the verifier \
                 at the token endpoint whenever a challenge was sent",
            ));
        }

        // RFC 6749 §4.1.3: the token request repeats the `redirect_uri`, identically, whenever
        // the authorization request included it — and the builder appends the definition's
        // `redirect_uri` to every authorize URL — so an exchange request that never references
        // the part dies at the upstream with `invalid_request` on every flow: the same
        // required-carry shape the verifier rule above enforces.
        if let Grant::AuthorizationCode { .. } = grant
            && !request_references(&exchange.request, &ValueExpr::RedirectUri)
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
            && !request_references(&exchange.request, &ValueExpr::AuthorizationCode)
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
            && !request_references(&exchange.request, &ValueExpr::DeviceCode)
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
        if client_secret.is_none() && !request_references(&exchange.request, &ValueExpr::ClientId) {
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
            && !request_references(request, &ValueExpr::RefreshToken)
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
            && !request_references(request, &ValueExpr::ClientId)
        {
            return Err(ValidationError::new(
                "OAuth credential without a client secret requires the token refresh request \
                 to reference the `client_id` value part: RFC 6749 §6 re-authenticates the \
                 client as the token endpoint does, making the client ID REQUIRED when no \
                 secret authenticates (§4.1.3, §3.2.1)",
            ));
        }

        validate_response_descriptor(&exchange.response, "token exchange")?;
        validate_terminal_codes(&exchange.terminal_codes, "token exchange")?;

        if let Some(steps) = &self.provisioning
            && steps.is_empty()
        {
            return Err(ValidationError::new(
                "provisioning must carry at least one step when present",
            ));
        }

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

        // The refresh token must be produced before the refresh grant presents it.
        let produces_refresh_token = exchange
            .response
            .fields
            .iter()
            .any(|field| matches!(field.target, Target::RefreshToken))
            || self
                .provisioning
                .as_ref()
                .is_some_and(|steps| extractions_target(steps, &Target::RefreshToken));

        match refresh {
            TokenRefresh::Grant {
                request,
                response,
                terminal_codes,
            } => {
                if !produces_refresh_token {
                    return Err(ValidationError::new(
                        "refresh grant requires the credential to carry a refresh token: map \
                         the `refresh_token` target in the exchange response or extract it in a \
                         provisioning step",
                    ));
                }

                validate_control_request(request, Stage::RefreshGrant, slots)?;

                if let Some(response) = response {
                    validate_response_descriptor(response, "token refresh")?;

                    // RFC 6749 §6: the refresh response carries the access token — REQUIRED —
                    // and a declared mapping wholly replaces the exchange's default as the
                    // renewal's fold input, so a declared mapping with no `access_token`
                    // target folds every renewal into a tokenless outcome: the lease is
                    // acquired and cleared first, and the renewal then dies on the missing
                    // token — a permanent failure on every dispatch of the provider past its
                    // first expiry. An absent mapping defaults to the exchange's response, so the
                    // defaulted case leans on the access-token rule above — a rule that also
                    // admits a provisioning-supplied token, which leaves a provisioning-only-
                    // token definition under grant mode as the known residual edge. That edge
                    // wedges loudly at the runtime guard, and closing it — requiring the
                    // effective (declared-or-defaulted) mapping to carry the token row — is
                    // deliberately left for a focused follow-up.
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

        Ok(())
    }

    /// Validates the provisioning descriptor: stage-available expressions, control-request
    /// shapes, extraction shapes, and the poll configurations. Provisioning runs after a token
    /// exchange, so an API-key recipe never carries one.
    fn validate_provisioning(&self, slots: &SlotNamespace) -> Result<(), ValidationError> {
        if self.provisioning.is_some() && !self.credential.is_oauth() {
            return Err(ValidationError::new(
                "provisioning requires an OAuth credential: it runs after the token exchange",
            ));
        }

        for step in self.provisioning.as_deref().unwrap_or_default() {
            validate_control_request(&step.request, Stage::Provisioning, slots)?;

            if let Some(skip_if) = &step.skip_if {
                slots.require_defined(skip_if, "provisioning step guard")?;
            }

            for extraction in &step.extract {
                validate_provision_extraction(extraction, &step.response_encoding)?;
            }

            if let Some(poll) = &step.poll {
                validate_control_request(&poll.request, Stage::Provisioning, slots)?;

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

                validate_path(response_path, "discovery pagination response path")?;
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

                validate_path(has_more_path, "discovery has-more path")?;
                validate_path(last_id_path, "discovery last-identifier path")?;
            }
        }

        Ok(())
    }
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

/// Validates one terminal-code list: the list and every code non-empty (README §5.1).
fn validate_terminal_codes(codes: &[String], what: &str) -> Result<(), ValidationError> {
    if codes.is_empty() {
        return Err(ValidationError::new(format!(
            "{what} terminal codes must carry at least one code"
        )));
    }

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

    match &request.body {
        None => {}

        Some(ControlBody::Form { fields }) => {
            if request.method == ControlMethod::Get {
                return Err(ValidationError::new(
                    "a GET control request never carries a body",
                ));
            }

            walk.params(fields, "control request form field")?;
        }

        Some(ControlBody::Json { node }) => {
            if request.method == ControlMethod::Get {
                return Err(ValidationError::new(
                    "a GET control request never carries a body",
                ));
            }

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

/// Whether one string expression references the sought value part, directly or inside a
/// coalesce's arguments. Shared with the parent module's base-reference predicate, which seeks
/// the same walk over the inference descriptor's non-URL members.
pub(super) fn references(expr: &StringExpr, sought: &ValueExpr) -> bool {
    expr.0.iter().any(|part| match part {
        ValueExpr::Coalesce { parts } => parts.contains(sought),

        other => other == sought,
    })
}

/// Whether one template tree references the sought value part in any of its string leaves.
/// Shared with the parent module's base-reference predicate, which seeks the same walk over
/// the inference envelope.
pub(super) fn template_node_references(node: &TemplateNode, sought: &ValueExpr) -> bool {
    match node {
        TemplateNode::Json { .. } | TemplateNode::Body => false,

        TemplateNode::String { expr } => references(expr, sought),

        TemplateNode::Object { fields } => fields
            .values()
            .any(|field| template_node_references(field, sought)),

        TemplateNode::Array { items } => items
            .iter()
            .any(|item| template_node_references(item, sought)),
    }
}

/// Whether one control request references the sought value part anywhere its stage expressions
/// live: the URL, the headers, the query rows, and the body's form fields or JSON string leaves.
/// Shared with the parent module's base-reference predicate, which seeks the same walk over
/// every control request of the definition.
pub(super) fn request_references(request: &ControlRequest, sought: &ValueExpr) -> bool {
    let rows_reference = |rows: &[ParamRow]| rows.iter().any(|row| references(&row.value, sought));

    let body_references = match &request.body {
        None => false,

        Some(ControlBody::Form { fields }) => rows_reference(fields),

        Some(ControlBody::Json { node }) => template_node_references(node, sought),
    };

    body_references
        || rows_reference(&request.headers)
        || rows_reference(&request.query)
        || references(&request.url, sought)
}

/// Validates one response descriptor: non-empty paths, and — for a form-encoded response, whose
/// fields are flat — exactly one path member.
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

    Ok(())
}

/// Validates one provisioning extraction: its path, shape, and the form-encoding rule.
fn validate_provision_extraction(
    extraction: &ProvisionExtraction,
    encoding: &ResponseEncoding,
) -> Result<(), ValidationError> {
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

    /// Walks one string expression.
    fn string_expr(&mut self, expr: &StringExpr, what: &str) -> Result<(), ValidationError> {
        for part in &expr.0 {
            self.value_expr(part, what)?;
        }

        Ok(())
    }

    /// Walks one percent-encoded row list — query parameters, form fields, and authorize-URL
    /// parameters: non-empty names, stage-available values. Percent-encoding carries any name,
    /// so no header-name rule applies.
    fn params(&mut self, rows: &[ParamRow], what: &str) -> Result<(), ValidationError> {
        for row in rows {
            if row.name.is_empty() {
                return Err(ValidationError::new(format!(
                    "{what} carries an empty name"
                )));
            }

            self.string_expr(&row.value, what)?;
        }

        Ok(())
    }

    /// Walks one header-row list: non-empty, syntactically valid HTTP header names,
    /// stage-available values. A header row reaches the wire as a literal header name (README
    /// §5.2), so an invalid name is a definition defect caught here at the write instead of
    /// failing every dispatch at runtime under a misleading error classification.
    fn headers(&mut self, rows: &[ParamRow], what: &str) -> Result<(), ValidationError> {
        for row in rows {
            if row.name.is_empty() {
                return Err(ValidationError::new(format!(
                    "{what} carries an empty name"
                )));
            }

            if http::HeaderName::try_from(row.name.as_str()).is_err() {
                return Err(ValidationError::new(format!(
                    "{what} carries an invalid HTTP header name `{}`",
                    row.name
                )));
            }

            self.string_expr(&row.value, what)?;
        }

        Ok(())
    }

    /// Walks one template tree, returning the number of body placeholders it carries.
    fn template_node(&mut self, node: &TemplateNode, what: &str) -> Result<usize, ValidationError> {
        match node {
            TemplateNode::Json { .. } => Ok(0),

            TemplateNode::String { expr } => {
                self.string_expr(expr, what)?;

                Ok(0)
            }

            TemplateNode::Body => Ok(1),

            TemplateNode::Object { fields } => {
                let mut body_count = 0;

                for (name, field) in fields {
                    if name.is_empty() {
                        return Err(ValidationError::new(format!(
                            "{what} carries an empty object member name"
                        )));
                    }

                    body_count += self.template_node(field, what)?;
                }

                Ok(body_count)
            }

            TemplateNode::Array { items } => {
                let mut body_count = 0;

                for item in items {
                    body_count += self.template_node(item, what)?;
                }

                Ok(body_count)
            }
        }
    }

    /// Validates one value part against the stage's available kinds.
    fn value_expr(&mut self, expr: &ValueExpr, what: &str) -> Result<(), ValidationError> {
        match expr {
            ValueExpr::Literal { .. } => Ok(()),

            ValueExpr::Coalesce { parts } => {
                if parts.is_empty() {
                    return Err(ValidationError::new(format!(
                        "{what} carries an empty coalesce"
                    )));
                }

                for part in parts {
                    if matches!(part, ValueExpr::Coalesce { .. }) {
                        return Err(ValidationError::new(format!(
                            "{what} nests a coalesce inside a coalesce; coalesce arguments \
                             carry direct value parts only"
                        )));
                    }

                    self.value_expr(part, what)?;
                }

                Ok(())
            }

            ValueExpr::Slot { name } => {
                if !self.stage.allows(ValueKind::Slot) {
                    return Err(ValidationError::new(format!(
                        "{what} references a slot, which the {} stage does not carry",
                        stage_name(self.stage)
                    )));
                }

                self.slots.require_defined(name, what)
            }

            other => {
                let kind = other.kind();

                if self.stage.allows(kind) {
                    Ok(())
                } else {
                    Err(ValidationError::new(format!(
                        "{what} references `{}`, which the {} stage does not carry",
                        kind.display_name(),
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
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "the mutations address fixed fixture positions, whose absence is the rejection"
    )]
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap or unwrap_err lands on its side"
    )]

    use super::*;
    use serde_json::json;

    /// The parts of one literal string expression.
    fn literal(text: &str) -> serde_json::Value {
        serde_json::json!([{ "type": "literal", "value": text }])
    }

    /// A good full definition payload in the Antigravity shape: an authorization-code grant with
    /// a client secret, a form exchange, identity-free slots, a provisioning sequence with a
    /// conditional long-running poll, an enveloped inference with response unwrapping, and a
    /// map-key discovery. The rejection cases below mutate this document.
    fn good_antigravity_json() -> serde_json::Value {
        serde_json::json!({
          "format": "gemini_generate_content",
          "inference": {
            "url": [
              {
                "type": "base"
              },
              {
                "type": "literal",
                "value": ":generateContent"
              }
            ],
            "stream_url": [
              {
                "type": "base"
              },
              {
                "type": "literal",
                "value": ":streamGenerateContent?alt=sse"
              }
            ],
            "headers": [
              {
                "name": "Authorization",
                "value": [
                  {
                    "type": "literal",
                    "value": "Bearer "
                  },
                  {
                    "type": "secret"
                  }
                ]
              },
              {
                "name": "User-Agent",
                "value": [
                  {
                    "type": "literal",
                    "value": "antigravity"
                  }
                ]
              }
            ],
            "upstream_streaming": "match_client",
            "envelope": {
              "type": "object",
              "fields": {
                "project": {
                  "type": "string",
                  "expr": [
                    {
                      "type": "slot",
                      "name": "project"
                    }
                  ]
                },
                "model": {
                  "type": "string",
                  "expr": [
                    {
                      "type": "model"
                    }
                  ]
                },
                "request": {
                  "type": "body"
                },
                "requestType": {
                  "type": "json",
                  "value": "agent"
                },
                "requestId": {
                  "type": "string",
                  "expr": [
                    {
                      "type": "literal",
                      "value": "agent-"
                    },
                    {
                      "type": "request_uuid"
                    }
                  ]
                }
              }
            },
            "response_unwrap": "response"
          },
          "credential": {
            "type": "oauth",
            "client_id": "client-id",
            "client_secret": "client-secret",
            "initial_slots": [
              {
                "name": "device_id",
                "generator": "uuid_hex"
              }
            ],
            "grant": {
              "type": "authorization_code",
              "authorize_url": [
                {
                  "type": "literal",
                  "value": "https://accounts.example/o/oauth2/v2/auth"
                }
              ],
              "redirect_uri": "http://localhost:51121/oauth-callback",
              "pkce": "none",
              "extra_params": [
                {
                  "name": "access_type",
                  "value": [
                    {
                      "type": "literal",
                      "value": "offline"
                    }
                  ]
                },
                {
                  "name": "prompt",
                  "value": [
                    {
                      "type": "literal",
                      "value": "consent"
                    }
                  ]
                },
                {
                  "name": "scope",
                  "value": [
                    {
                      "type": "literal",
                      "value": "https://www.googleapis.com/auth/cloud-platform"
                    }
                  ]
                }
              ]
            },
            "exchange": {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://oauth2.example/token"
                  }
                ],
                "body": {
                  "kind": "form",
                  "fields": [
                    {
                      "name": "grant_type",
                      "value": [
                        {
                          "type": "literal",
                          "value": "authorization_code"
                        }
                      ]
                    },
                    {
                      "name": "code",
                      "value": [
                        {
                          "type": "authorization_code"
                        }
                      ]
                    },
                    {
                      "name": "client_id",
                      "value": [
                        {
                          "type": "client_id"
                        }
                      ]
                    },
                    {
                      "name": "client_secret",
                      "value": [
                        {
                          "type": "client_secret"
                        }
                      ]
                    },
                    {
                      "name": "redirect_uri",
                      "value": [
                        {
                          "type": "redirect_uri"
                        }
                      ]
                    }
                  ]
                }
              },
              "response": {
                "encoding": "json",
                "fields": [
                  {
                    "path": [
                      "access_token"
                    ],
                    "target": {
                      "type": "access_token"
                    },
                    "required": true
                  },
                  {
                    "path": [
                      "refresh_token"
                    ],
                    "target": {
                      "type": "refresh_token"
                    },
                    "required": true
                  },
                  {
                    "path": [
                      "expires_in"
                    ],
                    "target": {
                      "type": "expires_in_seconds"
                    },
                    "required": false
                  }
                ]
              },
              "terminal_codes": [
                "invalid_grant"
              ]
            },
            "id_token_claims": [],
            "refresh": {
              "type": "grant",
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://oauth2.example/token"
                  }
                ],
                "body": {
                  "kind": "form",
                  "fields": [
                    {
                      "name": "grant_type",
                      "value": [
                        {
                          "type": "literal",
                          "value": "refresh_token"
                        }
                      ]
                    },
                    {
                      "name": "refresh_token",
                      "value": [
                        {
                          "type": "refresh_token"
                        }
                      ]
                    },
                    {
                      "name": "client_id",
                      "value": [
                        {
                          "type": "client_id"
                        }
                      ]
                    },
                    {
                      "name": "client_secret",
                      "value": [
                        {
                          "type": "client_secret"
                        }
                      ]
                    }
                  ]
                }
              },
              "terminal_codes": [
                "invalid_grant"
              ]
            }
          },
          "provisioning": [
            {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "base"
                  },
                  {
                    "type": "literal",
                    "value": ":loadCodeAssist"
                  }
                ],
                "headers": [
                  {
                    "name": "Authorization",
                    "value": [
                      {
                        "type": "literal",
                        "value": "Bearer "
                      },
                      {
                        "type": "secret"
                      }
                    ]
                  },
                  {
                    "name": "User-Agent",
                    "value": [
                      {
                        "type": "literal",
                        "value": "antigravity"
                      }
                    ]
                  }
                ],
                "body": {
                  "kind": "json",
                  "node": {
                    "type": "object",
                    "fields": {
                      "metadata": {
                        "type": "json",
                        "value": {
                          "ideType": "ANTIGRAVITY"
                        }
                      }
                    }
                  }
                }
              },
              "response_encoding": "json",
              "extract": [
                {
                  "path": [
                    "cloudaicompanionProject"
                  ],
                  "target": {
                    "type": "slot",
                    "name": "project"
                  },
                  "shape": {
                    "type": "string_or_field",
                    "field": "id"
                  },
                  "required": false
                },
                {
                  "path": [
                    "paidTier",
                    "id"
                  ],
                  "target": {
                    "type": "slot",
                    "name": "paid_tier"
                  },
                  "shape": {
                    "type": "string"
                  },
                  "required": false
                },
                {
                  "path": [
                    "currentTier",
                    "id"
                  ],
                  "target": {
                    "type": "slot",
                    "name": "current_tier"
                  },
                  "shape": {
                    "type": "string"
                  },
                  "required": false
                }
              ]
            },
            {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "base"
                  },
                  {
                    "type": "literal",
                    "value": ":onboardUser"
                  }
                ],
                "headers": [
                  {
                    "name": "Authorization",
                    "value": [
                      {
                        "type": "literal",
                        "value": "Bearer "
                      },
                      {
                        "type": "secret"
                      }
                    ]
                  }
                ],
                "body": {
                  "kind": "json",
                  "node": {
                    "type": "object",
                    "fields": {
                      "tierId": {
                        "type": "string",
                        "expr": [
                          {
                            "type": "coalesce",
                            "parts": [
                              {
                                "type": "slot",
                                "name": "paid_tier"
                              },
                              {
                                "type": "slot",
                                "name": "current_tier"
                              },
                              {
                                "type": "literal",
                                "value": "free-tier"
                              }
                            ]
                          }
                        ]
                      },
                      "metadata": {
                        "type": "json",
                        "value": {
                          "ideType": "ANTIGRAVITY"
                        }
                      }
                    }
                  }
                }
              },
              "response_encoding": "json",
              "skip_if": "project",
              "poll": {
                "request": {
                  "method": "POST",
                  "url": [
                    {
                      "type": "base"
                    },
                    {
                      "type": "literal",
                      "value": ":onboardUser"
                    }
                  ],
                  "headers": [
                    {
                      "name": "Authorization",
                      "value": [
                        {
                          "type": "literal",
                          "value": "Bearer "
                        },
                        {
                          "type": "secret"
                        }
                      ]
                    }
                  ],
                  "body": {
                    "kind": "json",
                    "node": {
                      "type": "json",
                      "value": {}
                    }
                  }
                },
                "response_encoding": "json",
                "done_path": [
                  "done"
                ],
                "error_path": [
                  "error"
                ],
                "result_extract": [
                  {
                    "path": [
                      "response",
                      "cloudaicompanionProject"
                    ],
                    "target": {
                      "type": "slot",
                      "name": "project"
                    },
                    "shape": {
                      "type": "string_or_field",
                      "field": "id"
                    },
                    "required": true
                  }
                ],
                "interval_ms": 1000,
                "budget_ms": 60000
              }
            }
          ],
          "discovery": {
            "request": {
              "method": "POST",
              "url": [
                {
                  "type": "base"
                },
                {
                  "type": "literal",
                  "value": ":fetchAvailableModels"
                }
              ],
              "headers": [
                {
                  "name": "Authorization",
                  "value": [
                    {
                      "type": "literal",
                      "value": "Bearer "
                    },
                    {
                      "type": "secret"
                    }
                  ]
                }
              ],
              "body": {
                "kind": "json",
                "node": {
                  "type": "object",
                  "fields": {}
                }
              }
            },
            "extract": {
              "mode": "map_keys",
              "at": [
                "models"
              ]
            },
            "pagination": {
              "type": "none"
            }
          }
        })
    }

    /// A good device-grant payload in the Copilot shape: a device grant whose polling exchange
    /// supplies only a durable slot, a reprovisioning refresh whose steps mint the access token,
    /// the expiry, and the base URL, and a cursor-paginated discovery.
    fn good_copilot_json() -> serde_json::Value {
        serde_json::json!({
          "format": "openai_chat_completion",
          "inference": {
            "url": [
              {
                "type": "base"
              },
              {
                "type": "literal",
                "value": "/chat/completions"
              }
            ],
            "headers": [
              {
                "name": "Authorization",
                "value": [
                  {
                    "type": "literal",
                    "value": "Bearer "
                  },
                  {
                    "type": "secret"
                  }
                ]
              },
              {
                "name": "Copilot-Integration-Id",
                "value": [
                  {
                    "type": "literal",
                    "value": "vscode-chat"
                  }
                ]
              }
            ],
            "upstream_streaming": "match_client"
          },
          "credential": {
            "type": "oauth",
            "client_id": "Iv1.b507a08c87ecfe98",
            "initial_slots": [
              {
                "name": "device_id",
                "generator": "uuid"
              }
            ],
            "grant": {
              "type": "device",
              "authorization": {
                "request": {
                  "method": "POST",
                  "url": [
                    {
                      "type": "literal",
                      "value": "https://github.com/login/device/code"
                    }
                  ],
                  "headers": [
                    {
                      "name": "Accept",
                      "value": [
                        {
                          "type": "literal",
                          "value": "application/json"
                        }
                      ]
                    }
                  ],
                  "body": {
                    "kind": "form",
                    "fields": [
                      {
                        "name": "client_id",
                        "value": [
                          {
                            "type": "client_id"
                          }
                        ]
                      },
                      {
                        "name": "device_id",
                        "value": [
                          {
                            "type": "slot",
                            "name": "device_id"
                          }
                        ]
                      }
                    ]
                  }
                },
                "response": {
                  "encoding": "form",
                  "fields": [
                    {
                      "path": [
                        "device_code"
                      ],
                      "target": {
                        "type": "device_code"
                      },
                      "required": true
                    },
                    {
                      "path": [
                        "user_code"
                      ],
                      "target": {
                        "type": "user_code"
                      },
                      "required": true
                    },
                    {
                      "path": [
                        "verification_uri"
                      ],
                      "target": {
                        "type": "verification_uri"
                      },
                      "required": false
                    },
                    {
                      "path": [
                        "verification_uri_complete"
                      ],
                      "target": {
                        "type": "verification_uri_complete"
                      },
                      "required": false
                    },
                    {
                      "path": [
                        "expires_in"
                      ],
                      "target": {
                        "type": "expires_in_seconds"
                      },
                      "required": false
                    },
                    {
                      "path": [
                        "interval"
                      ],
                      "target": {
                        "type": "interval_seconds"
                      },
                      "required": false
                    }
                  ]
                }
              },
              "expires_in_fallback_seconds": 900
            },
            "exchange": {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://github.com/login/oauth/access_token"
                  }
                ],
                "headers": [
                  {
                    "name": "Accept",
                    "value": [
                      {
                        "type": "literal",
                        "value": "application/json"
                      }
                    ]
                  }
                ],
                "body": {
                  "kind": "form",
                  "fields": [
                    {
                      "name": "grant_type",
                      "value": [
                        {
                          "type": "literal",
                          "value": "urn:ietf:params:oauth:grant-type:device_code"
                        }
                      ]
                    },
                    {
                      "name": "client_id",
                      "value": [
                        {
                          "type": "client_id"
                        }
                      ]
                    },
                    {
                      "name": "device_code",
                      "value": [
                        {
                          "type": "device_code"
                        }
                      ]
                    }
                  ]
                }
              },
              "response": {
                "encoding": "json",
                "fields": [
                  {
                    "path": [
                      "access_token"
                    ],
                    "target": {
                      "type": "slot",
                      "name": "github_token"
                    },
                    "required": true
                  }
                ]
              },
              "terminal_codes": [
                "authorization_pending",
                "slow_down",
                "expired_token",
                "unsupported_grant_type",
                "incorrect_client_credentials",
                "incorrect_device_code",
                "device_flow_disabled",
                "personal_access_token",
                "access_denied"
              ]
            },
            "refresh": {
              "type": "reprovision"
            }
          },
          "provisioning": [
            {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://api.github.com/copilot_internal/v2/token"
                  }
                ],
                "headers": [
                  {
                    "name": "Authorization",
                    "value": [
                      {
                        "type": "literal",
                        "value": "Bearer "
                      },
                      {
                        "type": "slot",
                        "name": "github_token"
                      }
                    ]
                  }
                ]
              },
              "response_encoding": "json",
              "extract": [
                {
                  "path": [
                    "token"
                  ],
                  "target": {
                    "type": "access_token"
                  },
                  "shape": {
                    "type": "string"
                  },
                  "required": true
                },
                {
                  "path": [
                    "expires_at"
                  ],
                  "target": {
                    "type": "expires_at_unix_seconds"
                  },
                  "shape": {
                    "type": "integer"
                  },
                  "required": true
                },
                {
                  "path": [
                    "endpoints",
                    "api"
                  ],
                  "target": {
                    "type": "base_url"
                  },
                  "shape": {
                    "type": "string"
                  },
                  "required": true
                }
              ]
            }
          ],
          "discovery": {
            "request": {
              "method": "GET",
              "url": [
                {
                  "type": "base"
                },
                {
                  "type": "literal",
                  "value": "/models"
                }
              ],
              "headers": [
                {
                  "name": "Authorization",
                  "value": [
                    {
                      "type": "literal",
                      "value": "Bearer "
                    },
                    {
                      "type": "secret"
                    }
                  ]
                }
              ]
            },
            "extract": {
              "mode": "array",
              "array_at": [
                "data"
              ],
              "id_field": [
                "id"
              ]
            },
            "pagination": {
              "type": "cursor",
              "has_more_path": [
                "has_more"
              ],
              "last_id_path": [
                "last_id"
              ],
              "request_param": "after",
              "max_pages": 10
            }
          }
        })
    }

    /// Parses and validates a payload document, flattening a parse rejection and a validation
    /// rejection into one string result.
    fn validate_json(payload: serde_json::Value) -> Result<(), String> {
        let parsed = serde_json::from_value::<DefinitionPayload>(payload)
            .map_err(|error| error.to_string())?;

        parsed.validate().map_err(|error| error.message)
    }

    /// Applies one mutation to a nested document and validates the result.
    fn mutate_and_validate(
        base: serde_json::Value,
        mutate: impl FnOnce(&mut serde_json::Value),
    ) -> Result<(), String> {
        let mut payload = base;

        mutate(&mut payload);

        validate_json(payload)
    }

    /// Builds the coalesce-argument probe: a `reference` value part nested inside a coalesce
    /// argument satisfies the stage's reference requirement.
    fn coalesce_argument(reference: &str) -> serde_json::Value {
        serde_json::json!([
            {
                "type": "coalesce",
                "parts": [
                    { "type": reference },
                    { "type": "literal", "value": "fallback" }
                ]
            }
        ])
    }

    /// The accepted researched shapes (README §5.6): the Antigravity, Copilot, Codex, and
    /// static-key definitions all parse and validate, and each payload round-trips through its
    /// JSON form.
    ///
    /// # Panics
    ///
    /// Panics if an accepted fixture fails validation or its round-trip diverges.
    #[test]
    fn accepts_the_researched_definition_shapes() {
        for (name, payload) in [
            ("antigravity", good_antigravity_json()),
            ("copilot", good_copilot_json()),
        ] {
            let parsed = serde_json::from_value::<DefinitionPayload>(payload.clone()).unwrap();

            parsed.validate().unwrap();

            let rendered = serde_json::to_value(&parsed).unwrap();

            let reparsed = serde_json::from_value::<DefinitionPayload>(rendered).unwrap();

            assert_eq!(reparsed, parsed, "fixture `{name}` must round-trip");
        }

        // The Codex shape: PKCE, identity-token claims into slots, a JSON refresh grant, always
        // streaming, the render option, and token pagination over an aliased array.
        let codex = serde_json::json!({
          "format": "openai_response",
          "inference": {
            "url": [
              {
                "type": "literal",
                "value": "https://chatgpt.com/backend-api/codex/responses"
              }
            ],
            "headers": [
              {
                "name": "Authorization",
                "value": [
                  {
                    "type": "literal",
                    "value": "Bearer "
                  },
                  {
                    "type": "secret"
                  }
                ]
              },
              {
                "name": "chatgpt-account-id",
                "value": [
                  {
                    "type": "slot",
                    "name": "account_id"
                  }
                ]
              },
              {
                "name": "Originator",
                "value": [
                  {
                    "type": "literal",
                    "value": "codex_cli_rs"
                  }
                ]
              },
              {
                "name": "Session-Id",
                "value": [
                  {
                    "type": "request_uuid"
                  }
                ]
              }
            ],
            "upstream_streaming": "always",
            "render_options": [
              "omit_max_output_tokens"
            ]
          },
          "credential": {
            "type": "oauth",
            "client_id": "app_EMoamEEZ73f0CkXaXp7hrann",
            "grant": {
              "type": "authorization_code",
              "authorize_url": [
                {
                  "type": "literal",
                  "value": "https://auth.openai.com/oauth/authorize"
                }
              ],
              "redirect_uri": "http://localhost:1455/auth/callback",
              "pkce": "s256",
              "extra_params": [
                {
                  "name": "scope",
                  "value": [
                    {
                      "type": "literal",
                      "value": "openid profile email offline_access"
                    }
                  ]
                },
                {
                  "name": "originator",
                  "value": [
                    {
                      "type": "literal",
                      "value": "codex_cli_rs"
                    }
                  ]
                }
              ]
            },
            "exchange": {
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://auth.openai.com/oauth/token"
                  }
                ],
                "body": {
                  "kind": "form",
                  "fields": [
                    {
                      "name": "grant_type",
                      "value": [
                        {
                          "type": "literal",
                          "value": "authorization_code"
                        }
                      ]
                    },
                    {
                      "name": "code",
                      "value": [
                        {
                          "type": "authorization_code"
                        }
                      ]
                    },
                    {
                      "name": "redirect_uri",
                      "value": [
                        {
                          "type": "redirect_uri"
                        }
                      ]
                    },
                    {
                      "name": "client_id",
                      "value": [
                        {
                          "type": "client_id"
                        }
                      ]
                    },
                    {
                      "name": "code_verifier",
                      "value": [
                        {
                          "type": "code_verifier"
                        }
                      ]
                    }
                  ]
                }
              },
              "response": {
                "encoding": "json",
                "fields": [
                  {
                    "path": [
                      "access_token"
                    ],
                    "target": {
                      "type": "access_token"
                    },
                    "required": true
                  },
                  {
                    "path": [
                      "refresh_token"
                    ],
                    "target": {
                      "type": "refresh_token"
                    },
                    "required": true
                  },
                  {
                    "path": [
                      "id_token"
                    ],
                    "target": {
                      "type": "id_token"
                    },
                    "required": true
                  },
                  {
                    "path": [
                      "expires_in"
                    ],
                    "target": {
                      "type": "expires_in_seconds"
                    },
                    "required": false
                  }
                ]
              },
              "terminal_codes": [
                "invalid_grant",
                "token_expired",
                "invalid_token",
                "token_exchange_user_error"
              ]
            },
            "id_token_claims": [
              {
                "claim": [
                  "https://api.openai.com/auth",
                  "chatgpt_account_id"
                ],
                "slot": "account_id"
              }
            ],
            "refresh": {
              "type": "grant",
              "request": {
                "method": "POST",
                "url": [
                  {
                    "type": "literal",
                    "value": "https://auth.openai.com/oauth/token"
                  }
                ],
                "body": {
                  "kind": "json",
                  "node": {
                    "type": "object",
                    "fields": {
                      "client_id": {
                        "type": "string",
                        "expr": [
                          {
                            "type": "client_id"
                          }
                        ]
                      },
                      "grant_type": {
                        "type": "string",
                        "expr": [
                          {
                            "type": "literal",
                            "value": "refresh_token"
                          }
                        ]
                      },
                      "refresh_token": {
                        "type": "string",
                        "expr": [
                          {
                            "type": "refresh_token"
                          }
                        ]
                      }
                    }
                  }
                }
              },
              "terminal_codes": [
                "invalid_grant",
                "token_expired",
                "invalid_token",
                "refresh_token_expired",
                "refresh_token_reused",
                "refresh_token_invalidated"
              ]
            }
          },
          "discovery": {
            "request": {
              "method": "GET",
              "url": [
                {
                  "type": "literal",
                  "value": "https://chatgpt.com/backend-api/codex/models"
                }
              ],
              "query": [
                {
                  "name": "client_version",
                  "value": [
                    {
                      "type": "literal",
                      "value": "0.147.0"
                    }
                  ]
                }
              ],
              "headers": [
                {
                  "name": "Authorization",
                  "value": [
                    {
                      "type": "literal",
                      "value": "Bearer "
                    },
                    {
                      "type": "secret"
                    }
                  ]
                },
                {
                  "name": "chatgpt-account-id",
                  "value": [
                    {
                      "type": "slot",
                      "name": "account_id"
                    }
                  ]
                }
              ]
            },
            "extract": {
              "mode": "array",
              "array_at": [
                "items"
              ],
              "id_field": [
                "slug"
              ]
            }
          }
        });

        validate_json(codex).unwrap();

        // The static-key shape: a bare API-key recipe whose inference and discovery speak of the
        // secret and the base only.
        let static_key = serde_json::json!({
          "format": "anthropic_messages",
          "inference": {
            "url": [
              {
                "type": "base"
              },
              {
                "type": "literal",
                "value": "/v1/messages"
              }
            ],
            "headers": [
              {
                "name": "x-api-key",
                "value": [
                  {
                    "type": "secret"
                  }
                ]
              },
              {
                "name": "anthropic-version",
                "value": [
                  {
                    "type": "literal",
                    "value": "2023-06-01"
                  }
                ]
              }
            ],
            "upstream_streaming": "match_client"
          },
          "credential": {
            "type": "api_key"
          },
          "discovery": {
            "request": {
              "method": "GET",
              "url": [
                {
                  "type": "base"
                },
                {
                  "type": "literal",
                  "value": "/v1/models"
                }
              ],
              "headers": [
                {
                  "name": "x-api-key",
                  "value": [
                    {
                      "type": "secret"
                    }
                  ]
                }
              ]
            },
            "extract": {
              "mode": "array",
              "array_at": [
                "data"
              ],
              "id_field": [
                "id"
              ]
            }
          }
        });

        validate_json(static_key).unwrap();
    }

    /// A `Body` placeholder never appears in a control request — its JSON body is fully
    /// described by the definition.
    ///
    /// # Panics
    ///
    /// Panics if the mutated fixture validates.
    #[test]
    fn rejects_a_body_placeholder_in_a_control_request() {
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["provisioning"][0]["request"]["body"]["kind"] = serde_json::json!("json");
            payload["provisioning"][0]["request"]["body"]["node"] = serde_json::json!({
              "type": "array",
              "items": [
                {
                  "type": "body"
                }
              ]
            });
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("body placeholder"), "{error}");
    }

    /// An inference envelope carries its body placeholder exactly once — never zero, never two.
    ///
    /// # Panics
    ///
    /// Panics if a mutated envelope validates.
    #[test]
    fn rejects_envelopes_without_exactly_one_body() {
        for parts in [0, 2] {
            let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
                // Reduce or duplicate the envelope's single body placeholder.
                let envelope = &mut payload["inference"]["envelope"]["fields"];

                if parts == 0 {
                    envelope["request"] =
                        serde_json::json!({"type": "string", "expr": literal("x")});
                } else {
                    envelope["requestId"] = serde_json::json!({
                      "type": "body"
                    });
                }
            });

            let error = outcome.unwrap_err();

            assert!(
                error.contains("body placeholders"),
                "parts: {parts}, error: {error}"
            );
        }
    }

    /// Each stage carries only the values it produces: the routed model and the request UUID are
    /// inference-only, the authorization code and its verifier are exchange-only under an
    /// authorization-code grant, the device code is exchange-only under a device grant, the
    /// refresh token is refresh-only, and the secret never reaches the device authorization or
    /// the token exchange.
    ///
    /// # Panics
    ///
    /// Panics if an out-of-stage value validates.
    #[test]
    fn rejects_values_outside_their_stage() {
        // The routed model inside the exchange request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"][2]["value"] = json!([
              {
                "type": "model"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("model"), "{error}");

        // The secret inside the device authorization request.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["request"]["headers"] = json!([
              {
                "name": "Authorization",
                "value": [
                  {
                    "type": "secret"
                  }
                ]
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("secret"), "{error}");

        // The device code inside an authorization-code exchange.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"][1]["value"] = json!([
              {
                "type": "device_code"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("device code"), "{error}");

        // The authorization code inside a device-grant exchange.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"][1]["value"] = json!([
              {
                "type": "authorization_code"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("authorization code"), "{error}");

        // The refresh token inside a provisioning request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["provisioning"][0]["request"]["headers"][0]["value"] = serde_json::json!([
              {
                "type": "refresh_token"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("refresh token"), "{error}");

        // The request UUID inside the discovery request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["discovery"]["request"]["headers"][0]["value"] = serde_json::json!([
              {
                "type": "request_uuid"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("request UUID"), "{error}");
    }

    /// Every slot reference — in expressions, guards, and template strings — resolves against
    /// the namespace the recipe writes, and an API-key recipe that references a slot is rejected
    /// because it defines none.
    ///
    /// # Panics
    ///
    /// Panics if an undefined slot reference validates.
    #[test]
    fn rejects_undefined_slot_references() {
        // The Copilot provisioning header names an unrecorded slot.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["provisioning"][0]["request"]["headers"][0]["value"] = serde_json::json!([
              {
                "type": "literal",
                "value": "Bearer "
              },
              {
                "type": "slot",
                "name": "ghost"
              }
            ]);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("`ghost`"), "{error}");

        // A guard naming an unrecorded slot.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["provisioning"][0]["skip_if"] = serde_json::json!("nowhere");
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("`nowhere`"), "{error}");

        // A static-key definition has no slot namespace at all.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"] = serde_json::json!({
              "type": "api_key"
            });
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("slot `project`"), "{error}");
    }

    /// Malformed templates are rejected at write time: an empty coalesce, a coalesce nested in a
    /// coalesce, an empty URL expression, and an empty path member.
    ///
    /// # Panics
    ///
    /// Panics if a malformed template validates.
    #[test]
    fn rejects_malformed_templates() {
        let empty_coalesce = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["inference"]["headers"][1]["value"] = serde_json::json!([
              {
                "type": "coalesce",
                "parts": []
              }
            ]);
        });

        assert!(empty_coalesce.unwrap_err().contains("empty coalesce"));

        let nested_coalesce = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["inference"]["headers"][1]["value"] = serde_json::json!([
              {
                "type": "coalesce",
                "parts": [
                  {
                    "type": "coalesce",
                    "parts": [
                      {
                        "type": "literal",
                        "value": "x"
                      }
                    ]
                  }
                ]
              }
            ]);
        });

        assert!(nested_coalesce.unwrap_err().contains("nests a coalesce"));

        let empty_url = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["inference"]["url"] = serde_json::json!([]);
        });

        assert!(
            empty_url
                .unwrap_err()
                .contains("at least one expression part")
        );

        let empty_path_member = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["provisioning"][0]["extract"][1]["path"] = serde_json::json!(["paidTier", ""]);
        });

        assert!(empty_path_member.unwrap_err().contains("empty member"));
    }

    /// The OAuth sections must agree (README §5.3): the access token is mapped by the exchange
    /// or a provisioning step, a refresh grant requires a produced refresh token, reprovisioning
    /// requires provisioning steps, and terminal-code lists are non-empty.
    ///
    /// # Panics
    ///
    /// Panics if an inconsistent OAuth section validates.
    #[test]
    fn rejects_inconsistent_oauth_sections() {
        // Neither the exchange nor provisioning maps the access token.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["response"]["fields"][0]["target"] = json!({
              "type": "id_token"
            });
        });

        assert!(outcome.unwrap_err().contains("access token"));

        // A refresh grant with no refresh token produced anywhere.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["response"]["fields"][1]["target"] = json!({
              "type": "id_token"
            });
        });

        assert!(outcome.unwrap_err().contains("refresh token"));

        // Reprovisioning without provisioning steps: the Antigravity exchange maps the access
        // token itself, so flipping its refresh mode violates only the reprovision rule. The
        // dropped steps' project slot moves into the initial slots, so no unrelated slot
        // reference dangles.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"] = serde_json::json!({
              "type": "reprovision"
            });
            payload["credential"]["initial_slots"] = serde_json::json!([
              {
                "name": "device_id",
                "generator": "uuid_hex"
              },
              {
                "name": "project",
                "generator": "uuid_hex"
              }
            ]);
            payload["provisioning"] = serde_json::Value::Null;
        });

        assert!(outcome.unwrap_err().contains("reprovision"));

        // An empty exchange terminal-code list.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["terminal_codes"] = serde_json::json!([]);
        });

        assert!(outcome.unwrap_err().contains("terminal codes"));
    }

    /// RFC 7636 §4.6: a grant that sent a code challenge must present the verifier at the token
    /// endpoint, so an `s256` grant whose exchange request never references the `code_verifier`
    /// value part is rejected at the write boundary — symmetric to the device grant's
    /// required-target checks — instead of dying at the upstream with `invalid_grant`. Without
    /// PKCE the same exchange stays valid.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming `s256` exchange validates.
    #[test]
    fn requires_the_s256_exchange_to_reference_the_code_verifier() {
        // The Antigravity shape's exchange carries no verifier anywhere; flipping its grant to
        // `s256` leaves the challenge with no verifier presented at the token endpoint.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["grant"]["pkce"] = serde_json::json!("s256");
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("code_verifier"), "{error}");

        // Without PKCE the verifier is absent by design.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The `s256` rule reads the whole exchange request, so the verifier satisfies it wherever
    /// the stage's expressions live: a form field, a query row, or a coalesce argument. (The
    /// JSON-body leaves are covered by the Claude Code and Devin catalog fixtures above.)
    ///
    /// # Panics
    ///
    /// Panics if a verifier-carrying exchange is rejected.
    #[test]
    fn accepts_an_s256_exchange_referencing_the_code_verifier() {
        // As the exchange's form field.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["grant"]["pkce"] = serde_json::json!("s256");

            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "name": "code_verifier",
                    "value": [{ "type": "code_verifier" }]
                }));
        });

        outcome.unwrap();

        // As a query row on the exchange request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["grant"]["pkce"] = serde_json::json!("s256");

            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
                { "name": "code_verifier", "value": [{ "type": "code_verifier" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments — over the `client_secret` field, whose value is free
        // to change: the `code` field stays intact under the code-reference rule.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["grant"]["pkce"] = serde_json::json!("s256");

            let field = &mut payload["credential"]["exchange"]["request"]["body"]["fields"][3];

            field["value"] = coalesce_argument("code_verifier");
        });

        outcome.unwrap();
    }

    /// RFC 6749 §4.1.3: the token request repeats the redirect URI whenever the authorization
    /// request included it — and the builder appends the definition's `redirect_uri` to every
    /// authorize URL — so an authorization-code exchange that never references the
    /// `redirect_uri` value part is rejected at the write boundary, symmetric to the `s256`
    /// verifier rule, instead of dying at the upstream with `invalid_request`. A device grant
    /// carries no redirect URI and stays valid (the Copilot fixture above).
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming exchange validates.
    #[test]
    fn requires_the_authorization_code_exchange_to_reference_the_redirect_uri() {
        // The Antigravity shape's exchange presents the redirect URI as its final form field;
        // dropping it leaves the token endpoint with no redirect URI to repeat.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(4);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("redirect_uri"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture references the redirect URI as a form field, so every
        // authorization-code definition the catalog already carries stays valid.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The redirect-URI rule reads the whole exchange request, so the value part satisfies it
    /// wherever the stage's expressions live: the fixture's own form field, a query row, or a
    /// coalesce argument.
    ///
    /// # Panics
    ///
    /// Panics if a redirect-URI-carrying exchange is rejected.
    #[test]
    fn accepts_an_authorization_code_exchange_referencing_the_redirect_uri() {
        // As a query row on the exchange request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(4);

            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
                { "name": "redirect_uri", "value": [{ "type": "redirect_uri" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments — over the `client_secret` field, whose value is free
        // to change: the `code` field stays intact under the code-reference rule.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(4);

            let field = &mut payload["credential"]["exchange"]["request"]["body"]["fields"][3];

            field["value"] = coalesce_argument("redirect_uri");
        });

        outcome.unwrap();
    }

    /// RFC 6749 §4.1.3: the token request presents the pasted authorization code — `code` is
    /// REQUIRED — so an authorization-code exchange that never references the
    /// `authorization_code` value part is rejected at the write boundary, symmetric to the
    /// `s256` verifier and redirect-URI rules, instead of burning every flow on its first
    /// exchange attempt (README §7.2). The untouched fixture references the code as a form
    /// field, so every researched authorization-code shape stays valid.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming exchange validates.
    #[test]
    fn requires_the_authorization_code_exchange_to_reference_the_code() {
        // The Antigravity shape's exchange presents the code as its second form field;
        // dropping it leaves the token endpoint with no code to present.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(1);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("authorization_code"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture references the code as a form field.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The code rule reads the whole exchange request, so the value part satisfies it wherever
    /// the stage's expressions live: a query row, a coalesce argument, or a JSON body leaf
    /// (the Claude Code and Devin catalog fixtures above carry the latter).
    ///
    /// # Panics
    ///
    /// Panics if a code-carrying exchange is rejected.
    #[test]
    fn accepts_an_authorization_code_exchange_referencing_the_code() {
        // As a query row on the exchange request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(1);

            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
                { "name": "code", "value": [{ "type": "authorization_code" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            let field = &mut payload["credential"]["exchange"]["request"]["body"]["fields"][1];

            field["value"] = coalesce_argument("authorization_code");
        });

        outcome.unwrap();
    }

    /// RFC 6749 §4.1.3: the token request carries the client ID — "REQUIRED, if the client is
    /// not authenticating with the authorization server" (§3.2.1) — and a credential with no
    /// `client_secret` never authenticates, so a secret-less credential whose exchange request
    /// never references the `client_id` value part is rejected at the write boundary, symmetric
    /// to the other required-carry rules, instead of dying at the upstream with `invalid_client`
    /// on every flow. A credential with a client secret authenticates with it and stays exempt.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming secret-less exchange validates.
    #[test]
    fn requires_a_secret_less_exchange_to_reference_the_client_id() {
        // The Antigravity shape's exchange presents the client ID as its third form field;
        // dropping it — and the credential's secret, which is the RFC's not-authenticating
        // condition, together with the secret field presenting it — leaves the token endpoint
        // with no client identification at all.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
            fields.remove(2);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("client_id"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture authenticates with its secret and presents the client ID
        // besides.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The client-ID rule binds only the secret-less credentials — a credential with a client
    /// secret authenticates with it, and its exchange may omit the ID — and reads the whole
    /// exchange request, so the value part satisfies it wherever the stage's expressions live.
    /// (The JSON-body leaves are covered by the Codex and Claude Code catalog fixtures above.)
    ///
    /// # Panics
    ///
    /// Panics if a client-ID-carrying secret-less exchange is rejected.
    #[test]
    fn accepts_a_secret_less_exchange_referencing_the_client_id() {
        // The fixture's own form field, once the credential's secret is gone together with the
        // exchange's secret field presenting it.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
        });

        outcome.unwrap();

        // As a query row on the exchange request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
            fields.remove(2);

            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
                { "name": "client_id", "value": [{ "type": "client_id" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);

            let field = &mut fields[2];

            field["value"] = coalesce_argument("client_id");
        });

        outcome.unwrap();

        // A credential with a client secret authenticates with it: its exchange may omit the
        // client ID outright.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(2);
        });

        outcome.unwrap();
    }

    /// RFC 8628 §3.4: the token request carries the flow's device code — `device_code` is
    /// REQUIRED — and under a device grant this same request doubles as the polling request,
    /// so a device exchange that never references the `device_code` value part is rejected at
    /// the write boundary, symmetric to the authorization-code rules, instead of riding
    /// non-terminal rejections until the flow's own expiry (README §5.3, §7.2). The untouched
    /// Copilot fixture references the code as a form field.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming exchange validates.
    #[test]
    fn requires_the_device_exchange_to_reference_the_device_code() {
        // The Copilot shape's exchange presents the device code as its final form field;
        // dropping it leaves every poll with no device code to present.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(2);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("device_code"), "{error}");
        assert!(error.contains("RFC 8628"), "{error}");

        // The untouched fixture references the device code as a form field.
        mutate_and_validate(good_copilot_json(), |_| {}).unwrap();
    }

    /// The device-code rule reads the whole exchange request, so the value part satisfies it
    /// wherever the stage's expressions live: the fixture's own form field, a query row, or a
    /// coalesce argument.
    ///
    /// # Panics
    ///
    /// Panics if a device-code-carrying exchange is rejected.
    #[test]
    fn accepts_a_device_exchange_referencing_the_device_code() {
        // As a query row on the exchange request.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["exchange"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(2);

            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
                { "name": "device_code", "value": [{ "type": "device_code" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let field = &mut payload["credential"]["exchange"]["request"]["body"]["fields"][2];

            field["value"] = coalesce_argument("device_code");
        });

        outcome.unwrap();
    }

    /// RFC 8628 §3.1: the device-authorization request carries the client ID — "REQUIRED, if
    /// the client is not authenticating with the authorization server" (RFC 6749 §3.2.1) — and
    /// a credential with no `client_secret` never authenticates, so a secret-less device grant
    /// whose authorization request never references the `client_id` value part is rejected at
    /// the write boundary, symmetric to the exchange and refresh client-ID rules, instead of
    /// dying at the upstream with `invalid_client` on every flow. A credential with a client
    /// secret authenticates with it and stays exempt. The untouched Copilot fixture references
    /// the client ID as a form field.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming secret-less device authorization validates.
    #[test]
    fn requires_a_secret_less_device_authorization_to_reference_the_client_id() {
        // The Copilot shape's device-authorization request presents the client ID as its first
        // form field; dropping it leaves the stage with no client identification at all.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(0);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("client_id"), "{error}");
        assert!(error.contains("RFC 8628"), "{error}");
        assert!(error.contains("device authorization"), "{error}");

        // The untouched fixture is secret-less and references the client ID as a form field.
        mutate_and_validate(good_copilot_json(), |_| {}).unwrap();
    }

    /// The client-ID rule binds only the secret-less credentials — a credential with a client
    /// secret authenticates with it, and its device-authorization request may omit the ID — and
    /// reads the whole request, so the value part satisfies it wherever the stage's expressions
    /// live.
    ///
    /// # Panics
    ///
    /// Panics if a client-ID-carrying secret-less device authorization is rejected.
    #[test]
    fn accepts_a_secret_less_device_authorization_referencing_the_client_id() {
        // As a query row on the device-authorization request.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(0);

            payload["credential"]["grant"]["authorization"]["request"]["query"] =
                serde_json::json!([{ "name": "client_id", "value": [{ "type": "client_id" }] }]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let request = &mut payload["credential"]["grant"]["authorization"]["request"];

            let field = &mut request["body"]["fields"][0];

            field["value"] = coalesce_argument("client_id");
        });

        outcome.unwrap();

        // A credential with a client secret authenticates with it: its device-authorization
        // request may omit the client ID outright.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::json!("copilot-secret");

            payload["credential"]["grant"]["authorization"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(0);
        });

        outcome.unwrap();
    }

    /// RFC 6749 §6: renewing an access token is a `grant_type=refresh_token` request carrying
    /// the stored refresh token — `refresh_token` is REQUIRED — so a refresh grant whose
    /// request never references the `refresh_token` value part is rejected at the write
    /// boundary, symmetric to the exchange rules above. Left in place, every renewal dies at
    /// the upstream and the rejection code lands in the grant's `terminal_codes`, so the flow
    /// misdiagnoses the healthy provider as one whose grant is revoked or expired and needs
    /// re-authorization — an endless re-authorize/refresh-fail loop. The untouched fixture
    /// references the refresh token as a form field.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming refresh grant validates.
    #[test]
    fn requires_the_refresh_grant_to_reference_the_refresh_token() {
        // The Antigravity shape's refresh request presents the refresh token as its second
        // form field; dropping it leaves every renewal with no refresh token to present.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(1);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("refresh_token"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture references the refresh token as a form field.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The refresh-token rule reads the whole refresh request, so the value part satisfies it
    /// wherever the stage's expressions live: the fixture's own form field, a query row, or a
    /// coalesce argument. (The JSON-body leaves are covered by the Codex, Claude Code, and
    /// Kimi Code catalog fixtures above.)
    ///
    /// # Panics
    ///
    /// Panics if a refresh-token-carrying refresh grant is rejected.
    #[test]
    fn accepts_a_refresh_grant_referencing_the_refresh_token() {
        // As a query row on the refresh request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(1);

            payload["credential"]["refresh"]["request"]["query"] = serde_json::json!([
                { "name": "refresh_token", "value": [{ "type": "refresh_token" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            let field = &mut payload["credential"]["refresh"]["request"]["body"]["fields"][1];

            field["value"] = coalesce_argument("refresh_token");
        });

        outcome.unwrap();
    }

    /// RFC 6749 §6: a refresh-token grant request re-authenticates the client the same way the
    /// token endpoint requires (RFC 6749 §4.1.3's §3.2.1 rule), so a secret-less credential
    /// whose refresh grant never references the `client_id` value part is rejected at the write
    /// boundary, symmetric to the refresh-token rule above, instead of dying at the upstream
    /// with `invalid_client` on every renewal. A credential with a client secret authenticates
    /// its renewal with it and stays exempt.
    ///
    /// # Panics
    ///
    /// Panics if the non-conforming secret-less refresh grant validates.
    #[test]
    fn requires_a_secret_less_refresh_grant_to_reference_the_client_id() {
        // The Antigravity shape's refresh request presents the client ID as its third form
        // field; dropping it — and the credential's secret, which is the RFC's
        // not-authenticating condition, together with the secret field presenting it — leaves
        // every renewal with no client identification at all.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
            fields.remove(2);
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("client_id"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture authenticates its renewal with its secret and presents the
        // client ID besides.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// The client-ID rule binds only the secret-less credentials — a credential with a client
    /// secret authenticates its renewal with it, and its refresh grant may omit the ID — and
    /// reads the whole refresh request, so the value part satisfies it wherever the stage's
    /// expressions live.
    ///
    /// # Panics
    ///
    /// Panics if a client-ID-carrying secret-less refresh grant is rejected.
    #[test]
    fn accepts_a_secret_less_refresh_grant_referencing_the_client_id() {
        // The fixture's own form field, once the credential's secret is gone together with the
        // refresh request's secret field presenting it.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
        });

        outcome.unwrap();

        // As a query row on the refresh request.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);
            fields.remove(2);

            payload["credential"]["refresh"]["request"]["query"] = serde_json::json!([
                { "name": "client_id", "value": [{ "type": "client_id" }] }
            ]);
        });

        outcome.unwrap();

        // Inside a coalesce's arguments.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["client_secret"] = serde_json::Value::Null;

            let fields = payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap();

            fields.remove(3);

            let field = &mut fields[2];

            field["value"] = coalesce_argument("client_id");
        });

        outcome.unwrap();

        // A credential with a client secret authenticates its renewal with it: its refresh
        // grant may omit the client ID outright.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"]["request"]["body"]["fields"]
                .as_array_mut()
                .unwrap()
                .remove(2);
        });

        outcome.unwrap();
    }

    /// RFC 6749 §6: the refresh response carries the access token — REQUIRED — and a declared
    /// refresh response mapping wholly replaces the exchange's default as the renewal's fold
    /// input, so a declared mapping with no `access_token` target folds every renewal into a
    /// tokenless outcome: the lease is acquired and cleared first, and the renewal then dies
    /// on the missing token. The Antigravity fixture's mapping is defaulted, so the rejection
    /// case declares the exchange's mapping with its `access_token` row removed.
    ///
    /// # Panics
    ///
    /// Panics if the tokenless declared refresh mapping validates.
    #[test]
    fn requires_a_declared_refresh_response_to_map_the_access_token() {
        // The exchange's mapping minus its `access_token` row, declared wholesale: the
        // default it replaces is the only token source the renewal would have folded.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            let declared: Vec<_> = payload["credential"]["exchange"]["response"]["fields"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|field| field["target"]["type"] != "access_token")
                .cloned()
                .collect();

            payload["credential"]["refresh"]["response"] = json!({
              "encoding": "json",
              "fields": declared
            });
        });

        let error = outcome.unwrap_err();

        assert!(error.contains("access_token"), "{error}");
        assert!(error.contains("RFC 6749"), "{error}");

        // The untouched fixture's mapping stays defaulted, so the exchange's own required
        // `access_token` row is the renewal's fold input.
        mutate_and_validate(good_antigravity_json(), |_| {}).unwrap();
    }

    /// A declared refresh response mapping that carries the `access_token` target satisfies
    /// the rule — the declared rows are the renewal's whole fold input — whether it declares
    /// the exchange's mapping wholesale or a narrower one whose token row is the only target.
    ///
    /// # Panics
    ///
    /// Panics if a token-carrying declared refresh mapping is rejected.
    #[test]
    fn accepts_a_declared_refresh_response_mapping_the_access_token() {
        // The exchange's full mapping, declared: the token row rides along.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            let response = payload["credential"]["exchange"]["response"].clone();

            payload["credential"]["refresh"]["response"] = response;
        });

        outcome.unwrap();

        // A narrower declared mapping whose `access_token` row is the only target.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"]["response"] = json!({
              "encoding": "json",
              "fields": [
                {
                  "path": ["access_token"],
                  "target": {
                    "type": "access_token"
                  },
                  "required": true
                }
              ]
            });
        });

        outcome.unwrap();
    }

    /// A `reprovision` refresh requires its provisioning sequence to extract an absolute
    /// `expires_at_unix_seconds` (README §5.4): a renewal without a renewed expiry would
    /// rewrite the stored one to `null` — the no-scheduled-refresh state — and the credential
    /// would silently never refresh again, so the state is unrepresentable for a valid
    /// definition.
    ///
    /// # Panics
    ///
    /// Panics if a reprovision sequence without an expiry extraction validates.
    #[test]
    fn rejects_a_reprovision_without_an_expiry_extraction() {
        // Retarget the Copilot sequence's `expires_at_unix_seconds` extraction at a slot.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["provisioning"][0]["extract"][1]["target"] = json!({
              "type": "slot",
              "name": "expiry"
            });
        });

        assert!(
            outcome.unwrap_err().contains("expires_at_unix_seconds"),
            "the rejection names the missing expiry extraction"
        );

        // Drop the extraction outright, keeping the sequence non-empty and access-token
        // supplying.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["provisioning"][0]["extract"] = json!([
              {
                "path": ["token"],
                "target": {"type": "access_token"},
                "shape": {"type": "string"},
                "required": true
              }
            ]);
        });

        assert!(outcome.unwrap_err().contains("expires_at_unix_seconds"));
    }

    /// The device authorization response must map both the `device_code` and `user_code`
    /// targets — marked required — and at least one verification URI, and it must configure the
    /// explicit expiry fallback exactly when it maps no required `expires_in_seconds` target:
    /// an optional expiry mapping leaves an upstream omission with no lifetime source.
    ///
    /// # Panics
    ///
    /// Panics if a non-conforming device response validates.
    #[test]
    fn rejects_non_conforming_device_responses() {
        // Drop the user_code mapping.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let fields = &mut payload["credential"]["grant"]["authorization"]["response"]["fields"];
            *fields = serde_json::json!([
              {
                "path": [
                  "device_code"
                ],
                "target": {
                  "type": "device_code"
                },
                "required": true
              },
              {
                "path": [
                  "verification_uri"
                ],
                "target": {
                  "type": "verification_uri"
                },
                "required": false
              }
            ]);
        });

        assert!(outcome.unwrap_err().contains("user_code"));

        // Mark the device code optional.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["response"]["fields"][0]["required"] =
                serde_json::json!(false);
        });

        assert!(outcome.unwrap_err().contains("required"));

        // Drop both verification-URI mappings.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let fields = &mut payload["credential"]["grant"]["authorization"]["response"]["fields"];
            let kept: Vec<_> = fields
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|field| {
                    !field["target"]["type"].as_str().is_some_and(|target| {
                        target == "verification_uri" || target == "verification_uri_complete"
                    })
                })
                .collect();
            *fields = serde_json::Value::Array(kept);
        });

        assert!(outcome.unwrap_err().contains("verification URI"));

        // Drop the fallback and the `expires_in_seconds` mapping together: the definition must
        // carry one explicit expiry source of the two.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let grant = &mut payload["credential"]["grant"];
            grant["expires_in_fallback_seconds"] = serde_json::Value::Null;

            let fields = &mut grant["authorization"]["response"]["fields"];
            let kept: Vec<_> = fields
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|field| {
                    !field["target"]["type"]
                        .as_str()
                        .is_some_and(|target| target == "expires_in_seconds")
                })
                .collect();
            *fields = serde_json::Value::Array(kept);
        });

        assert!(outcome.unwrap_err().contains("expires_in_fallback_seconds"));

        // The expiry mapping optional with no fallback is the same rejection: an upstream that
        // omits the member would leave the flow with no lifetime source at all.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            let grant = &mut payload["credential"]["grant"];
            grant["expires_in_fallback_seconds"] = serde_json::Value::Null;

            #[expect(clippy::expect_used, reason = "the fixture maps response fields")]
            let fields = grant["authorization"]["response"]["fields"]
                .as_array_mut()
                .expect("the fixture maps response fields");

            for field in fields {
                if field["target"]["type"] == "expires_in_seconds" {
                    field["required"] = serde_json::json!(false);
                }
            }
        });

        assert!(outcome.unwrap_err().contains("expires_in_fallback_seconds"));

        // The expiry mapping required with no fallback is the accepted shape: the response must
        // carry the member, so a lifetime always exists.
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["expires_in_fallback_seconds"] = serde_json::Value::Null;

            #[expect(clippy::expect_used, reason = "the fixture maps response fields")]
            let fields = payload["credential"]["grant"]["authorization"]["response"]["fields"]
                .as_array_mut()
                .expect("the fixture maps response fields");

            for field in fields {
                if field["target"]["type"] == "expires_in_seconds" {
                    field["required"] = serde_json::json!(true);
                }
            }
        });

        assert!(
            outcome.is_ok(),
            "a required expiry mapping needs no fallback"
        );
    }

    /// Render options outside the definition format's closed set are rejected; the accepted
    /// option on its own format stays.
    ///
    /// # Panics
    ///
    /// Panics if an out-of-set render option validates.
    #[test]
    fn rejects_render_options_outside_the_format_set() {
        let outcome = mutate_and_validate(good_copilot_json(), |payload| {
            payload["inference"]["render_options"] = serde_json::json!(["omit_max_output_tokens"]);
        });

        let error = outcome.unwrap_err();

        assert!(
            error.contains("omit_max_output_tokens") && error.contains("openai_chat_completion"),
            "{error}"
        );
    }

    /// A `GET` control request never carries a body, and a form-encoded response's paths are
    /// single-member chains.
    ///
    /// # Panics
    ///
    /// Panics if a shape violation validates.
    #[test]
    fn rejects_control_request_and_form_shape_violations() {
        let get_with_body = mutate_and_validate(good_copilot_json(), |payload| {
            payload["discovery"]["request"]["method"] = serde_json::json!("GET");
            payload["discovery"]["request"]["body"] = serde_json::json!({
                "kind": "form",
                "fields": [{"name": "a", "value": literal("b")}]
            });
        });

        assert!(
            get_with_body
                .unwrap_err()
                .contains("GET control request never carries a body")
        );

        let nested_form_path = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["response"]["fields"][0]["path"] =
                serde_json::json!(["a", "b"]);
        });

        assert!(nested_form_path.unwrap_err().contains("exactly one member"));
    }

    /// The discovery extraction and pagination shapes reject empty member paths, empty request
    /// parameters, and an empty strip prefix.
    ///
    /// # Panics
    ///
    /// Panics if a shape violation validates.
    #[test]
    fn rejects_discovery_shape_violations() {
        let empty_array_at = mutate_and_validate(good_copilot_json(), |payload| {
            payload["discovery"]["extract"]["array_at"] = serde_json::json!([]);
        });

        // An empty array path is the root itself, which the vocabulary permits.
        empty_array_at.unwrap();

        let empty_id_field = mutate_and_validate(good_copilot_json(), |payload| {
            payload["discovery"]["extract"]["id_field"] = serde_json::json!([]);
        });

        assert!(
            empty_id_field
                .unwrap_err()
                .contains("identifier field path")
        );

        let empty_request_param = mutate_and_validate(good_copilot_json(), |payload| {
            payload["discovery"]["pagination"]["request_param"] = serde_json::json!("");
        });

        assert!(
            empty_request_param
                .unwrap_err()
                .contains("request parameter")
        );

        let empty_strip_prefix = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["discovery"]["extract"] = serde_json::json!({
              "mode": "array",
              "array_at": [
                "models"
              ],
              "id_field": [
                "name"
              ],
              "strip_prefix": ""
            });
        });

        assert!(empty_strip_prefix.unwrap_err().contains("prefix"));
    }

    /// Header rows must carry valid HTTP header names: a stored invalid name would fail every
    /// dispatch at runtime — misclassified as an upstream-unreachable control failure or an
    /// internal inference error — so the write boundary rejects it naming the row. Query and form
    /// rows are percent-encoded names and stay exempt from the header rule.
    ///
    /// # Panics
    ///
    /// Panics if an invalid header name validates or a valid one is rejected.
    #[test]
    fn rejects_invalid_header_row_names_but_encoded_query_rows() {
        // The inference path: a trailing space is not a header-name token character.
        let trailing_space = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["inference"]["headers"][1]["name"] = serde_json::json!("User-Agent ");
        });

        let error = trailing_space.unwrap_err();

        assert!(
            error.contains("inference header") && error.contains("User-Agent"),
            "{error}"
        );

        // The control path: the device-authorization request's header rows obey the same rule.
        let control_trailing_space = mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["request"]["headers"][0]["name"] =
                serde_json::json!("Accept ");
        });

        assert!(
            control_trailing_space
                .unwrap_err()
                .contains("control request header")
        );

        // A valid `Authorization` name stays accepted on the row the invalid one failed on.
        mutate_and_validate(good_copilot_json(), |payload| {
            payload["credential"]["grant"]["authorization"]["request"]["headers"][0]["name"] =
                serde_json::json!("Authorization");
        })
        .unwrap();

        // Query rows are percent-encoded, not wire header names: characters that would fail the
        // header rule stay accepted there.
        mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["request"]["query"] = serde_json::json!([
              { "name": "code verifier", "value": [{ "type": "literal", "value": "x" }] }
            ]);
        })
        .unwrap();
    }

    /// The wire surface is strict: unknown fields are rejected at every level of the payload, an
    /// unknown value-expression kind names the closed set, and the no-refresh mode must be the
    /// tagged `{"type": "none"}` object rather than a bare string.
    ///
    /// # Panics
    ///
    /// Panics if a non-strict document parses.
    #[test]
    fn parses_the_payload_strictly() {
        // An unknown field at the payload root.
        let mut payload = good_antigravity_json();
        payload["bogus"] = serde_json::json!(1);

        assert!(serde_json::from_value::<DefinitionPayload>(payload).is_err());

        // An unknown value-expression kind.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["inference"]["headers"][1]["value"] = serde_json::json!([
              {
                "type": "template",
                "value": "x"
              }
            ]);
        });

        assert!(outcome.is_err());

        // An unknown response target.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["exchange"]["response"]["fields"][0]["target"] = json!({
              "type": "identity"
            });
        });

        assert!(outcome.is_err());

        // A bare-string no-refresh mode.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"] = serde_json::json!("none");
        });

        assert!(outcome.is_err());

        // The tagged no-refresh mode.
        let outcome = mutate_and_validate(good_antigravity_json(), |payload| {
            payload["credential"]["refresh"] = serde_json::json!({
              "type": "none"
            });
        });

        outcome.unwrap();
    }

    /// The frontend preset catalog ships unverified product presets whose
    /// descriptor shapes must pass the gateway's write-time validation exactly
    /// as their operators will submit them (README §5.6). These mirror the
    /// catalog's Claude Code, Kimi Code, Devin, and OpenCode Zen payloads —
    /// the Copilot shape is covered by the reprovision case above and the
    /// Codex/Antigravity shapes by the fixtures here.
    ///
    /// # Panics
    ///
    /// Panics if a catalog shape fails validation, which would break every
    /// operator submitting that preset.
    #[test]
    fn accepts_the_unverified_catalog_preset_shapes() {
        // Claude Code: PKCE against the hosted paste-back callback, JSON
        // token requests, the beta header on inference and discovery, and
        // cursor-paginated model discovery.
        #[expect(
            clippy::unwrap_used,
            reason = "the catalog shape is constructed to validate"
        )]
        validate_json(serde_json::json!({
            "format": "anthropic_messages",
            "inference": {
                "url": [
                    {
                        "type": "literal",
                        "value": "https://api.anthropic.com/v1/messages"
                    }
                ],
                "headers": [
                    {
                        "name": "Authorization",
                        "value": [
                            {
                                "type": "literal",
                                "value": "Bearer "
                            },
                            {
                                "type": "secret"
                            }
                        ]
                    },
                    {
                        "name": "anthropic-beta",
                        "value": [
                            {
                                "type": "literal",
                                "value": "oauth-2025-04-20"
                            }
                        ]
                    },
                    {
                        "name": "anthropic-version",
                        "value": [
                            {
                                "type": "literal",
                                "value": "2023-06-01"
                            }
                        ]
                    }
                ],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "9d1c250a-e61b-44e9-88ed-594feddb3c84",
                "grant": {
                    "type": "authorization_code",
                    "authorize_url": [
                        {
                            "type": "literal",
                            "value": "https://claude.ai/oauth/authorize"
                        }
                    ],
                    "redirect_uri": "https://console.anthropic.com/oauth/code/callback",
                    "pkce": "s256",
                    "extra_params": [
                        {
                            "name": "code",
                            "value": [
                                {
                                    "type": "literal",
                                    "value": "true"
                                }
                            ]
                        },
                        {
                            "name": "scope",
                            "value": [
                                {
                                    "type": "literal",
                                    "value": "org:create_api_key user:profile user:inference"
                                }
                            ]
                        }
                    ]
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [
                            {
                                "type": "literal",
                                "value": "https://console.anthropic.com/v1/oauth/token"
                            }
                        ],
                        "body": {
                            "kind": "json",
                            "node": {
                                "type": "object",
                                "fields": {
                                    "grant_type": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "literal",
                                                "value": "authorization_code"
                                            }
                                        ]
                                    },
                                    "client_id": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "client_id"
                                            }
                                        ]
                                    },
                                    "code": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "authorization_code"
                                            }
                                        ]
                                    },
                                    "code_verifier": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "code_verifier"
                                            }
                                        ]
                                    },
                                    "redirect_uri": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "redirect_uri"
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    },
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": [
                                    "access_token"
                                ],
                                "target": {
                                    "type": "access_token"
                                },
                                "required": true
                            },
                            {
                                "path": [
                                    "refresh_token"
                                ],
                                "target": {
                                    "type": "refresh_token"
                                },
                                "required": true
                            },
                            {
                                "path": [
                                    "expires_in"
                                ],
                                "target": {
                                    "type": "expires_in_seconds"
                                },
                                "required": false
                            }
                        ]
                    },
                    "terminal_codes": [
                        "invalid_grant",
                        "expired_token",
                        "invalid_client",
                        "unauthorized_client"
                    ]
                },
                "refresh": {
                    "type": "grant",
                    "request": {
                        "method": "POST",
                        "url": [
                            {
                                "type": "literal",
                                "value": "https://console.anthropic.com/v1/oauth/token"
                            }
                        ],
                        "body": {
                            "kind": "json",
                            "node": {
                                "type": "object",
                                "fields": {
                                    "grant_type": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "literal",
                                                "value": "refresh_token"
                                            }
                                        ]
                                    },
                                    "client_id": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "client_id"
                                            }
                                        ]
                                    },
                                    "refresh_token": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "refresh_token"
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    },
                    "terminal_codes": [
                        "invalid_grant",
                        "expired_token"
                    ]
                }
            },
            "discovery": {
                "request": {
                    "method": "GET",
                    "url": [
                        {
                            "type": "literal",
                            "value": "https://api.anthropic.com/v1/models"
                        }
                    ],
                    "headers": [
                        {
                            "name": "Authorization",
                            "value": [
                                {
                                    "type": "literal",
                                    "value": "Bearer "
                                },
                                {
                                    "type": "secret"
                                }
                            ]
                        },
                        {
                            "name": "anthropic-beta",
                            "value": [
                                {
                                    "type": "literal",
                                    "value": "oauth-2025-04-20"
                                }
                            ]
                        }
                    ]
                },
                "extract": {
                    "mode": "array",
                    "array_at": [
                        "data"
                    ],
                    "id_field": [
                        "id"
                    ]
                },
                "pagination": {
                    "type": "cursor",
                    "has_more_path": [
                        "has_more"
                    ],
                    "last_id_path": [
                        "last_id"
                    ],
                    "request_param": "after_id",
                    "max_pages": 10
                }
            }
        }))
        .unwrap();

        // Kimi Code: a device flow whose device-id slot rides the X-Msh-*
        // headers on authorization, polling, and refresh alike.
        #[expect(
            clippy::unwrap_used,
            reason = "the catalog shape is constructed to validate"
        )]
        validate_json(serde_json::json!({
            "format": "openai_chat_completion",
            "inference": {
                "url": [
                    {
                        "type": "literal",
                        "value": "https://api.kimi.com/coding/v1"
                    },
                    {
                        "type": "literal",
                        "value": "/chat/completions"
                    }
                ],
                "headers": [
                    {
                        "name": "Authorization",
                        "value": [
                            {
                                "type": "literal",
                                "value": "Bearer "
                            },
                            {
                                "type": "secret"
                            }
                        ]
                    },
                    {
                        "name": "X-Msh-Device-Id",
                        "value": [
                            {
                                "type": "slot",
                                "name": "device_id"
                            }
                        ]
                    },
                    {
                        "name": "X-Msh-Platform",
                        "value": [
                            {
                                "type": "literal",
                                "value": "macos"
                            }
                        ]
                    },
                    {
                        "name": "User-Agent",
                        "value": [
                            {
                                "type": "literal",
                                "value": "KimiCLI/1.0.0"
                            }
                        ]
                    }
                ],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "17e5f67c-37e0-44de-a1c1-33a124c8d3f5",
                "initial_slots": [
                    {
                        "name": "device_id",
                        "generator": "uuid_hex"
                    }
                ],
                "grant": {
                    "type": "device",
                    "authorization": {
                        "request": {
                            "method": "POST",
                            "url": [
                                {
                                    "type": "literal",
                                    "value": "https://auth.kimi.com/oauth/device_authorization"
                                }
                            ],
                            "headers": [
                                {
                                    "name": "X-Msh-Device-Id",
                                    "value": [
                                        {
                                            "type": "slot",
                                            "name": "device_id"
                                        }
                                    ]
                                }
                            ],
                            "body": {
                                "kind": "form",
                                "fields": [
                                    {
                                        "name": "client_id",
                                        "value": [
                                            {
                                                "type": "client_id"
                                            }
                                        ]
                                    }
                                ]
                            }
                        },
                        "response": {
                            "encoding": "json",
                            "fields": [
                                {
                                    "path": [
                                        "device_code"
                                    ],
                                    "target": {
                                        "type": "device_code"
                                    },
                                    "required": true
                                },
                                {
                                    "path": [
                                        "user_code"
                                    ],
                                    "target": {
                                        "type": "user_code"
                                    },
                                    "required": true
                                },
                                {
                                    "path": [
                                        "verification_uri_complete"
                                    ],
                                    "target": {
                                        "type": "verification_uri_complete"
                                    },
                                    "required": false
                                },
                                {
                                    "path": [
                                        "verification_uri"
                                    ],
                                    "target": {
                                        "type": "verification_uri"
                                    },
                                    "required": false
                                },
                                {
                                    "path": [
                                        "expires_in"
                                    ],
                                    "target": {
                                        "type": "expires_in_seconds"
                                    },
                                    "required": false
                                },
                                {
                                    "path": [
                                        "interval"
                                    ],
                                    "target": {
                                        "type": "interval_seconds"
                                    },
                                    "required": false
                                }
                            ]
                        }
                    },
                    "expires_in_fallback_seconds": 900
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [
                            {
                                "type": "literal",
                                "value": "https://auth.kimi.com/oauth/token"
                            }
                        ],
                        "headers": [
                            {
                                "name": "X-Msh-Device-Id",
                                "value": [
                                    {
                                        "type": "slot",
                                        "name": "device_id"
                                    }
                                ]
                            }
                        ],
                        "body": {
                            "kind": "form",
                            "fields": [
                                {
                                    "name": "grant_type",
                                    "value": [
                                        {
                                            "type": "literal",
                                            "value": "urn:ietf:params:oauth:grant-type:device_code"
                                        }
                                    ]
                                },
                                {
                                    "name": "client_id",
                                    "value": [
                                        {
                                            "type": "client_id"
                                        }
                                    ]
                                },
                                {
                                    "name": "device_code",
                                    "value": [
                                        {
                                            "type": "device_code"
                                        }
                                    ]
                                }
                            ]
                        }
                    },
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": [
                                    "access_token"
                                ],
                                "target": {
                                    "type": "access_token"
                                },
                                "required": true
                            },
                            {
                                "path": [
                                    "refresh_token"
                                ],
                                "target": {
                                    "type": "refresh_token"
                                },
                                "required": true
                            },
                            {
                                "path": [
                                    "expires_in"
                                ],
                                "target": {
                                    "type": "expires_in_seconds"
                                },
                                "required": false
                            }
                        ]
                    },
                    "terminal_codes": [
                        "expired_token",
                        "access_denied"
                    ]
                },
                "refresh": {
                    "type": "grant",
                    "request": {
                        "method": "POST",
                        "url": [
                            {
                                "type": "literal",
                                "value": "https://auth.kimi.com/oauth/token"
                            }
                        ],
                        "headers": [
                            {
                                "name": "X-Msh-Device-Id",
                                "value": [
                                    {
                                        "type": "slot",
                                        "name": "device_id"
                                    }
                                ]
                            }
                        ],
                        "body": {
                            "kind": "form",
                            "fields": [
                                {
                                    "name": "grant_type",
                                    "value": [
                                        {
                                            "type": "literal",
                                            "value": "refresh_token"
                                        }
                                    ]
                                },
                                {
                                    "name": "refresh_token",
                                    "value": [
                                        {
                                            "type": "refresh_token"
                                        }
                                    ]
                                },
                                {
                                    "name": "client_id",
                                    "value": [
                                        {
                                            "type": "client_id"
                                        }
                                    ]
                                }
                            ]
                        }
                    },
                    "terminal_codes": [
                        "invalid_grant"
                    ]
                }
            },
            "discovery": {
                "request": {
                    "method": "GET",
                    "url": [
                        {
                            "type": "literal",
                            "value": "https://api.kimi.com/coding/v1/models"
                        }
                    ],
                    "headers": [
                        {
                            "name": "Authorization",
                            "value": [
                                {
                                    "type": "literal",
                                    "value": "Bearer "
                                },
                                {
                                    "type": "secret"
                                }
                            ]
                        }
                    ]
                },
                "extract": {
                    "mode": "array",
                    "array_at": [
                        "data"
                    ],
                    "id_field": [
                        "id"
                    ]
                },
                "pagination": {
                    "type": "none"
                }
            }
        }))
        .unwrap();

        // Devin: a ConnectRPC exchange answering a static key and server URL
        // — no refresh, no discovery (manual models only).
        #[expect(
            clippy::unwrap_used,
            reason = "the catalog shape is constructed to validate"
        )]
        validate_json(serde_json::json!({
            "format": "anthropic_messages",
            "inference": {
                "url": [
                    {
                        "type": "base"
                    },
                    {
                        "type": "literal",
                        "value": "/v1/messages"
                    }
                ],
                "headers": [
                    {
                        "name": "Authorization",
                        "value": [
                            {
                                "type": "literal",
                                "value": "Bearer "
                            },
                            {
                                "type": "secret"
                            }
                        ]
                    },
                    {
                        "name": "anthropic-version",
                        "value": [
                            {
                                "type": "literal",
                                "value": "2023-06-01"
                            }
                        ]
                    }
                ],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "oauth",
                "client_id": "devin-oauth-client",
                "grant": {
                    "type": "authorization_code",
                    "authorize_url": [
                        {
                            "type": "literal",
                            "value": "https://app.devin.ai/oauth/authorize"
                        }
                    ],
                    "redirect_uri": "http://localhost:1455/auth/callback",
                    "pkce": "s256",
                    "extra_params": []
                },
                "exchange": {
                    "request": {
                        "method": "POST",
                        "url": [
                            {
                                "type": "literal",
                                "value": "https://api.devin.ai/v1/oauth/exchange_token"
                            }
                        ],
                        "headers": [
                            {
                                "name": "connect-protocol-version",
                                "value": [
                                    {
                                        "type": "literal",
                                        "value": "1"
                                    }
                                ]
                            }
                        ],
                        "body": {
                            "kind": "json",
                            "node": {
                                "type": "object",
                                "fields": {
                                    "grantType": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "literal",
                                                "value": "authorization_code"
                                            }
                                        ]
                                    },
                                    "clientId": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "client_id"
                                            }
                                        ]
                                    },
                                    "code": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "authorization_code"
                                            }
                                        ]
                                    },
                                    "codeVerifier": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "code_verifier"
                                            }
                                        ]
                                    },
                                    "redirectUri": {
                                        "type": "string",
                                        "expr": [
                                            {
                                                "type": "redirect_uri"
                                            }
                                        ]
                                    }
                                }
                            }
                        }
                    },
                    "response": {
                        "encoding": "json",
                        "fields": [
                            {
                                "path": [
                                    "api_key"
                                ],
                                "target": {
                                    "type": "access_token"
                                },
                                "required": true
                            },
                            {
                                "path": [
                                    "api_server_url"
                                ],
                                "target": {
                                    "type": "base_url"
                                },
                                "required": true
                            }
                        ]
                    },
                    "terminal_codes": [
                        "invalid_grant",
                        "unauthenticated"
                    ]
                },
                "refresh": {
                    "type": "none"
                }
            }
        }))
        .unwrap();

        // OpenCode Zen: a static-key definition with the plain ID-list
        // discovery; the Gemini variant also carries the model-in-URL actions.
        #[expect(
            clippy::unwrap_used,
            reason = "the catalog shape is constructed to validate"
        )]
        validate_json(serde_json::json!({
            "format": "gemini_generate_content",
            "inference": {
                "url": [
                    {
                        "type": "literal",
                        "value": "https://opencode.ai/zen/v1"
                    },
                    {
                        "type": "model"
                    },
                    {
                        "type": "literal",
                        "value": ":generateContent"
                    }
                ],
                "stream_url": [
                    {
                        "type": "literal",
                        "value": "https://opencode.ai/zen/v1"
                    },
                    {
                        "type": "model"
                    },
                    {
                        "type": "literal",
                        "value": ":streamGenerateContent?alt=sse"
                    }
                ],
                "headers": [
                    {
                        "name": "x-goog-api-key",
                        "value": [
                            {
                                "type": "secret"
                            }
                        ]
                    }
                ],
                "upstream_streaming": "match_client"
            },
            "credential": {
                "type": "api_key"
            },
            "discovery": {
                "request": {
                    "method": "GET",
                    "url": [
                        {
                            "type": "literal",
                            "value": "https://opencode.ai/zen/v1/models"
                        }
                    ],
                    "headers": [
                        {
                            "name": "x-goog-api-key",
                            "value": [
                                {
                                    "type": "secret"
                                }
                            ]
                        }
                    ]
                },
                "extract": {
                    "mode": "array",
                    "array_at": [
                        "data"
                    ],
                    "id_field": [
                        "id"
                    ]
                },
                "pagination": {
                    "type": "none"
                }
            }
        }))
        .unwrap();
    }

    /// The per-format render-option table is the single home of the codec sets: today only the
    /// OpenAI Responses format carries an option.
    ///
    /// # Panics
    ///
    /// Panics if a format carries an undocumented option set.
    #[test]
    fn pins_the_render_option_sets() {
        assert_eq!(
            allowed_render_options(UpstreamFormat::OpenAiResponse),
            [RenderOption::OmitMaxOutputTokens]
        );

        for format in [
            UpstreamFormat::OpenAiChatCompletion,
            UpstreamFormat::GeminiGenerateContent,
            UpstreamFormat::GeminiInteraction,
            UpstreamFormat::AnthropicMessages,
        ] {
            assert!(
                allowed_render_options(format).is_empty(),
                "unexpected options for {}",
                format.as_str()
            );
        }
    }

    /// The interpreter contract the write-time validation feeds: a literal string is a one-part
    /// expression, and an inference URL composes base, literals, and the routed model.
    ///
    /// # Panics
    ///
    /// Panics if a fixture does not parse.
    #[test]
    fn parses_literal_and_composite_expressions() {
        let payload = serde_json::from_value::<DefinitionPayload>(good_antigravity_json()).unwrap();

        // The inference URL of the fixture is base + literal.
        assert_eq!(
            payload.inference.url.0.len(),
            2,
            "the fixture URL carries base and literal parts"
        );

        // A one-part literal parses into the same expression `StringExpr::literal` builds.
        let parsed: StringExpr = serde_json::from_value(literal("Bearer ")).unwrap();

        assert_eq!(parsed, StringExpr::literal("Bearer "));
    }
}
