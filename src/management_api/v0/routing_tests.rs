//! Public route and pool management contracts through the actual version router.

use super::{model_routes::*, provider_pools::*};
use crate::{context::Context, management_api::Route as _, test_support};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use std::error::Error as StdError;
use tower::Service as _;

/// Sends JSON through routing, authentication, and the version's rejection boundary.
///
/// # Errors
///
/// Returns request, service, body, or JSON errors.
async fn request(
    router: &mut Router,
    path: &str,
    body: Value,
) -> Result<(StatusCode, Value), Box<dyn StdError>> {
    let response = router
        .call(
            Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("authorization", &crate::CONFIG.management_api.token)
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

/// All native outward listings expose authored public routes and native capability fields,
/// without publishing discovery inventory or hiding a route because its account is blocked.
///
/// # Errors
///
/// Returns fixture, HTTP, or database errors.
///
/// # Panics
///
/// Panics if public identities, owner labels, or capabilities differ across outward surfaces.
#[expect(
    clippy::await_holding_lock,
    reason = "live authentication configuration must exclude reload fixtures"
)]
#[tokio::test]
async fn native_model_lists_share_the_explicit_public_namespace() -> Result<(), Box<dyn StdError>> {
    let _configuration_guard = test_support::configuration_lock();
    test_support::install_global_configuration();
    let (database, _database_guard) = test_support::create_test_database().await;
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
        None,
        crate::provider::StoredCredentials::ApiKey {
            api_key: "upstream-secret".into(),
        },
    )
    .await;
    let discovered = crate::provider::DiscoveredModel {
        upstream_id: "M".into(),
        thinking_levels: None,
        max_input_tokens: Some(800),
        max_output_tokens: Some(400),
        compaction: None,
    };
    let inventory =
        crate::storage::models::apply_discovered_models(&database, provider.id, &[discovered])
            .await?;
    crate::storage::api_keys::insert_api_key(
        &database,
        &crate::storage::api_keys::ApiKeyFields {
            name: "client",
            key: "client-secret",
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await?;
    let pool_id = crate::storage::routing::create_pool(
        &database,
        "coding",
        "pool-coding-",
        true,
        &[crate::routing::ProviderPoolMember {
            provider_id: provider.id,
            priority: 7,
        }],
    )
    .await?;
    crate::storage::routing::create_route(
        &database,
        "coding-M",
        true,
        &crate::routing::ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "M".into(),
        },
    )
    .await?;
    #[expect(
        clippy::indexing_slicing,
        reason = "discovery returned the seeded single model"
    )]
    let model_id = inventory[0].model.id;
    crate::storage::routing::create_route(
        &database,
        "private-M",
        true,
        &crate::routing::ModelRouteTarget::Direct { model_id },
    )
    .await?;
    database
        .execute(
            r#"INSERT INTO provider_quota_blocks
        VALUES (?1, '["weekly"]', '{"type":"all_models"}', 100, NULL)"#,
            crate::database::params![provider.id],
        )
        .await?;
    let context = Context {
        database: database.clone(),
        api_log_store: None,
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: std::sync::Arc::new(
            crate::provider::quota::coordinator::Coordinator::default(),
        ),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
    };
    let mut router = crate::api::router(database).with_state(context);
    for (path, credential, secret, first_id, second_id, input, output) in [
        (
            "/openai-v1/models",
            "authorization",
            "Bearer client-secret",
            "/data/0/id",
            "/data/2/id",
            "/data/0/max_input_tokens",
            "/data/0/max_output_tokens",
        ),
        (
            "/gemini-v1beta/models",
            "x-goog-api-key",
            "client-secret",
            "/models/0/name",
            "/models/2/name",
            "/models/0/inputTokenLimit",
            "/models/0/outputTokenLimit",
        ),
        (
            "/anthropic-v1/models",
            "x-api-key",
            "client-secret",
            "/data/0/id",
            "/data/2/id",
            "/data/0/max_input_tokens",
            "/data/0/max_tokens",
        ),
    ] {
        let response = router
            .call(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .header(credential, secret)
                    .header("anthropic-version", "2023-06-01")
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await?)?;
        let is_gemini = path.starts_with("/gemini");
        assert_eq!(
            test_support::at(&payload, first_id),
            if is_gemini {
                "models/coding-M"
            } else {
                "coding-M"
            }
        );
        assert_eq!(
            test_support::at(&payload, second_id),
            if is_gemini {
                "models/private-M"
            } else {
                "private-M"
            }
        );
        assert_eq!(test_support::at(&payload, input), 800);
        assert_eq!(test_support::at(&payload, output), 400);
        if path.starts_with("/openai") {
            assert_eq!(test_support::at(&payload, "/data/0/owned_by"), "coding");
            assert_eq!(test_support::at(&payload, "/data/2/owned_by"), "account");
        }
        assert_eq!(
            test_support::at(&payload, if is_gemini { "/models" } else { "/data" })
                .as_array()
                .map(Vec::len),
            Some(3)
        );
    }
    Ok(())
}

/// A complete administration workflow rejects malformed orders, preserves state on failed
/// references, permits routes without current inventory, and keeps identity on target edits.
///
/// # Errors
///
/// Returns fixture and HTTP errors.
///
/// # Panics
///
/// Panics if actual version-zero statuses or persisted state differ from the contract.
#[expect(
    clippy::await_holding_lock,
    reason = "global configuration reads must exclude the concurrent reload fixture"
)]
#[tokio::test]
async fn route_and_pool_actions_share_transactional_contracts() -> Result<(), Box<dyn StdError>> {
    let _guard = test_support::configuration_lock();
    test_support::install_global_configuration();
    let (database, _database_guard) = test_support::create_test_database().await;
    let definition = test_support::insert_static_key_definition(
        &database,
        "fixture",
        crate::format::UpstreamFormat::OpenAiResponse,
    )
    .await;
    let provider = test_support::insert_provider(
        &database,
        &definition,
        "A",
        None,
        crate::provider::StoredCredentials::ApiKey {
            api_key: "fixture-secret".into(),
        },
    )
    .await
    .id;
    let context = Context {
        database,
        api_log_store: None,
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: std::sync::Arc::new(
            crate::provider::quota::coordinator::Coordinator::default(),
        ),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
    };
    let mut router = super::router().with_state(context);
    let member = json!({"provider_id": provider, "priority": 7});
    let (status, _) = request(
        &mut router,
        CreateProviderPool::PATH,
        json!({"model_prefix": "",
            "name": "coding", "is_enabled": true, "members": [member.clone(), member.clone()]
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, result) = request(
        &mut router,
        CreateProviderPool::PATH,
        json!({"model_prefix": "",
            "name": "coding", "is_enabled": true, "members": [member]
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    #[expect(
        clippy::indexing_slicing,
        reason = "successful creation returns its concrete identity payload"
    )]
    let pool_id = result["provider_pool_id"].clone();
    let target = json!({"type": "pool", "pool_id": pool_id, "upstream_id": "M"});
    let (status, result) = request(
        &mut router,
        CreateModelRoute::PATH,
        json!({
            "public_id": "coding-M", "is_enabled": true, "target": target
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    #[expect(
        clippy::indexing_slicing,
        reason = "successful creation returns its concrete identity payload"
    )]
    let route_id = result["model_route_id"].clone();
    let (status, _) = request(
        &mut router,
        DeleteProviderPool::PATH,
        json!({"provider_pool_id": pool_id}),
    )
    .await?;
    assert_eq!(status, StatusCode::CONFLICT);
    let (_, before) = request(&mut router, ListProviderPools::PATH, json!({})).await?;
    let (status, _) = request(
        &mut router,
        UpdateProviderPool::PATH,
        json!({"model_prefix": "",
            "provider_pool_id": pool_id, "name": "lost", "is_enabled": false,
            "members": [{"provider_id": 9999, "priority": 0}]
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (_, after) = request(&mut router, ListProviderPools::PATH, json!({})).await?;
    assert_eq!(before, after);
    let (status, _) = request(
        &mut router,
        UpdateModelRoute::PATH,
        json!({
            "model_route_id": route_id, "public_id": "coding-M", "is_enabled": false,
            "target": {"type": "pool", "pool_id": pool_id, "upstream_id": "N"}
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (_, routes) = request(&mut router, ListModelRoutes::PATH, json!({})).await?;
    #[expect(
        clippy::indexing_slicing,
        reason = "the workflow created exactly one listed route"
    )]
    let listed_route = routes["model_routes"][0].clone();
    assert_eq!(
        listed_route,
        json!({
            "id": route_id, "public_id": "coding-M", "is_enabled": false, "is_automatic": false,
            "target": {"type": "pool", "pool_id": pool_id, "upstream_id": "N"}
        })
    );
    let (status, _) = request(
        &mut router,
        CreateModelRoute::PATH,
        json!({
            "public_id": "bad/name", "is_enabled": true, "target": target
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(
        &mut router,
        CreateModelRoute::PATH,
        json!({
            "public_id": "invalid", "is_enabled": true,
            "target": {"type": "direct", "model_id": 1, "pool_id": pool_id}
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = request(
        &mut router,
        DeleteModelRoute::PATH,
        json!({"model_route_id": route_id}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = request(
        &mut router,
        DeleteProviderPool::PATH,
        json!({"provider_pool_id": pool_id}),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    Ok(())
}

/// Complete key writes report strict policy errors through the real management boundary and
/// preserve the committed row after every rejected replacement.
///
/// # Errors
/// Returns setup, transport, serialization, or storage failures.
///
/// # Panics
/// Panics if rejection statuses or replacement atomicity differ from the public contract.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "global configuration fixtures serialize reloads"
)]
async fn key_policy_replacements_reject_atomically() -> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    test_support::install_global_configuration();
    let (database, _database_guard) = test_support::create_test_database().await;
    let definition = test_support::insert_static_key_definition(
        &database,
        "policy-fixture",
        crate::format::UpstreamFormat::OpenAiResponse,
    )
    .await;
    let provider_id = test_support::insert_provider(
        &database,
        &definition,
        "A",
        None,
        crate::provider::StoredCredentials::ApiKey {
            api_key: "upstream-secret".into(),
        },
    )
    .await
    .id;
    let context = Context {
        database: database.clone(),
        api_log_store: None,
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: std::sync::Arc::new(
            crate::provider::quota::coordinator::Coordinator::default(),
        ),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
    };
    let mut router = super::router().with_state(context);
    let policy = json!({"default_access":"deny", "owner_defaults":[
        {"type":"provider", "provider_id":provider_id, "access":"allow"}
    ], "route_overrides":[]});
    let create_request = json!({"name":"client", "key":"client-secret", "is_enabled":true,
        "access_control":policy});
    let (status, created) =
        request(&mut router, "/api-keys:create", create_request.clone()).await?;
    assert_eq!(status, StatusCode::OK);
    let key_id = test_support::at(&created, "/api_key/id")
        .as_i64()
        .ok_or("created key")?;
    let baseline = crate::storage::api_keys::list_api_keys(&database).await?;
    let (status, _) = request(&mut router, "/api-keys:create", create_request).await?;
    assert_eq!(status, StatusCode::CONFLICT);
    for (invalid, expected) in [
        (
            json!({"default_access":"deny","owner_defaults":[
            {"type":"provider","provider_id":provider_id,"access":"allow"},
            {"type":"provider","provider_id":provider_id,"access":"allow"}
        ],"route_overrides":[]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"default_access":"allow","owner_defaults":[
            {"type":"provider","provider_id":provider_id,"access":"allow"},
            {"type":"provider","provider_id":provider_id,"access":"deny"}
        ],"route_overrides":[]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"default_access":"deny","owner_defaults":[],"route_overrides":[
                {"model_route_id":9999,"access":"allow"}
            ]}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"default_access":"deny","owner_defaults":[
            {"type":"pool","pool_id":9999,"access":"allow"}
        ],"route_overrides":[]}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"default_access":"deny","owner_defaults":[
            {"type":"provider","provider_id":9999,"access":"allow"}
        ],"route_overrides":[]}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"default_access":"inherit","owner_defaults":[],"route_overrides":[]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"default_access":"deny","owner_defaults":[
            {"type":"provider","provider_id":provider_id,"pool_id":1,"access":"allow"}
        ],"route_overrides":[]}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"default_access":"deny","owner_defaults":[
            {"type":"unknown","provider_id":provider_id,"access":"allow"}
        ],"route_overrides":[]}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let (status, _) = request(
            &mut router,
            "/api-keys:update",
            json!({
                "api_key_id":key_id,"name":"changed","key":"rotated","is_enabled":false,
                "access_control":invalid
            }),
        )
        .await?;
        assert_eq!(status, expected);
        assert_eq!(
            crate::storage::api_keys::list_api_keys(&database).await?,
            baseline
        );
        let (status, _) = request(
            &mut router,
            "/api-keys:create",
            json!({
                "name":"failed","key":"new-secret","is_enabled":true,"access_control":invalid
            }),
        )
        .await?;
        assert_eq!(status, expected);
        assert_eq!(
            crate::storage::api_keys::list_api_keys(&database).await?,
            baseline
        );
    }
    Ok(())
}
