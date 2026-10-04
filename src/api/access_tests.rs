//! Native protocol discovery and denial boundaries over real storage and counted transports.

use crate::{
    access_control::{
        AccessDecision::{Allow, Deny},
        ApiKeyAccessControl, ApiKeyOwnerDefault, ApiKeyRouteOverride,
    },
    context::Context,
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{
        self,
        api_keys::{ApiKeyFields, insert_api_key, update_api_key},
    },
    test_support::{self, MockAction, MockUpstream},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::{error::Error as StdError, sync::Arc};
use tower::Service as _;

/// Native protocol request through real format authentication, parsing, and admission.
async fn call(
    router: &mut Router,
    path: &str,
    body: Option<Value>,
    secret: &str,
) -> Result<(StatusCode, Value), Box<dyn StdError>> {
    let response = router
        .call(
            Request::builder()
                .method(if body.is_some() { "POST" } else { "GET" })
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", format!("Bearer {secret}"))
                .header("x-goog-api-key", secret)
                .header("x-api-key", secret)
                .header("anthropic-version", "2023-06-01")
                .body(body.map_or(Body::empty(), |body| Body::from(body.to_string())))?,
        )
        .await?;
    let status = response.status();
    Ok((
        status,
        serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?,
    ))
}

/// All native surfaces deny before control/data-plane effects, distinguish aliases, preserve
/// quota-independent discovery and valid empty pages, and retain key-state and storage errors.
///
/// # Panics
/// Panics if a forbidden request dispatches, produces accounting, or changes native error shape.
#[expect(
    clippy::await_holding_lock,
    reason = "configuration fixtures exclude concurrent reloads"
)]
#[tokio::test]
async fn native_admission_and_listing_are_key_aware_without_upstream_effects()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let (database, _guard) = test_support::create_test_database().await;
    let (mock, requests) =
        MockUpstream::start_scripted_recording_bodies(vec![MockAction::Close]).await;
    let definition = test_support::insert_static_key_definition(
        &database,
        "fixture",
        crate::format::UpstreamFormat::OpenAiResponse,
    )
    .await;
    let provider = test_support::insert_provider(
        &database,
        &definition,
        "account",
        Some(&mock.base()),
        crate::provider::StoredCredentials::ApiKey {
            api_key: "upstream".into(),
        },
    )
    .await;
    let inventory = storage::models::apply_discovered_models(
        &database,
        provider.id,
        &[crate::provider::DiscoveredModel {
            upstream_id: "M".into(),
            thinking_levels: None,
            max_input_tokens: Some(800),
            max_output_tokens: Some(400),
            compaction: None,
        }],
    )
    .await?;
    #[expect(clippy::expect_used, reason = "one discovered model was written")]
    let model_id = inventory.first().expect("physical model").model.id;
    let _direct_id = storage::routing::create_route(
        &database,
        "denied",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    let alias_id = storage::routing::create_route(
        &database,
        "allowed-alias",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    let pool_id = storage::routing::create_pool(
        &database,
        "pool",
        "pool-",
        true,
        &[ProviderPoolMember {
            provider_id: provider.id,
            priority: 0,
        }],
    )
    .await?;
    storage::routing::create_route(
        &database,
        "allowed-pool",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    storage::routing::create_route(
        &database,
        "disabled",
        false,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    let mut policy = ApiKeyAccessControl {
        default_access: Deny,
        owner_defaults: vec![
            ApiKeyOwnerDefault::Provider {
                provider_id: provider.id,
                access: Deny,
            },
            ApiKeyOwnerDefault::Pool {
                pool_id,
                access: Allow,
            },
        ],
        route_overrides: vec![ApiKeyRouteOverride {
            model_route_id: alias_id,
            access: Allow,
        }],
    };
    let key = insert_api_key(
        &database,
        &ApiKeyFields {
            name: "Client",
            key: "client-secret",
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?;
    let directory = test_support::TempDirGuard(
        std::env::temp_dir().join(format!("janusgate-access-{}", rand::random::<u64>())),
    );
    let store = storage::api_logs::ApiLogStore::new(directory.0.clone(), std::num::NonZeroU32::MIN);
    let context = Context {
        database: database.clone(),
        api_log_store: Some(store.clone()),
        affinity_cache: Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: Arc::new(crate::provider::quota::coordinator::Coordinator::default()),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
    };
    let mut router = super::router(database.clone()).with_state(context);
    for (path, body, envelope) in [
        (
            "/openai-v1/chat/completions",
            json!({"model":"denied","messages":[{"role":"user","content":"hi"}]}),
            "openai",
        ),
        (
            "/openai-v1/responses",
            json!({"model":"denied","input":"hi"}),
            "openai",
        ),
        (
            "/openai-v1/responses/compact",
            json!({"model":"denied","input":"hi"}),
            "openai",
        ),
        (
            "/gemini-v1beta/models/denied:generateContent",
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}]}),
            "gemini",
        ),
        (
            "/gemini-v1beta/models/denied:streamGenerateContent?alt=sse",
            json!({"contents":[{"role":"user","parts":[{"text":"hi"}]}]}),
            "gemini",
        ),
        (
            "/gemini-v1beta/interactions",
            json!({"model":"denied","input":"hi"}),
            "gemini",
        ),
        (
            "/anthropic-v1/messages",
            json!({"model":"denied","max_tokens":10,"messages":[{"role":"user","content":"hi"}]}),
            "anthropic",
        ),
    ] {
        let (status, payload) = call(&mut router, path, Some(body.clone()), &key.key).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}: {payload}");
        match envelope {
            "openai" => assert_eq!(test_support::at(&payload, "/error/code"), "model_not_found"),
            "gemini" => assert_eq!(test_support::at(&payload, "/error/status"), "NOT_FOUND"),
            _ => {
                assert_eq!(test_support::at(&payload, "/type"), "error");
                assert_eq!(test_support::at(&payload, "/error/type"), "not_found_error");
            }
        }
        assert_eq!(
            call(&mut router, path, Some(body), "invalid-secret")
                .await?
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    #[expect(clippy::expect_used, reason = "the recorder lock is not poisoned")]
    let has_requests = !requests.lock().expect("requests").is_empty();
    assert!(!has_requests);
    // Quota exhaustion never affects the authorized structural list.
    storage::quota_blocks::record_rejection(
        &database,
        provider.id,
        0,
        &["account".into()],
        &crate::provider::quota::QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    for path in [
        "/openai-v1/models",
        "/gemini-v1beta/models",
        "/anthropic-v1/models",
    ] {
        let (status, payload) = call(&mut router, path, None, &key.key).await?;
        assert_eq!(status, StatusCode::OK);
        let rows = if path.starts_with("/gemini") {
            test_support::at(&payload, "/models")
        } else {
            test_support::at(&payload, "/data")
        };
        #[expect(
            clippy::expect_used,
            reason = "native listing success carries an array"
        )]
        let rows = rows.as_array().expect("listing array");
        let names: Vec<_> = rows
            .iter()
            .map(|row| {
                if path.starts_with("/gemini") {
                    test_support::at(row, "/name").clone()
                } else {
                    test_support::at(row, "/id").clone()
                }
            })
            .collect();
        assert_eq!(
            names,
            if path.starts_with("/gemini") {
                vec![
                    json!("models/allowed-alias"),
                    json!("models/allowed-pool"),
                    json!("models/pool-M"),
                ]
            } else {
                vec![
                    json!("allowed-alias"),
                    json!("allowed-pool"),
                    json!("pool-M"),
                ]
            }
        );
        if path.starts_with("/anthropic") {
            assert_eq!(test_support::at(&payload, "/first_id"), "allowed-alias");
            assert_eq!(test_support::at(&payload, "/last_id"), "pool-M");
        }
    }
    policy.owner_defaults.clear();
    policy.route_overrides.clear();
    update_api_key(
        &database,
        key.id,
        ApiKeyFields {
            name: &key.name,
            key: &key.key,
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?;
    for path in [
        "/openai-v1/models",
        "/gemini-v1beta/models",
        "/anthropic-v1/models",
    ] {
        let (status, payload) = call(&mut router, path, None, &key.key).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            if path.starts_with("/gemini") {
                test_support::at(&payload, "/models")
            } else {
                test_support::at(&payload, "/data")
            },
            &json!([])
        );
        if path.starts_with("/anthropic") {
            assert!(test_support::at(&payload, "/first_id").is_null());
            assert!(test_support::at(&payload, "/last_id").is_null());
        }
    }
    // Key state and missing-rule storage cannot turn into empty success or an implicit allow.
    update_api_key(
        &database,
        key.id,
        ApiKeyFields {
            name: &key.name,
            key: &key.key,
            is_enabled: false,
            access_control: &policy,
        },
    )
    .await?;
    assert_eq!(
        call(&mut router, "/openai-v1/models", None, &key.key)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    update_api_key(
        &database,
        key.id,
        ApiKeyFields {
            name: &key.name,
            key: &key.key,
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?;
    database
        .execute("DROP TABLE api_key_model_route_overrides", ())
        .await?;
    assert_eq!(
        call(&mut router, "/openai-v1/models", None, &key.key)
            .await?
            .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    assert_eq!(
        call(
            &mut router,
            "/openai-v1/responses",
            Some(json!({"model":"denied","input":"hi"})),
            &key.key
        )
        .await?
        .0,
        StatusCode::INTERNAL_SERVER_ERROR
    );
    #[expect(clippy::expect_used, reason = "the recorder lock is not poisoned")]
    let has_requests = !requests.lock().expect("requests").is_empty();
    assert!(!has_requests);
    assert!(
        storage::api_logs::list_api_logs(
            Some(&store),
            100,
            None,
            &storage::api_logs::UsageFilter::default()
        )
        .await?
        .is_empty()
    );
    Ok(())
}
