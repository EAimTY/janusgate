//! Forced upstream quota observation and fenced account availability reconciliation.

use super::{Error, route};
use crate::{context::Context, provider::quota};
use axum::{Json, extract::State};
use axum_extra::extract::WithRejection;
use serde::{Deserialize, Serialize};

route! {
    pub(super) GetProviderQuota: {
        path: "/providers:quota",
        method: http::Method::POST,
        request: GetProviderQuotaRequest,
        response: GetProviderQuotaResponse,
    }
}

/// Strict quota query request; provider serving state does not constrain it.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderQuotaRequest {
    /// Provider row whose exact definition and credentials are queried.
    pub provider_id: i64,
}

/// One complete upstream reading; unknown facts remain explicit variants.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub(super) struct GetProviderQuotaResponse {
    /// Gateway observation time, UTC microseconds, stamped before extraction.
    pub observed_at: quota::SafeTimestamp,
    /// Independently identified upstream allowance pools, in deterministic order.
    pub quotas: Vec<quota::QuotaEntry>,
}

/// Forces or joins a current-generation read and reconciles its complete derived evidence.
pub(super) async fn get(
    State(context): State<Context>,
    WithRejection(Json(request), _): WithRejection<Json<GetProviderQuotaRequest>, Error>,
) -> Result<Json<GetProviderQuotaResponse>, Error> {
    let observation = context
        .quota_coordinator
        .refresh(
            &context.database,
            &context.control_plane,
            request.provider_id,
        )
        .await?;
    Ok(Json(GetProviderQuotaResponse {
        observed_at: observation.observed_at,
        quotas: observation.quotas,
    }))
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "fixtures and their expected wire outcomes are deliberate"
    )]

    use super::*;
    use crate::{
        format::UpstreamFormat,
        management_api::Route as _,
        provider::{StoredCredentials, definition::DefinitionPayload},
        storage,
        test_support::{
            self, MockAction, MockUpstream, OAuthDefinitionFixture, OAuthGrantFixture,
            OAuthRefreshFixture, OAuthResponseField, OAuthTarget,
        },
    };
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::{Value, json};
    use std::collections::BTreeMap;
    use tower::Service as _;

    /// Quota recipe shared by transport, renewal, and immutable-version acceptance tests.
    ///
    /// # Panics
    ///
    /// Panics if the typed fixture does not parse.
    fn recipe() -> crate::provider::definition::quota::QuotaDescriptor {
        serde_json::from_value(json!({
            "request":{"method":"GET","url":[{"type":"base"},{"type":"literal","value":"/usage"}],
                "headers":[{"name":"Authorization",
                    "value":[{"type":"literal",
                    "value":"Bearer "},
                    {"type":"secret"}]}]},

            "sources":[{"type":"object","key":"account","at":[],"is_required":true,
                    "entries":[{"key":"primary",
                    "label":"Primary",
                    "at":[],
                    "is_required":true,

                    "window":{"type":"unknown"},
                        "measurement":{"type":"unknown"},
                        "reset":{"type":"unknown"}}]}]
        }))
        .unwrap()
    }

    /// Builds router state with no inference/accounting side effects.
    fn context(database: crate::database::Database) -> Context {
        Context {
            database,
            api_log_store: None,
            affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
            quota_coordinator: std::sync::Arc::new(
                crate::provider::quota::coordinator::Coordinator::default(),
            ),
            data_plane: crate::http_client::data_plane(),
            control_plane: crate::http_client::control_plane(),
        }
    }

    /// Executes the declared route through authentication, strict parsing, and its error boundary.
    ///
    /// # Panics
    ///
    /// Panics if the router or response violates the test's JSON contract.
    #[expect(
        clippy::await_holding_lock,
        reason = "the test configuration must stay serialized while authentication runs"
    )]
    async fn request(context: &Context, body: Value, is_authorized: bool) -> (StatusCode, Value) {
        let _configuration_guard = test_support::configuration_lock();
        test_support::install_global_configuration();
        let mut router = super::super::router().with_state(context.clone());
        let response = router
            .call(
                Request::builder()
                    .method(GetProviderQuota::METHOD.clone())
                    .uri(GetProviderQuota::PATH)
                    .header("Content-Type", "application/json")
                    .header(
                        "Authorization",
                        if is_authorized {
                            crate::CONFIG.management_api.token.as_str()
                        } else {
                            "incorrect"
                        },
                    )
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    /// An OAuth grant suitable for testing real credential renewal.
    fn oauth_payload(token_url: &str) -> DefinitionPayload {
        test_support::oauth_definition_payload(OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: None,
            client_id: "fixture",
            initial_slots: &[
                ("github_token", "uuid"),
                ("account_id", "uuid"),
                ("project", "uuid"),
            ],
            grant: OAuthGrantFixture::AuthorizationCode,
            exchange_url: token_url,
            response_fields: vec![OAuthResponseField::new(
                "refresh_token",
                OAuthTarget::RefreshToken,
            )],

            terminal_codes: &["invalid_grant"],
            refresh: OAuthRefreshFixture::Grant {
                url: token_url,
                response: None,
            },
            provisioning: None,
        })
    }

    /// Reviewed fixture with independently stated request facts and complete normalized output.
    #[derive(Deserialize)]
    struct PresetFixture {
        /// Actual catalog quota recipe.
        descriptor: crate::provider::definition::quota::QuotaDescriptor,
        /// Synthetic values retaining the reviewed upstream shape.
        response: Value,
        /// Complete expected result.
        expected: Vec<quota::QuotaEntry>,
        /// Evidenced wire request using synthetic credential values.
        expected_request: ExpectedRequest,
    }

    /// Independent request contract exercised against a mock upstream.
    #[derive(Deserialize)]
    struct ExpectedRequest {
        /// Upstream method.
        method: String,
        /// Original URL, checked before redirecting to the mock.
        url: String,
        /// Required headers, including authentication source.
        headers: BTreeMap<String, String>,
        /// Optional expected JSON body.
        body: Option<Value>,
    }

    /// Each researched recipe traverses storage, current credentials, one HTTP read, and the route.
    ///
    /// # Panics
    ///
    /// Panics if an evidenced request or normalized shape diverges.
    #[tokio::test]
    async fn researched_recipes_execute_through_the_management_endpoint() {
        for document in [
            include_str!("../../../fixtures/quota/codex.json"),
            include_str!("../../../fixtures/quota/claude-code.json"),
            include_str!("../../../fixtures/quota/kimi-code.json"),
            include_str!("../../../fixtures/quota/github-copilot.json"),
            include_str!("../../../fixtures/quota/antigravity.json"),
        ] {
            let mut fixture: PresetFixture = serde_json::from_str(document).unwrap();
            let (mock, seen) =
                MockUpstream::start_scripted_recording_bodies(vec![MockAction::respond_json(
                    fixture.response.to_string(),
                )])
                .await;
            let slots = BTreeMap::from([
                ("account_id".into(), "account".into()),
                ("github_token".into(), "durable-github".into()),
                ("project".into(), "stored-project".into()),
            ]);
            let client = crate::http_client::control_plane();
            let environment =
                crate::provider::definition::eval::ValueEnv::for_discovery(&slots, None, "current");
            let rendered = crate::provider::control::build_request(
                &client,
                &fixture.descriptor.request,
                &environment,
                "fixture",
            )
            .unwrap()
            .build()
            .unwrap();
            assert_eq!(rendered.url().as_str(), fixture.expected_request.url);
            let target = rendered.url().path().to_owned();
            fixture.descriptor.request.url =
                crate::provider::definition::template::StringExpr::literal(format!(
                    "{}{target}",
                    mock.base(),
                ));
            let (database, _guard) = test_support::create_test_database().await;
            let mut payload = oauth_payload(&format!("{}/token", mock.base()));
            payload.quota = Some(fixture.descriptor);
            let definition = test_support::insert_definition(&database, "fixture", payload).await;
            let provider = test_support::insert_provider(
                &database,
                &definition,
                "fixture",
                None,
                StoredCredentials::OAuth {
                    access_token: "current".into(),
                    refresh_token: None,
                    slots,
                    expires_at: None,
                    refresh_started_at: None,
                },
            )
            .await;
            let (status, body) =
                request(&context(database), json!({"provider_id":provider.id}), true).await;
            assert_eq!(status, StatusCode::OK);
            let response: GetProviderQuotaResponse = serde_json::from_value(body).unwrap();
            assert_eq!(response.quotas, fixture.expected);
            let requests = seen.lock().unwrap().clone();
            let [reading]: [_; 1] = requests.try_into().unwrap();
            assert_eq!(reading.method, fixture.expected_request.method);
            assert_eq!(reading.target, target);
            for (name, value) in fixture.expected_request.headers {
                assert_eq!(reading.header(&name), Some(value.as_str()));
            }
            match fixture.expected_request.body {
                Some(body) => {
                    assert_eq!(serde_json::from_str::<Value>(&reading.body).unwrap(), body)
                }
                None => assert!(reading.body.is_empty()),
            }
        }
    }

    /// A disabled provider renews first and queries quota once with current credentials.
    ///
    /// # Panics
    ///
    /// Panics if renewal, authentication, or state isolation diverges from the contract.
    #[tokio::test]
    async fn endpoint_renews_before_one_read_and_preserves_other_state() {
        let (mock, seen) = MockUpstream::start_scripted_recording_bodies(vec![
            MockAction::respond_json(
                json!({"access_token":"fresh",
                "refresh_token":"rotated",
                "expires_in":3600})
                .to_string(),
            ),
            MockAction::respond_json(json!({}).to_string()),
        ])
        .await;
        let (database, _guard) = test_support::create_test_database().await;
        let mut payload = oauth_payload(&format!("{}/token", mock.base()));
        payload.quota = Some(recipe());
        let definition =
            test_support::insert_definition(&database, "renewing-quota", payload).await;
        let provider = test_support::insert_provider(
            &database,
            &definition,
            "quota",
            Some(&mock.base()),
            StoredCredentials::OAuth {
                access_token: "expired".into(),
                refresh_token: Some("renew".into()),
                slots: BTreeMap::from([("github_token".into(), "durable".into())]),

                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;
        database
            .execute(
                "UPDATE providers SET is_enabled = 0 WHERE id = ?1",
                crate::database::params![provider.id],
            )
            .await
            .unwrap();
        let state = context(database.clone());
        let (status, body) = request(&state, json!({"provider_id":provider.id}), true).await;
        assert_eq!(status, StatusCode::OK);
        let response: GetProviderQuotaResponse = serde_json::from_value(body).unwrap();
        assert_eq!(response.quotas.len(), 1);
        let requests = seen.lock().unwrap().clone();
        let [renewal, reading]: [_; 2] = requests.try_into().unwrap();
        assert_eq!(renewal.target, "/token");
        assert!(renewal.body.contains("refresh_token=renew"));
        assert_eq!(reading.target, "/usage");
        assert_eq!(reading.header("authorization"), Some("Bearer fresh"));
        let stored = storage::providers::find_provider_by_id(&database, provider.id)
            .await
            .unwrap()
            .unwrap();
        assert!(!stored.is_enabled);
        assert_eq!(stored.definition.version, definition.version);
        assert_eq!(stored.credentials.secret(), "fresh");
        let models = database.query("SELECT id FROM models", ()).await.unwrap();
        assert!(models.is_empty());
    }

    /// Unsupported definitions and absent providers stop before renewal; upstream statuses are safe
    /// 502s.
    ///
    /// # Panics
    ///
    /// Panics if request classification, dispatch count, or redaction diverges.
    #[tokio::test]
    async fn endpoint_preconditions_and_rejections_are_isolated() {
        let (mock, seen) = MockUpstream::start_raw_recording(vec![
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 16\r\n\
                 Connection: close\r\n\r\nsecret-body-here"
                .into(),
            "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 16\r\n\
                 Connection: close\r\n\r\nsecret-body-here"
                .into(),
            "HTTP/1.1 401 Unauthorized\r\nContent-Length: 16\r\n\
                 Connection: close\r\n\r\nsecret-body-here"
                .into(),
        ])
        .await;
        let (database, _guard) = test_support::create_test_database().await;
        let state = context(database.clone());
        // The absent-account 404 flows through the coordinator's refresh: the rendered body is
        // pinned here because the route no longer pre-reads the provider row itself.
        let (status, body) = request(&state, json!({"provider_id":999}), true).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            json!({"errno":3,"message":"Provider 999 does not exist"})
        );
        assert_eq!(
            request(
                &state,
                json!({"provider_id":999,
            "extra":true}),
                true
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&state, json!({"provider_id":999}), false).await.0,
            StatusCode::UNAUTHORIZED
        );
        let unsupported = test_support::insert_definition(
            &database,
            "unsupported",
            oauth_payload(&format!("{}/token", mock.base())),
        )
        .await;
        let provider = test_support::insert_provider(
            &database,
            &unsupported,
            "unsupported",
            Some(&mock.base()),
            StoredCredentials::OAuth {
                access_token: "expired".into(),
                refresh_token: Some("renew".into()),
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;
        let (status, body) = request(&state, json!({"provider_id":provider.id}), true).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body.get("message"),
            Some(&json!(format!(
                "provider {} does not declare quota viewing",
                provider.id
            )))
        );
        assert!(seen.lock().unwrap().is_empty());
        let mut payload =
            test_support::static_key_definition_payload(UpstreamFormat::OpenAiChatCompletion);
        payload.quota = Some(recipe());
        let definition = test_support::insert_definition(&database, "supported", payload).await;
        let provider = test_support::insert_provider(
            &database,
            &definition,
            "supported",
            Some(&mock.base()),
            StoredCredentials::ApiKey {
                api_key: "secret".into(),
            },
        )
        .await;
        for expected_status in [401, 429] {
            let (status, body) = request(&state, json!({"provider_id":provider.id}), true).await;
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            assert_eq!(body.get("errno"), Some(&json!(5)));
            assert!(
                body.get("message")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .contains(&expected_status.to_string())
            );
            assert!(!body.to_string().contains("secret"));
        }
        assert_eq!(seen.lock().unwrap().len(), 2);
        let stored = storage::providers::find_provider_by_id(&database, provider.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.credentials, provider.credentials);
        assert_eq!(stored.definition, provider.definition);
        assert_eq!(stored.is_enabled, provider.is_enabled);
        let mut payload = oauth_payload(&format!("{}/token", mock.base()));
        payload.quota = Some(recipe());
        let definition =
            test_support::insert_definition(&database, "renewal-failure", payload).await;
        let provider = test_support::insert_provider(
            &database,
            &definition,
            "renewal-failure",
            Some(&mock.base()),
            StoredCredentials::OAuth {
                access_token: "expired".into(),
                refresh_token: Some("renew".into()),
                slots: BTreeMap::new(),
                expires_at: Some(1),
                refresh_started_at: None,
            },
        )
        .await;
        let (status, body) = request(&state, json!({"provider_id":provider.id}), true).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(body.get("errno"), Some(&json!(5)));
        assert!(!body.to_string().contains("secret"));
        assert!(body.to_string().contains("credential renewal rejected"));
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests.last().unwrap(), "POST /token");
        let stored = storage::providers::find_provider_by_id(&database, provider.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.credentials, provider.credentials);
        assert_eq!(stored.definition, provider.definition);
    }

    /// A durable slot can authenticate quota independently of the provisioned inference secret.
    /// A definition rebind returned by renewal is authoritative, including absent capability.
    ///
    /// # Panics
    ///
    /// Panics if slot isolation or the renewal/rebind race mixes provider states.
    #[tokio::test]
    async fn slot_authentication_and_renewal_rebind_are_authoritative() {
        let (mock, seen) =
            MockUpstream::start_scripted_recording_bodies(vec![MockAction::respond_json(
                "{}".into(),
            )])
            .await;
        let (database, _guard) = test_support::create_test_database().await;
        let mut payload = oauth_payload(&format!("{}/token", mock.base()));
        let mut quota = recipe();
        quota.request.headers = serde_json::from_value(json!([{"name":"Authorization",
            "value":[{"type":"literal",
            "value":"token "},
            {"type":"slot",
            "name":"github_token"}]}]))
        .unwrap();
        payload.quota = Some(quota);
        let definition =
            test_support::insert_definition(&database, "slot-quota", payload.clone()).await;
        let provider = test_support::insert_provider(
            &database,
            &definition,
            "slot",
            Some(&mock.base()),
            StoredCredentials::OAuth {
                access_token: "inference".into(),
                refresh_token: Some("renew".into()),
                slots: BTreeMap::from([("github_token".into(), "durable".into())]),
                expires_at: None,
                refresh_started_at: None,
            },
        )
        .await;
        quota::query(&database, &crate::http_client::control_plane(), &provider)
            .await
            .unwrap();
        assert_eq!(
            seen.lock()
                .unwrap()
                .first()
                .unwrap()
                .header("authorization"),
            Some("token durable")
        );
        let mut expired = provider.clone();
        if let StoredCredentials::OAuth { expires_at, .. } = &mut expired.credentials {
            *expires_at = Some(1);
        }
        payload.quota = None;
        let successor = storage::provider_definitions::insert_definition(
            &database,
            storage::provider_definitions::NewDefinition {
                name: &definition.name,
                base_version: Some(definition.version),
                payload: &payload,
                created_at: 2,
            },
        )
        .await
        .unwrap();
        storage::providers::update_provider(
            &database,
            provider.id,
            storage::providers::ProviderUpdate {
                model_prefix: "",
                name: &provider.name,
                base_url: provider.base_url.as_deref(),
                is_enabled: true,

                definition: Some((&successor.name, successor.version)),
                credentials: None,
            },
        )
        .await
        .unwrap();
        let error = quota::query(&database, &crate::http_client::control_plane(), &expired)
            .await
            .err()
            .unwrap();
        assert!(matches!(error, quota::Error::NotConfigured { .. }));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
}
