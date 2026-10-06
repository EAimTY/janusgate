//! The credential-acquisition descriptor: how a provider definition acquires and renews the
//! credentials its instances carry (README §5.3).
//!
//! A definition either describes a static API key — whose secret the instance carries outright —
//! or a full OAuth recipe: the authorization grant, the token exchange, the identity-claim
//! extraction, and the refresh mode. No actual API key is ever stored on a provider definition.

use super::template::{ControlRequest, ParamRow, ResponseDescriptor, ResponsePath, StringExpr};
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use vec1::Vec1;
/// The credential acquisition a provider definition describes (README §5.3): the `type`
/// discriminator is the only stored credential-kind representation for the definition, and it must
/// match the provider row's credential kind at the management boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialAcquisition {
    /// A static API key the instance carries; no secret and no OAuth settings live on the
    /// definition.
    ApiKey,

    /// An OAuth account: the definition carries the full recipe the gateway runs to acquire and
    /// renew the instance's tokens.
    #[serde(rename = "oauth")]
    OAuth(
        /// The recipe, boxed to keep the tiny static-key variant cheap to move.
        Box<OAuthRecipe>,
    ),
}

/// The OAuth recipe of one provider definition (README §5.3): the authorization grant, the token
/// exchange, the identity-claim extraction, and the refresh mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct OAuthRecipe {
    /// The OAuth client identifier presented at authorization, exchange, and refresh.
    pub client_id: String,

    /// The OAuth client secret, when the grant authenticates with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,

    /// The lifetime of the access token in seconds, configured only when the upstream contract
    /// guarantees it; the expiry fallback after a reported `expires_in` and an `exp` claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token_lifetime_seconds: Option<NonZeroU64>,

    /// The durable slots initialized once for a new flow, before its first external request;
    /// reauthorization starts from the target provider's stored slots and initializes only the
    /// missing names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_slots: Vec<SlotInitializer>,

    /// The authorization grant the flow runs.
    pub grant: Grant,

    /// The token exchange redeeming the grant's product.
    pub exchange: Exchange,

    /// The JWT payload claims copied into slots at exchange time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub id_token_claims: Vec<IdTokenClaim>,

    /// How an acquired credential renews.
    pub refresh: TokenRefresh,
}

impl CredentialAcquisition {
    /// Whether this acquisition describes an OAuth recipe.
    #[must_use]
    pub const fn is_oauth(&self) -> bool {
        matches!(self, Self::OAuth(_))
    }
}

/// One durable slot initialized at flow start (README §5.3): a stable generated value any
/// request stage of the flow can reference — a device ID used again during token polling and
/// refresh, for example.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct SlotInitializer {
    /// The slot's name.
    pub name: String,

    /// The generator producing the slot's value.
    pub generator: SlotGenerator,
}

/// The closed generator vocabulary of a slot initializer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SlotGenerator {
    /// A random UUID v4 in its canonical lowercase hyphenated form.
    Uuid,

    /// A random UUID v4 rendered as 32 lowercase hexadecimal digits.
    UuidHex,
}

/// The OAuth authorization grant a flow runs (README §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Grant {
    /// The OAuth authorization-code grant: the administrator opens the built authorize URL, the
    /// provider redirects back with a single-use code, and the code is pasted back into the
    /// management API.
    AuthorizationCode {
        /// The base of the authorize URL the administrator opens; the builder appends the
        /// standard parameters (`response_type`, `client_id`, `redirect_uri`, `state`, and the
        /// PKCE pair when configured) and the extra parameters below.
        authorize_url: StringExpr,

        /// The loopback redirect URI the completion pastes its code from; also available to the
        /// exchange request as the [`ValueExpr::RedirectUri`](super::template::ValueExpr) value.
        redirect_uri: String,

        /// The PKCE configuration of the grant.
        pkce: Pkce,

        /// The additional authorize-URL parameters beyond the standard ones — scopes, provider
        /// flags, markers — rendered in the carried order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        extra_params: Vec<ParamRow>,
    },

    /// the RFC 8628 device-authorization grant: the gateway executes the configured device
    /// authorization request at flow start and polls the token endpoint with the issued device
    /// code until the administrator approves or the flow expires.
    Device {
        /// The device authorization request and its response mapping.
        authorization: DeviceAuthorization,

        /// The flow lifetime fallback in seconds, required exactly when the authorization
        /// response maps no required `expires_in_seconds` target; a reported `expires_in`
        /// always wins.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_in_fallback_seconds: Option<NonZeroU64>,
    },
}

impl Grant {
    /// Whether this grant is the device grant.
    #[must_use]
    pub const fn is_device(&self) -> bool {
        matches!(self, Self::Device { .. })
    }
}

/// The PKCE configuration of an authorization-code grant (RFC 7636).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Pkce {
    /// No PKCE: the grant authenticates without a code verifier — with a client secret, or with a
    /// public client that does not use PKCE.
    None,

    /// PKCE with the `S256` challenge method: the gateway mints the verifier and appends the
    /// challenge to the authorize URL.
    S256,
}

/// The device authorization request of a device grant and the mapping decoding its response
/// (README §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DeviceAuthorization {
    /// The control request executing the device authorization.
    pub request: ControlRequest,

    /// The response mapping decoding the device authorization response.
    pub response: ResponseDescriptor,
}

/// The token exchange redeeming a grant's product for tokens (README §5.3). Under a device
/// grant this same request is the polling request: the engine repeats it per the stored schedule
/// until it succeeds, reports `authorization_pending` or `slow_down`, or fails with a terminal
/// code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Exchange {
    /// The control request executing the exchange.
    pub request: ControlRequest,

    /// The response mapping decoding the exchange response.
    pub response: ResponseDescriptor,

    /// The full list of terminal failure codes — the codes whose retry is futile — compared
    /// case-insensitively; no engine defaults apply. The `Vec1` type rejects an empty list at
    /// parse.
    #[cfg_attr(feature = "management-api-typescript-bindings", ts(as = "Vec<String>"))]
    pub terminal_codes: Vec1<String>,
}

/// One JWT payload claim copied into a slot at exchange time (README §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct IdTokenClaim {
    /// The closed member path of the claim inside the JWT payload.
    pub claim: ResponsePath,

    /// The slot the claim value is stored into.
    pub slot: String,
}

/// How an OAuth-acquired credential renews (README §5.3). Even the no-refresh mode is the
/// tagged `{"type": "none"}` object, never a bare string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TokenRefresh {
    /// A refresh grant against the token endpoint, run under the existing database lease.
    Grant {
        /// The control request executing the refresh grant.
        request: ControlRequest,

        /// The response mapping decoding the refresh response; when absent, the exchange
        /// response's mapping is reused.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<ResponseDescriptor>,

        /// The full list of terminal failure codes, compared case-insensitively; no engine
        /// defaults apply. The `Vec1` type rejects an empty list at parse.
        #[cfg_attr(feature = "management-api-typescript-bindings", ts(as = "Vec<String>"))]
        terminal_codes: Vec1<String>,
    },

    /// Re-run the definition's provisioning sequence under the existing lease — the Copilot
    /// session-token renewal. Requires the definition to carry a non-empty provisioning list.
    Reprovision,

    /// The credential never refreshes: an OAuth-acquired static credential. Time-based refresh is
    /// never scheduled for it.
    None,
}
