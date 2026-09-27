//! The shared descriptor vocabulary: string expressions, JSON template nodes, control requests,
//! and response descriptors.
//!
//! Every templated string in a descriptor — a URL, a header value, a query or form value, a JSON
//! string leaf — is one [`StringExpr`]: an ordered sequence of [`ValueExpr`] parts concatenated
//! into a single string. There is no separate URL placeholder parser and no separate
//! string-template language; URL assembly, HTTP header construction, and JSON string rendering are
//! destination-specific consumers of the one evaluated expression.
//!
//! Control requests — the OAuth device authorization, token exchange, refresh, provisioning,
//! polling, and discovery requests — share one [`ControlRequest`] shape, and their responses share
//! one [`ResponseDescriptor`] decoding shape. The vocabulary is deliberately closed: growing it (a
//! new token, a new render option, a new extraction shape) is the documented extension path, never
//! a provider branch.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::BTreeMap;
/// One ordered part of a string expression (README §5.1).
///
/// The serde and TypeScript names are the descriptor wire names; every expression part is a
/// `{"type": …}`-tagged object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueExpr {
    /// A constant string.
    Literal {
        /// The constant text.
        value: String,
    },

    /// The API key, or the flow's or provider's current OAuth access token.
    Secret,

    /// A working flow slot, or a stored credential slot once the provider exists.
    Slot {
        /// The slot's name.
        name: String,
    },

    /// The definition's configured OAuth client ID.
    ClientId,

    /// The definition's configured OAuth client secret.
    ClientSecret,

    /// The authorization code of the completion currently being exchanged.
    AuthorizationCode,

    /// The PKCE code verifier minted when the flow began.
    CodeVerifier,

    /// The definition's configured OAuth redirect URI.
    RedirectUri,

    /// The random state token identifying the flow.
    #[serde(rename = "oauth_state")]
    OAuthState,

    /// The device code issued by the device authorization response.
    DeviceCode,

    /// The stored refresh token of the credential being renewed.
    RefreshToken,

    /// One fresh UUID per upstream inference dispatch, reused by every reference in that dispatch.
    RequestUuid,

    /// The routed model's upstream identifier.
    Model,

    /// The working flow base URL, or the stored provider base URL after creation.
    Base,

    /// The first non-empty result of the carried parts; every part empty renders empty.
    ///
    /// `Coalesce` is the only compound value part: no conditionals, no arithmetic, and no nesting
    /// beyond its arguments — a `Coalesce` argument may not itself be a `Coalesce`.
    Coalesce {
        /// The parts tried in order.
        parts: Vec<ValueExpr>,
    },
}

/// The value-expression kinds a descriptor stage may reference. The management write boundary
/// rejects an expression referencing a kind its stage does not carry (README §5.1), so runtime
/// evaluation of an absent value is stored drift and fails fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ValueKind {
    /// A constant string.
    Literal,

    /// The current API key or OAuth access token.
    Secret,

    /// A named slot.
    Slot,

    /// The configured OAuth client ID.
    ClientId,

    /// The configured OAuth client secret.
    ClientSecret,

    /// The authorization code being exchanged.
    AuthorizationCode,

    /// The PKCE code verifier.
    CodeVerifier,

    /// The configured redirect URI.
    RedirectUri,

    /// The flow's state token.
    OAuthState,

    /// The device code of the flow.
    DeviceCode,

    /// The stored refresh token.
    RefreshToken,

    /// The per-dispatch UUID.
    RequestUuid,

    /// The routed model's upstream identifier.
    Model,

    /// The working or stored base URL.
    Base,
}

impl ValueExpr {
    /// The kind of this expression part; a `Coalesce` is decomposed and its parts are walked by
    /// the caller, so this reports the kinds of the direct expression forms only.
    pub(crate) fn kind(&self) -> ValueKind {
        match self {
            Self::Literal { .. } => ValueKind::Literal,
            Self::Secret => ValueKind::Secret,
            Self::Slot { .. } => ValueKind::Slot,
            Self::ClientId => ValueKind::ClientId,
            Self::ClientSecret => ValueKind::ClientSecret,
            Self::AuthorizationCode => ValueKind::AuthorizationCode,
            Self::CodeVerifier => ValueKind::CodeVerifier,
            Self::RedirectUri => ValueKind::RedirectUri,
            Self::OAuthState => ValueKind::OAuthState,
            Self::DeviceCode => ValueKind::DeviceCode,
            Self::RefreshToken => ValueKind::RefreshToken,
            Self::RequestUuid => ValueKind::RequestUuid,
            Self::Model => ValueKind::Model,
            Self::Base => ValueKind::Base,

            // The walk decomposes a `Coalesce` before consulting kinds, so this arm is only a
            // completeness pin.
            Self::Coalesce { .. } => ValueKind::Literal,
        }
    }
}

/// An ordered sequence of [`ValueExpr`] parts concatenated into one string (README §5.1).
///
/// The empty sequence is the empty string; a literal string is the one-part expression carrying a
/// single [`ValueExpr::Literal`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct StringExpr(
    /// The ordered parts.
    pub Vec<ValueExpr>,
);

impl StringExpr {
    /// Builds the one-part literal expression of `text`.
    #[must_use]
    pub fn literal(text: impl Into<String>) -> Self {
        Self(vec![ValueExpr::Literal { value: text.into() }])
    }

    /// Builds the expression carrying the single value part.
    #[cfg(test)]
    #[must_use]
    pub fn single(part: ValueExpr) -> Self {
        Self(vec![part])
    }
}

/// A name-value row of one templated HTTP header, query parameter, or form field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ParamRow {
    /// The header, query, or form field name.
    pub name: String,

    /// The evaluated value.
    pub value: StringExpr,
}

/// A typed JSON template tree: the shape of a control request's JSON body and of an inference
/// envelope (README §5.1).
///
/// [`TemplateNode::Body`] is the codec-rendered upstream body. It is the one node whose content the
/// engine supplies, so a control request — whose body is fully described — never carries it, and an
/// inference envelope carries it exactly once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TemplateNode {
    /// A literal JSON leaf of any shape — a string, number, boolean, or `null` — rendered
    /// verbatim. The carried value's shape is the operator's to define, so it is modeled as an
    /// opaque JSON value.
    Json {
        /// The verbatim JSON value.
        value: serde_json::Value,
    },

    /// A JSON string leaf whose content is one evaluated string expression.
    String {
        /// The evaluated string content.
        expr: StringExpr,
    },

    /// The codec-rendered upstream inference body.
    Body,

    /// A JSON object whose members render in the carried order.
    Object {
        /// The members, keyed by name.
        fields: BTreeMap<String, TemplateNode>,
    },

    /// A JSON array whose entries render in the carried order.
    Array {
        /// The entries.
        items: Vec<TemplateNode>,
    },
}

/// The HTTP method a control request declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "UPPERCASE")]
pub enum ControlMethod {
    /// An HTTP `GET` request; a control request with this method carries no body.
    Get,

    /// An HTTP `POST` request.
    Post,
}

/// The body of a control request (README §5.1): a form-encoded field set or a typed JSON tree.
///
/// The executor sets the request's `Content-Type` from the carried variant; every other header,
/// `Accept` included, is an explicit row of the request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlBody {
    /// A `application/x-www-form-urlencoded` body of evaluated name-value fields.
    Form {
        /// The evaluated fields, rendered in the carried order.
        fields: Vec<ParamRow>,
    },

    /// A JSON body rendered from the typed template tree. The tree never carries
    /// [`TemplateNode::Body`]: a control request's body is fully described by the definition.
    Json {
        /// The template tree rendered as the body.
        node: TemplateNode,
    },
}

/// One shared HTTP request description for the OAuth device authorization, token exchange,
/// refresh, provisioning, polling, and discovery stages (README §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    /// The request method.
    pub method: ControlMethod,

    /// The evaluated request URL.
    pub url: StringExpr,

    /// The evaluated request headers, rendered in the carried order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ParamRow>,

    /// The evaluated query parameters, appended to the URL in the carried order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<ParamRow>,

    /// The request body; a `GET` request never carries one, and a `POST` without a body is the
    /// bodyless form some endpoints require.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<ControlBody>,
}

/// The wire encoding a control response arrives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ResponseEncoding {
    /// A JSON document.
    Json,

    /// A `application/x-www-form-urlencoded` body of flat string fields.
    Form,
}

/// A response path: the closed mini-vocabulary addressing one value of a decoded control response
/// (README §5.1) — an object member chain, rendered as the ordered member names, with the empty
/// chain denoting the root.
///
/// Arrays are not addressed by value paths: discovery locates its array and per-entry field
/// explicitly through its own extraction shape, and form-encoded responses are flat, so a path
/// into them carries a single member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(transparent)]
pub struct ResponsePath(
    /// The ordered member names.
    pub Vec<String>,
);

impl ResponsePath {
    /// Resolves the member chain into the JSON value it addresses: the empty chain is the root,
    /// and a missing member or a non-object intermediate value yields [`None`] (README §5.1).
    /// Arrays are not addressed by value paths, so an array met mid-chain is a `None` like any
    /// other non-object.
    #[must_use]
    pub(crate) fn resolve<'a>(&self, json: &'a Json) -> Option<&'a Json> {
        let mut value = json;

        for member in &self.0 {
            value = value.get(member)?;
        }

        Some(value)
    }
}

/// The declared shape of one extracted response value (README §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtractionShape {
    /// The value is a JSON string, or a JSON number rendered as its decimal text.
    String,

    /// The value must be a JSON integer.
    Integer,

    /// The value may be a bare string or an object carrying the named string member — the bare
    /// string and `{field}`-object polymorphism some upstreams render for one value.
    StringOrField {
        /// The object member carrying the string form.
        field: String,
    },
}

/// One name-to-value mapping of a decoded control response: the value at `path` is stored into the
/// stage's `target` when present. A missing required path fails the stage; a missing optional path
/// leaves its target unchanged; a present value of the wrong declared shape always fails
/// (README §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ResponseField {
    /// The closed path of the response value.
    pub path: ResponsePath,

    /// Where the extracted value is stored.
    pub target: ResponseTarget,

    /// Whether a missing path fails the stage (`true`) or leaves the target unchanged (`false`).
    pub required: bool,
}

/// The declared encoding and field mappings of one control response (README §5.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ResponseDescriptor {
    /// The wire encoding the response arrives in.
    pub encoding: ResponseEncoding,

    /// The name-to-value mappings applied to the decoded response.
    pub fields: Vec<ResponseField>,
}

/// The merged working-state targets of a decoded control response: one closed vocabulary shared
/// by every stage, whose per-stage subsets the write-time validation pins (README §5.3, §5.4).
/// A stage's mapping may only carry the targets its stage produces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResponseTarget {
    /// The access token; may be supplied by a later provisioning step instead, but must exist
    /// before the provider write.
    AccessToken,

    /// The refresh token, when the upstream issues one.
    RefreshToken,

    /// The access token's lifetime in seconds.
    ExpiresInSeconds,

    /// The identity token whose claims feed the definition's claim mappings.
    IdToken,

    /// The upstream base URL the response supplies.
    BaseUrl,

    /// A named working slot of the flow.
    Slot {
        /// The slot's name.
        name: String,
    },

    /// The device code the polling exchange presents (device authorization only).
    DeviceCode,

    /// The human-readable code the administrator enters at the verification URI (device
    /// authorization only).
    UserCode,

    /// The URI the administrator opens to approve the flow (device authorization only).
    VerificationUri,

    /// The verification URI carrying the user code, ready to open (device authorization only).
    VerificationUriComplete,

    /// The polling interval in seconds the endpoint requires (device authorization only).
    IntervalSeconds,

    /// The access token's absolute expiry, rendered by the upstream in Unix seconds
    /// (provisioning only).
    ExpiresAtUnixSeconds,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One path resolution, panicking on a `None` the fixture guarantees.
    ///
    /// # Panics
    ///
    /// Panics when the path does not resolve.
    fn resolved<'a>(path: &'a ResponsePath, json: &'a Json) -> &'a Json {
        #[expect(clippy::unwrap_used, reason = "the fixture path always resolves")]
        path.resolve(json).unwrap()
    }

    /// `resolve` walks the member chain with the empty chain as the root, and any missing
    /// member or non-object intermediate yields `None` (README §5.1).
    ///
    /// # Panics
    ///
    /// Panics when a fixture path does not resolve.
    #[test]
    fn resolves_member_chains() {
        let json = serde_json::json!({ "a": { "b": { "c": "leaf" } } });

        let root = ResponsePath::default();

        assert_eq!(resolved(&root, &json), &json);

        let leaf = ResponsePath(vec!["a".to_string(), "b".to_string(), "c".to_string()]);

        assert_eq!(resolved(&leaf, &json), &serde_json::json!("leaf"));

        let missing = ResponsePath(vec!["a".to_string(), "absent".to_string()]);

        assert_eq!(missing.resolve(&json), None);

        // An intermediate value that is not an object — a string here — addresses nothing.
        let flat = serde_json::json!({ "a": "text" });

        assert_eq!(leaf.resolve(&flat), None);
    }
}
