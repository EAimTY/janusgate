//! The string-expression interpreter: the one evaluator every templated string in a descriptor
//! runs through (README §5.1).
//!
//! A [`ValueEnv`] carries the values of one stage; evaluating an expression whose stage did not
//! supply a referenced value is stored drift and fails fast — the management write boundary
//! rejected such references at definition time. Slots evaluate to their stored value, or to the
//! empty string when absent: that is what lets a `Coalesce` pick the first slot a provisioning
//! sequence actually placed.

use super::template::{StringExpr, TemplateNode, ValueExpr};
use serde_json::Value as Json;
use std::collections::BTreeMap;

/// One value the interpreter could not supply: the expression referenced a value its stage did
/// not carry. Write-time validation rejects every such reference, so this is stored drift and
/// never a client fault.
#[derive(Debug, thiserror::Error)]
#[error("{what} references the {kind}, which this stage does not carry")]
pub(crate) struct EvalError {
    /// The human-readable kind name of the missing value.
    pub(crate) kind: &'static str,

    /// What the expression was evaluated for.
    pub(crate) what: String,
}

/// The values of one evaluation stage.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ValueEnv<'a> {
    /// The working flow base URL, or the stored provider base URL after creation.
    pub(crate) base_url: Option<&'a str>,

    /// The API key, or the current OAuth access token.
    pub(crate) secret: Option<&'a str>,

    /// The working flow slots, or the stored credential slots after creation.
    pub(crate) slots: &'a std::collections::BTreeMap<String, String>,

    /// The configured OAuth client ID.
    pub(crate) client_id: Option<&'a str>,

    /// The configured OAuth client secret.
    pub(crate) client_secret: Option<&'a str>,

    /// The authorization code being exchanged.
    pub(crate) authorization_code: Option<&'a str>,

    /// The PKCE code verifier minted at flow start.
    pub(crate) code_verifier: Option<&'a str>,

    /// The configured redirect URI.
    pub(crate) redirect_uri: Option<&'a str>,

    /// The flow's state token.
    pub(crate) oauth_state: Option<&'a str>,

    /// The flow's device code.
    pub(crate) device_code: Option<&'a str>,

    /// The stored refresh token.
    pub(crate) refresh_token: Option<&'a str>,

    /// The request UUID of the current inference dispatch, cached by the dispatcher so every
    /// reference in one dispatch renders the same value.
    pub(crate) request_uuid: Option<&'a str>,

    /// The routed model's upstream identifier.
    pub(crate) model: Option<&'a str>,
}

impl<'a> ValueEnv<'a> {
    /// Builds the stage environment of one inference dispatch (README §5.2): the stored base
    /// URL, the credential secret, the stored slots, the dispatch's request UUID, and the routed
    /// model are exactly the values the stage's expressions may reference.
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

    /// Builds the stage environment of an authorize-URL build (README §5.3): the configured
    /// client credentials and the flow's state token are exactly the values the stage's
    /// expressions may reference — the write-time validation bars the base URL and slots there.
    /// The flow's slots still ride along because the environment always carries a slot map.
    #[must_use]
    pub(crate) fn for_authorize_url(
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

    /// Builds the stage environment of a device authorization request (README §5.3): the working
    /// base URL, the flow's slots, and the configured client credentials are exactly the values
    /// the stage's expressions may reference.
    #[must_use]
    pub(crate) fn for_device_authorization(
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

    /// Builds the stage environment of a token exchange (README §5.3): the working base URL, the
    /// flow's slots, the configured client credentials, and the grant's authorization code, PKCE
    /// verifier, and redirect URI are exactly the values the stage's expressions may reference.
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

    /// Builds the stage environment of one device-flow poll attempt (README §5.3): the working
    /// base URL, the flow's slots, the configured client credentials, and the flow's device code
    /// are exactly the values the stage's expressions may reference. The same environment serves
    /// the completion's provisioning sequence, whose expressions may additionally reference the
    /// stage values a provisioning stage carries.
    #[must_use]
    pub(crate) fn for_device_poll(
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

    /// Builds the stage environment of a refresh grant (README §5.3): the working base URL, the
    /// stored slots, the configured client credentials, and the stored refresh token are exactly
    /// the values the stage's expressions may reference.
    #[must_use]
    pub(crate) fn for_refresh_grant(
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

    /// Builds the stage environment of a provisioning step or poll (README §5.4): the working or
    /// stored base URL, the working or stored slots, the credential secret when the caller has
    /// one, and the configured client credentials are exactly the values the stage's expressions
    /// may reference. The engine rebuilds the environment from the accumulated working state
    /// before every request, so this constructor receives the stage constants alone.
    #[must_use]
    pub(crate) fn for_provisioning(
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

    /// Builds the stage environment of one upstream model discovery (README §5.5): the stored
    /// base URL, the credential secret, and the stored slots are exactly the values the stage's
    /// expressions may reference.
    #[must_use]
    pub(crate) fn for_discovery(
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

    /// Renders one typed JSON template tree into a JSON value, splicing the codec-rendered body
    /// into the tree's single body placeholder.
    ///
    /// # Errors
    ///
    /// Returns [`EvalError`] when a string leaf references a value the stage does not carry.
    pub(crate) fn render_json(
        &self,
        node: &TemplateNode,
        body: Json,
        what: &str,
    ) -> Result<Json, EvalError> {
        match node {
            TemplateNode::Json { value } => Ok(value.clone()),

            TemplateNode::String { expr } => Ok(Json::String(self.evaluate(expr, what)?)),

            TemplateNode::Body => Ok(body),

            TemplateNode::Object { fields } => {
                let mut object = serde_json::Map::new();

                for (name, field) in fields {
                    object.insert(name.clone(), self.render_json(field, body.clone(), what)?);
                }

                Ok(Json::Object(object))
            }

            TemplateNode::Array { items } => {
                let mut array = Vec::with_capacity(items.len());

                for item in items {
                    array.push(self.render_json(item, body.clone(), what)?);
                }

                Ok(Json::Array(array))
            }
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
                    .ok_or_else(|| missing(other.kind().display_name()))?;

                rendered.push_str(value);

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A stage carrying one of every value, with two slots for the coalesce cases.
    fn full_env<'a>(slots: &'a BTreeMap<String, String>, request_uuid: &'a str) -> ValueEnv<'a> {
        ValueEnv {
            base_url: Some("https://upstream.example/v1"),
            secret: Some("secret-token"),
            slots,
            client_id: Some("client-id"),
            client_secret: Some("client-secret"),
            authorization_code: Some("auth-code"),
            code_verifier: Some("code-verifier"),
            redirect_uri: Some("http://localhost:1455/auth/callback"),
            oauth_state: Some("state-token"),
            device_code: Some("device-code"),
            refresh_token: Some("refresh-token"),
            request_uuid: Some(request_uuid),
            model: Some("gpt-5"),
        }
    }

    /// Renders one expression, panicking on evaluation failure.
    ///
    /// # Panics
    ///
    /// Panics when the expression does not evaluate.
    fn render(env: &ValueEnv, expr: &StringExpr) -> String {
        #[expect(clippy::unwrap_used, reason = "the test expressions always evaluate")]
        env.evaluate(expr, "test").unwrap()
    }

    /// Every value kind renders its stage value verbatim, and the empty expression renders the
    /// empty string (README §5.1).
    ///
    /// # Panics
    ///
    /// Panics if a value renders differently from its stage value.
    #[test]
    fn renders_every_value_kind() {
        let mut slots = BTreeMap::new();
        slots.insert("device_id".to_string(), "0f9b1a2c".to_string());

        let env = full_env(&slots, "uuid-1");

        let cases = [
            (StringExpr::default(), ""),
            (StringExpr::literal("Bearer "), "Bearer "),
            (StringExpr::single(ValueExpr::Secret), "secret-token"),
            (
                StringExpr::single(ValueExpr::Slot {
                    name: "device_id".to_string(),
                }),
                "0f9b1a2c",
            ),
            (StringExpr::single(ValueExpr::ClientId), "client-id"),
            (StringExpr::single(ValueExpr::ClientSecret), "client-secret"),
            (
                StringExpr::single(ValueExpr::AuthorizationCode),
                "auth-code",
            ),
            (StringExpr::single(ValueExpr::CodeVerifier), "code-verifier"),
            (
                StringExpr::single(ValueExpr::RedirectUri),
                "http://localhost:1455/auth/callback",
            ),
            (StringExpr::single(ValueExpr::OAuthState), "state-token"),
            (StringExpr::single(ValueExpr::DeviceCode), "device-code"),
            (StringExpr::single(ValueExpr::RefreshToken), "refresh-token"),
            (StringExpr::single(ValueExpr::RequestUuid), "uuid-1"),
            (StringExpr::single(ValueExpr::Model), "gpt-5"),
            (
                StringExpr::single(ValueExpr::Base),
                "https://upstream.example/v1",
            ),
        ];

        for (expr, expected) in cases {
            assert_eq!(render(&env, &expr), expected);
        }
    }

    /// A composite expression concatenates its parts in order — the inference URL shape of
    /// `base + literal + model` and the authorization-header shape of `literal + secret`.
    ///
    /// # Panics
    ///
    /// Panics if a composite renders out of order.
    #[test]
    fn concatenates_composite_expressions_in_order() {
        let slots = BTreeMap::new();
        let env = full_env(&slots, "uuid-1");

        let url = StringExpr(vec![
            ValueExpr::Base,
            ValueExpr::Literal {
                value: "/models/".to_string(),
            },
            ValueExpr::Model,
        ]);

        assert_eq!(
            render(&env, &url),
            "https://upstream.example/v1/models/gpt-5"
        );

        let header = StringExpr(vec![
            ValueExpr::Literal {
                value: "Bearer ".to_string(),
            },
            ValueExpr::Secret,
        ]);

        assert_eq!(render(&env, &header), "Bearer secret-token");
    }

    /// A missing slot renders empty — which is what makes a coalesce over provisioned slots
    /// pick the first slot the flow placed — while a missing stage value fails.
    ///
    /// # Panics
    ///
    /// Panics if an absent slot renders non-empty or a missing value stops failing.
    #[test]
    fn renders_missing_slots_empty_and_missing_values_as_errors() {
        let slots = BTreeMap::new();
        let env = full_env(&slots, "uuid-1");

        assert_eq!(
            render(
                &env,
                &StringExpr::single(ValueExpr::Slot {
                    name: "absent".to_string(),
                })
            ),
            ""
        );

        let no_secret_env = ValueEnv {
            base_url: None,
            secret: None,
            slots: &slots,
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
        };

        #[expect(
            clippy::unwrap_used,
            reason = "the missing value is constructed to fail"
        )]
        let error = no_secret_env
            .evaluate(&StringExpr::single(ValueExpr::Secret), "test")
            .unwrap_err();

        assert_eq!(error.kind, "secret");
    }

    /// A coalesce renders the first non-empty part, skips absent slots and empty literals, and
    /// renders empty when every part is empty — the Antigravity tier selection.
    ///
    /// # Panics
    ///
    /// Panics if a coalesce renders outside its contract.
    #[test]
    fn coalesce_picks_the_first_non_empty_part() {
        let mut slots = BTreeMap::new();
        slots.insert("current_tier".to_string(), "standard-tier".to_string());

        let env = full_env(&slots, "uuid-1");

        let tier = StringExpr::single(ValueExpr::Coalesce {
            parts: vec![
                ValueExpr::Slot {
                    name: "paid_tier".to_string(),
                },
                ValueExpr::Slot {
                    name: "current_tier".to_string(),
                },
                ValueExpr::Literal {
                    value: "free-tier".to_string(),
                },
            ],
        });

        assert_eq!(render(&env, &tier), "standard-tier");

        let mut empty_slots = BTreeMap::new();
        empty_slots.insert("paid_tier".to_string(), String::new());

        let empty_env = full_env(&empty_slots, "uuid-1");

        assert_eq!(render(&empty_env, &tier), "free-tier");

        let all_empty = StringExpr::single(ValueExpr::Coalesce {
            parts: vec![
                ValueExpr::Slot {
                    name: "paid_tier".to_string(),
                },
                ValueExpr::Literal {
                    value: String::new(),
                },
            ],
        });

        assert_eq!(render(&empty_env, &all_empty), "");
    }

    /// The template renderer splices the codec-rendered body into the single body placeholder
    /// and evaluates the string leaves (README §5.1).
    ///
    /// # Panics
    ///
    /// Panics if the rendered JSON diverges from the expected shape.
    #[test]
    fn renders_the_envelope_around_the_body() {
        let slots = BTreeMap::new();
        let env = full_env(&slots, "uuid-9");

        let node = TemplateNode::Object {
            fields: [
                (
                    "project".to_string(),
                    TemplateNode::String {
                        expr: StringExpr::single(ValueExpr::Slot {
                            name: "project".to_string(),
                        }),
                    },
                ),
                (
                    "model".to_string(),
                    TemplateNode::String {
                        expr: StringExpr::single(ValueExpr::Model),
                    },
                ),
                ("request".to_string(), TemplateNode::Body),
                (
                    "requestType".to_string(),
                    TemplateNode::Json {
                        value: serde_json::json!("agent"),
                    },
                ),
                (
                    "requestId".to_string(),
                    TemplateNode::String {
                        expr: StringExpr(vec![
                            ValueExpr::Literal {
                                value: "agent-".to_string(),
                            },
                            ValueExpr::RequestUuid,
                        ]),
                    },
                ),
            ]
            .into_iter()
            .collect(),
        };

        let body = serde_json::json!({"contents": []});

        #[expect(clippy::unwrap_used, reason = "the test envelope always renders")]
        let rendered = env.render_json(&node, body, "test").unwrap();

        assert_eq!(
            rendered,
            serde_json::json!({
                "project": "",
                "model": "gpt-5",
                "request": {"contents": []},
                "requestType": "agent",
                "requestId": "agent-uuid-9"
            })
        );
    }
}
