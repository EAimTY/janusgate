//! The string-expression interpreter: the one evaluator every templated string in a descriptor
//! runs through (`docs/design/providers.md`).
//!
//! A [`ValueEnv`] carries the values of one stage; evaluating an expression whose stage did not
//! supply a referenced value is stored drift and fails fast — the management write boundary
//! rejected such references at definition time. Slots evaluate to their stored value, or to the
//! empty string when absent: that is what lets a `Coalesce` pick the first slot a provisioning
//! sequence actually placed.

use super::template::{StringExpr, TemplateNode, ValueExpr};
use serde::Serialize;
use serde_json::Value as Json;
use std::collections::BTreeMap;

#[cfg(test)]
mod tests;

/// A rendered provider-authored JSON template, retaining the codec body as a concrete borrow.
#[derive(Serialize)]
#[serde(untagged)]
pub(crate) enum RenderedJson<'a, T> {
    /// Arbitrary operator-authored literal fragment.
    Literal(&'a Json),
    /// Evaluated string expression.
    Text(String),
    /// The codec's concrete request, serialized directly at its placeholder.
    Body(&'a T),
    /// Operator-authored member names and their evaluated values, carried in the canonical
    /// lexicographic name order of [`TemplateNode::Object`](super::template::TemplateNode).
    Object(BTreeMap<&'a str, Self>),
    /// Ordered evaluated template elements.
    Array(Vec<Self>),
}

/// One value the interpreter could not supply: the expression referenced a value its stage did
/// not carry. Write-time validation rejects every such reference, so this is stored drift and
/// never a client fault.
#[derive(Debug, thiserror::Error)]
#[error("{what} references the {kind}, which this stage does not carry")]
pub(crate) struct EvalError {
    /// The human-readable kind name of the missing value.
    kind: &'static str,

    /// What the expression was evaluated for.
    what: String,
}

/// The values of one evaluation stage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueEnv<'a> {
    /// The working flow base URL, or the stored provider base URL after creation.
    base_url: Option<&'a str>,

    /// The API key, or the current OAuth access token.
    secret: Option<&'a str>,

    /// The working flow slots, or the stored credential slots after creation.
    slots: &'a std::collections::BTreeMap<String, String>,

    /// The configured OAuth client ID.
    client_id: Option<&'a str>,

    /// The configured OAuth client secret.
    client_secret: Option<&'a str>,

    /// The authorization code being exchanged.
    authorization_code: Option<&'a str>,

    /// The PKCE code verifier minted at flow start.
    code_verifier: Option<&'a str>,

    /// The configured redirect URI.
    redirect_uri: Option<&'a str>,

    /// The flow's state token.
    oauth_state: Option<&'a str>,

    /// The flow's device code.
    device_code: Option<&'a str>,

    /// The stored refresh token.
    refresh_token: Option<&'a str>,

    /// The request UUID of the current inference dispatch, cached by the dispatcher so every
    /// reference in one dispatch renders the same value.
    request_uuid: Option<&'a str>,

    /// The routed model's upstream identifier.
    model: Option<&'a str>,
}

impl<'a> ValueEnv<'a> {
    /// Builds the stage environment of one inference dispatch (`docs/design/providers.md`): the
    /// stored base URL, the credential secret, the stored slots, the dispatch's request UUID,
    /// and the routed model are exactly the values the stage's expressions may reference.
    #[must_use]
    pub(crate) fn for_inference(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        secret: &'a str,
        request_uuid: &'a str,
        model: &'a str,
    ) -> Self {
        Self {
            slots,
            base_url,
            secret: Some(secret),
            request_uuid: Some(request_uuid),
            model: Some(model),
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of an authorize-URL build (`docs/design/providers.md`): the
    /// configured client credentials and the flow's state token are exactly the values the
    /// stage's expressions may reference — the write-time validation bars the base URL and
    /// slots there. The flow's slots still ride along because the environment always carries a
    /// slot map.
    #[must_use]
    pub(in crate::provider) fn for_authorize_url(
        slots: &'a BTreeMap<String, String>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
        oauth_state: &'a str,
    ) -> Self {
        Self {
            slots,
            client_id: Some(client_id),
            client_secret,
            oauth_state: Some(oauth_state),
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of a device authorization request (`docs/design/providers.md`):
    /// the working base URL, the flow's slots, and the configured client credentials are
    /// exactly the values the stage's expressions may reference.
    #[must_use]
    pub(in crate::provider) fn for_device_authorization(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
    ) -> Self {
        Self {
            slots,
            base_url,
            client_id: Some(client_id),
            client_secret,
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of a token exchange (`docs/design/providers.md`): the working
    /// base URL, the flow's slots, the configured client credentials, and the grant's
    /// authorization code, PKCE verifier, and redirect URI are exactly the values the stage's
    /// expressions may reference.
    #[must_use]
    pub(crate) fn for_exchange(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
        authorization_code: &'a str,
        code_verifier: Option<&'a str>,
        redirect_uri: &'a str,
    ) -> Self {
        Self {
            slots,
            base_url,
            client_id: Some(client_id),
            client_secret,
            authorization_code: Some(authorization_code),
            code_verifier,
            redirect_uri: Some(redirect_uri),
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of one device-flow poll attempt (`docs/design/providers.md`):
    /// the working base URL, the flow's slots, the configured client credentials, and the
    /// flow's device code are exactly the values the stage's expressions may reference. The
    /// completion's provisioning sequence rebuilds its own environment per request through
    /// [`ValueEnv::for_provisioning`], which additionally carries the credential secret.
    #[must_use]
    pub(in crate::provider) fn for_device_poll(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
        device_code: Option<&'a str>,
    ) -> Self {
        Self {
            slots,
            base_url,
            client_id: Some(client_id),
            client_secret,
            device_code,
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of a refresh grant (`docs/design/providers.md`): the working
    /// base URL, the stored slots, the configured client credentials, and the stored refresh
    /// token are exactly the values the stage's expressions may reference.
    #[must_use]
    pub(in crate::provider) fn for_refresh_grant(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
        refresh_token: Option<&'a str>,
    ) -> Self {
        Self {
            slots,
            base_url,
            client_id: Some(client_id),
            client_secret,
            refresh_token,
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of a provisioning step or poll (`docs/design/providers.md`):
    /// the working or stored base URL, the working or stored slots, the credential secret when
    /// the caller has one, and the configured client credentials are exactly the values the
    /// stage's expressions may reference. The engine rebuilds the environment from the
    /// accumulated working state before every request, so this constructor receives the stage
    /// constants alone.
    #[must_use]
    pub(in crate::provider) fn for_provisioning(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        secret: Option<&'a str>,
        client_id: &'a str,
        client_secret: Option<&'a str>,
    ) -> Self {
        Self {
            slots,
            base_url,
            secret,
            client_id: Some(client_id),
            client_secret,
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of the stored-credential reads — upstream model discovery
    /// (`docs/design/providers.md`) and the read-only quota query: the stored base URL, the
    /// credential secret, and the stored slots are exactly the values the stage's expressions
    /// may reference.
    #[must_use]
    pub(in crate::provider) fn for_stored_credential(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        secret: &'a str,
    ) -> Self {
        Self {
            slots,
            base_url,
            secret: Some(secret),
            ..Self::stage_absent()
        }
    }

    /// Builds the stage environment of one compaction operation dispatch
    /// (`docs/design/providers.md`): the stored base URL, the credential secret, the stored
    /// slots, the dispatch's request UUID (cached across every expression of the dispatch), and
    /// the routed model are exactly the values the stage's expressions may reference — the same
    /// value set an inference dispatch carries, without any streaming-specific member. The
    /// standalone compact operation and the token-count operation evaluate through it.
    #[must_use]
    pub(crate) fn for_compaction_operation(
        slots: &'a BTreeMap<String, String>,
        base_url: Option<&'a str>,
        secret: &'a str,
        request_uuid: &'a str,
        model: &'a str,
    ) -> Self {
        Self {
            slots,
            base_url,
            secret: Some(secret),
            request_uuid: Some(request_uuid),
            model: Some(model),
            ..Self::stage_absent()
        }
    }

    /// The stage-carried values every constructor leaves absent, shared by the per-stage
    /// constructors so each lists only the values its stage carries.
    fn stage_absent() -> Self {
        static EMPTY_SLOTS: std::sync::OnceLock<BTreeMap<String, String>> =
            std::sync::OnceLock::new();

        Self {
            base_url: None,
            secret: None,
            slots: EMPTY_SLOTS.get_or_init(BTreeMap::new),
            client_id: None,
            client_secret: None,
            authorization_code: None,
            code_verifier: None,
            redirect_uri: None,
            oauth_state: None,
            device_code: None,
            refresh_token: None,
            request_uuid: None,
            model: None,
        }
    }
}

impl ValueEnv<'_> {
    /// Evaluates one string expression into its concatenated rendering.
    ///
    /// # Errors
    ///
    /// Returns [`EvalError`] when the expression references a value the stage does not carry.
    pub(crate) fn evaluate(&self, expr: &StringExpr, what: &str) -> Result<String, EvalError> {
        let mut rendered = String::new();

        for part in &expr.0 {
            self.push_part(part, what, &mut rendered)?;
        }

        Ok(rendered)
    }

    /// Evaluates a template while borrowing its concrete codec body and authored literal leaves.
    ///
    /// # Errors
    ///
    /// Returns an evaluation failure when a string expression references an unavailable value.
    pub(crate) fn render_json<'a, T>(
        &self,
        node: &'a TemplateNode,
        body: &'a T,
        what: &str,
    ) -> Result<RenderedJson<'a, T>, EvalError> {
        match node {
            TemplateNode::Json { value } => Ok(RenderedJson::Literal(value)),
            TemplateNode::String { expr } => Ok(RenderedJson::Text(self.evaluate(expr, what)?)),
            TemplateNode::Body => Ok(RenderedJson::Body(body)),
            TemplateNode::Object { fields } => fields
                .iter()
                .map(|(name, field)| Ok((name.as_str(), self.render_json(field, body, what)?)))
                .collect::<Result<_, _>>()
                .map(RenderedJson::Object),
            TemplateNode::Array { items } => items
                .iter()
                .map(|item| self.render_json(item, body, what))
                .collect::<Result<_, _>>()
                .map(RenderedJson::Array),
        }
    }

    /// The stage's value for one direct value part, or `None` when the stage does not carry
    /// that kind. Literal text, slots, and coalesce arguments never name a stage value — the
    /// evaluator handles those forms explicitly — so only the direct kinds reach this accessor.
    #[must_use]
    fn stage_value(&self, part: &ValueExpr) -> Option<&str> {
        match part {
            ValueExpr::Secret => self.secret,
            ValueExpr::ClientId => self.client_id,
            ValueExpr::ClientSecret => self.client_secret,
            ValueExpr::AuthorizationCode => self.authorization_code,
            ValueExpr::CodeVerifier => self.code_verifier,
            ValueExpr::RedirectUri => self.redirect_uri,
            ValueExpr::OAuthState => self.oauth_state,
            ValueExpr::DeviceCode => self.device_code,
            ValueExpr::RefreshToken => self.refresh_token,
            ValueExpr::RequestUuid => self.request_uuid,
            ValueExpr::Model => self.model,
            ValueExpr::Base => self.base_url,

            ValueExpr::Literal { .. } | ValueExpr::Slot { .. } | ValueExpr::Coalesce { .. } => None,
        }
    }

    /// Evaluates one value part into `rendered`.
    fn push_part(
        &self,
        part: &ValueExpr,
        what: &str,
        rendered: &mut String,
    ) -> Result<(), EvalError> {
        let missing = |kind: &'static str| EvalError {
            kind,
            what: what.to_string(),
        };

        match part {
            ValueExpr::Literal { value } => {
                rendered.push_str(value);

                Ok(())
            }

            ValueExpr::Coalesce { parts } => {
                // The first non-empty part wins; every part empty renders empty. A missing slot
                // evaluates empty, so a coalesce picks the first slot the flow actually placed.
                for part in parts {
                    let mut candidate = String::new();

                    self.push_part(part, what, &mut candidate)?;

                    if !candidate.is_empty() {
                        rendered.push_str(&candidate);

                        break;
                    }
                }

                Ok(())
            }

            ValueExpr::Slot { name } => {
                rendered.push_str(self.slots.get(name).map(String::as_str).unwrap_or_default());

                Ok(())
            }

            // Every remaining part is one direct stage value: absent here means the stage does
            // not carry it — stored drift the write-time validation already rejected — and the
            // error names the kind through the shared display-name vocabulary.
            other => {
                let value = self
                    .stage_value(other)
                    .ok_or_else(|| missing(other.display_name()))?;

                rendered.push_str(value);

                Ok(())
            }
        }
    }
}
