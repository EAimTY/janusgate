//! Priority scans, narrow fresh failover, durable blocks, and actual-account abandonment.

use super::{selection::SelectedAttempt, *};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    provider::{StoredCredentials, quota::QuotaApplicability},
    routing::{ModelRouteTarget, ProviderPoolMember},
    storage::{
        self,
        models::{ModelCreate, insert_model_checked},
    },
    test_support::{self, MockAction, MockUpstream, RecordedHttpRequests},
};
use std::{error::Error as StdError, num::NonZeroU32, sync::Arc};
use tower::ServiceExt as _;

/// A fresh pool backed by three genuinely separate physical accounts and HTTP transports.
async fn pool(
    actions: Vec<MockAction>,
) -> Result<
    (
        Context,
        test_support::TempDbGuard,
        test_support::TempDirGuard,
        Vec<(MockUpstream, RecordedHttpRequests)>,
        Vec<(i64, i64)>,
        i64,
    ),
    Box<dyn StdError>,
> {
    pool_scripts(actions.into_iter().map(|action| vec![action]).collect()).await
}

/// Separate account transports with ordered responses across independent requests.
async fn pool_scripts(
    actions: Vec<Vec<MockAction>>,
) -> Result<
    (
        Context,
        test_support::TempDbGuard,
        test_support::TempDirGuard,
        Vec<(MockUpstream, RecordedHttpRequests)>,
        Vec<(i64, i64)>,
        i64,
    ),
    Box<dyn StdError>,
> {
    test_support::install_recording_configuration().await;
    let (database, guard) = test_support::create_test_database().await;
    let directory = test_support::TempDirGuard(std::env::temp_dir().join(format!(
        "janusgate-selection-test-{}",
        rand::random::<u64>()
    )));
    let mut servers = Vec::new();
    let mut targets = Vec::new();
    for (index, action) in actions.into_iter().enumerate() {
        let (server, recorded) = MockUpstream::start_scripted_recording_bodies(action).await;
        let mut payload =
            test_support::static_key_definition_payload(UpstreamFormat::OpenAiChatCompletion);
        payload.inference.quota_exhaustion = Some(serde_json::from_value(serde_json::json!({
            "rules":[{"name":"account-limit","statuses":[429],
                "code":{"at":["error","code"],"value":"quota_exhausted"},
                "restriction_key":[{"type":"literal","value":"account"}],
                "applies_to":{"type":"all_models"}}]
        }))?);
        let name = format!("account-{index}");
        let definition = test_support::insert_definition(&database, &name, payload).await;
        let provider = test_support::insert_provider(
            &database,
            &definition,
            &name,
            Some(&server.base()),
            StoredCredentials::ApiKey {
                api_key: "secret".into(),
            },
        )
        .await;
        let model = insert_model_checked(
            &database,
            ModelCreate {
                provider_id: provider.id,
                upstream_id: "physical-model",
                is_enabled: true,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support_override: None,
            },
        )
        .await?;
        targets.push((provider.id, model.model.id));
        servers.push((server, recorded));
    }
    let members = targets
        .iter()
        .zip(0_u32..)
        .map(|((provider_id, _), priority)| ProviderPoolMember {
            provider_id: *provider_id,
            priority,
        })
        .collect::<Vec<_>>();
    let pool_id =
        storage::routing::create_pool(&database, "pool", "pool-pool-", true, &members).await?;
    let route_id = storage::routing::create_route(
        &database,
        "public-model",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "physical-model".into(),
        },
    )
    .await?;
    let context = Context {
        database,
        api_log_store: Some(crate::storage::api_logs::ApiLogStore::new(
            directory.0.clone(),
            NonZeroU32::MIN,
        )),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: Arc::new(crate::provider::quota::coordinator::Coordinator::default()),
    };
    Ok((context, guard, directory, servers, targets, route_id))
}

/// A positive exact rejection, independent of prose or a status-only heuristic.
fn exhausted() -> MockAction {
    let body = r#"{"error":{"code":"quota_exhausted"}}"#;
    MockAction::Respond(format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ))
}

/// Executes one fresh ordinary operation with its outward accounting identity.
async fn run(context: &Context) -> Result<SelectedAttempt, Error> {
    let mut ir = super::tests::fixture_ir();
    ir.model = "public-model".into();
    execute_selection(
        context,
        ordinary_request(ir),
        fresh_pool_selection_key(&context.database).await?,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await
}

/// Supplies actual ordinary wire provenance for internally constructed test histories.
fn ordinary_request(ir: IrRequest) -> crate::format::ParsedRequest {
    let affinity = crate::format::affinity::Sources::from_wire_messages(
        crate::format::UpstreamFormat::OpenAiChatCompletion,
        &ir.messages,
    );
    crate::format::ParsedRequest {
        ir,
        affinity,
        is_streaming: false,
        should_include_usage: false,
        compaction: None,
    }
}

/// Reads actual account rows after each abandonment.
async fn rows(context: &Context) -> Result<Vec<crate::storage::api_logs::ApiLog>, StorageError> {
    crate::storage::api_logs::list_api_logs(
        context.api_log_store.as_ref(),
        100,
        None,
        &crate::storage::api_logs::UsageFilter::default(),
    )
    .await
}

/// Compact-only summary dispatches finalize rejected accounts independently. A successful
/// summary followed by quota failure retains its reported usage in one locked-account row.
/// Exhausting a fresh scan does not invent an outward accounting row.
///
/// # Errors
///
/// Returns fixture, router, body, or accounting failures.
///
/// # Panics
///
/// Panics if abandonment statuses, physical identities, or billed usage are misattributed.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn compaction_abandonments_finalize_before_the_next_account() -> Result<(), Box<dyn StdError>>
{
    let _configuration = test_support::configuration_lock();
    for (is_exhausted, reported) in [
        (false, None),
        (false, Some((0, 0))),
        (false, Some((10, 2))),
        (true, None),
    ] {
        let last =
            if is_exhausted {
                exhausted()
            } else {
                MockAction::respond_json(serde_json::json!({
                "choices":[{"index":0,"message":{"role":"assistant","content":"summary"},
                    "finish_reason":"stop"}],"usage":reported.map(|(input, output)|
                        serde_json::json!({"prompt_tokens":input,"completion_tokens":output}))
            }).to_string())
            };
        let (context, _database, _directory, _servers, targets, route_id) =
            pool(vec![exhausted(), exhausted(), last]).await?;
        storage::api_keys::insert_api_key(
            &context.database,
            &storage::api_keys::ApiKeyFields {
                name: "compact",
                key: "gateway-key",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await?;
        let app = axum::Router::new()
            .nest("/api", crate::api::router(context.database.clone()))
            .with_state(context.clone());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/openai-v1/responses/compact")
                    .header("Content-Type", "application/json")
                    .header("Authorization", "Bearer gateway-key")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "model":"public-model","input":[{"type":"message",
                                "role":"user","content":"history".repeat(6000)},
                                {"type":"message","role":"user","content":"pending"}]
                        })
                        .to_string(),
                    ))?,
            )
            .await?;
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
        let body = String::from_utf8(body.to_vec())?;
        assert_eq!(
            status,
            if is_exhausted {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::OK
            },
            "{body}"
        );
        assert!(body.contains(if is_exhausted {
            "pool_quota_exhausted"
        } else {
            "response.compaction"
        }));
        // Buffered success hands its final write to a task; abandoned writes completed before
        // the next selection, and the final accounting owner must finish before inspection.
        let observed = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let observed = rows(&context).await?;
                if observed.len() == 3 {
                    return Ok::<_, StorageError>(observed);
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        for (index, row) in observed.iter().rev().enumerate() {
            let is_final_success = !is_exhausted && index == 2;
            assert_eq!(
                Some((row.provider_id, row.model_id)),
                targets.get(index).copied()
            );
            assert_eq!(row.model_route_id, route_id);
            assert_eq!(
                row.endpoint_type,
                ApiLogEndpointType::OpenaiResponsesCompact
            );
            assert_eq!(
                row.upstream_http_status,
                Some(if is_final_success { 200 } else { 429 })
            );
            assert_eq!(row.http_status, is_final_success.then_some(200));
            let usage = is_final_success.then_some(reported).flatten();
            assert_eq!(row.input_tokens, usage.map(|(input, _)| input));
            assert_eq!(row.output_tokens, usage.map(|(_, output)| output));
            assert_eq!(
                row.outcome,
                if is_final_success {
                    ApiLogOutcome::Completed
                } else {
                    ApiLogOutcome::UpstreamError
                }
            );
        }
    }
    for reported in [None, Some((0, 0)), Some((8, 3))] {
        let summary = MockAction::respond_json(
            serde_json::json!({
                "choices":[{"index":0,"message":{"role":"assistant","content":"accepted summary"},
                    "finish_reason":"stop"}],"usage":reported.map(|(input, output)|
                    serde_json::json!({"prompt_tokens":input,"completion_tokens":output}))
            })
            .to_string(),
        );
        let (context, _database, _directory, servers, targets, route_id) = pool_scripts(vec![
            vec![summary, exhausted()],
            vec![MockAction::respond_json("must not run".into())],
        ])
        .await?;
        storage::api_keys::insert_api_key(
            &context.database,
            &storage::api_keys::ApiKeyFields {
                name: "stage-ledger",
                key: "stage-ledger-key",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await?;
        let app = axum::Router::new()
            .nest("/api", crate::api::router(context.database.clone()))
            .with_state(context.clone());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/openai-v1/responses")
                    .header("Content-Type", "application/json")
                    .header("Authorization", "Bearer stage-ledger-key")
                    .body(axum::body::Body::from(
                        serde_json::json!({
                            "model":"public-model",
                            "input":[{"type":"message","role":"user","content":"w".repeat(150_001)},
                                {"type":"message","role":"user","content":"pending"}],
                            "context_management":[{"type":"compaction","compact_threshold":1}]
                        })
                        .to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(
            body.pointer("/error/code")
                .and_then(serde_json::Value::as_str),
            Some("bound_account_quota_exhausted")
        );
        let observed = rows(&context).await?;
        assert_eq!(observed.len(), 1, "all same-account stages finalize once");
        #[expect(
            clippy::expect_used,
            reason = "one actual locked-account row was asserted"
        )]
        let row = observed.first().expect("stage ledger row");
        assert_eq!(
            Some((row.provider_id, row.model_id)),
            targets.first().copied()
        );
        assert_eq!(row.model_route_id, route_id);
        assert_eq!(row.endpoint_type, ApiLogEndpointType::OpenaiResponses);
        assert_eq!(
            row.upstream_http_status,
            Some(200),
            "first summary headers own upstream status"
        );
        assert_eq!(
            row.http_status,
            Some(429),
            "final quota error owns outward status"
        );
        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(
            (row.input_tokens, row.output_tokens),
            (
                reported.map(|(input, _)| input),
                reported.map(|(_, output)| output)
            ),
            "the earlier successful stage retains reported totals exactly once"
        );
        for (index, (_, recorded)) in servers.into_iter().enumerate() {
            #[expect(clippy::expect_used, reason = "fixture recording locks are unpoisoned")]
            let recorded = recorded.lock().expect("requests");
            assert_eq!(recorded.len(), if index == 0 { 2 } else { 0 });
        }
    }
    Ok(())
}

/// A and B may each be abandoned on their first exact rejection; accepted C never returns to
/// selection after a parse failure, and the public alias never leaks into the upstream model.
///
/// # Errors
///
/// Returns fixture, execution, stream, or accounting failures.
///
/// # Panics
///
/// Panics if order, commitment, or actual-account attribution changes.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn fresh_quota_only_attempts_scan_once_and_account_before_continuing()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory, servers, targets, route_id) = pool(vec![
        exhausted(),
        exhausted(),
        MockAction::respond_json("malformed".into()),
    ])
    .await?;
    let selected = run(&context).await?;
    assert_eq!(
        Some(selected.routing.provider.id),
        targets.last().map(|target| target.0)
    );
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Accepted {
        mut events,
        attempt,
    } = selected.outcome
    else {
        panic!("accepted headers must commit C");
    };
    assert_eq!(
        rows(&context).await?.len(),
        2,
        "abandonments finish before C executes"
    );
    assert!(matches!(
        events.next().await,
        Some(Err(Error::Unavailable { .. }))
    ));
    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(502))
        .await;
    let rows = rows(&context).await?;
    assert_eq!(rows.len(), 3);
    for (index, row) in rows.iter().rev().enumerate() {
        assert_eq!(
            Some((row.provider_id, row.model_id)),
            targets.get(index).copied()
        );
        assert_eq!(row.model_route_id, route_id);
        assert_eq!(row.http_status, (index == 2).then_some(502));
        assert_eq!(
            row.upstream_http_status,
            Some(if index == 2 { 200 } else { 429 })
        );
        assert_eq!(row.input_tokens, None);
    }
    for (_, requests) in servers {
        #[expect(clippy::expect_used, reason = "fixture request locks are unpoisoned")]
        let requests = requests.lock().expect("requests recorded");
        assert_eq!(requests.len(), 1, "there is no wraparound");
        #[expect(clippy::expect_used, reason = "a dispatched body parses")]
        let body: serde_json::Value =
            serde_json::from_str(&requests.first().expect("one request").body)
                .expect("JSON request");
        assert_eq!(
            body.get("model").and_then(serde_json::Value::as_str),
            Some("physical-model")
        );
    }
    Ok(())
}

/// Unclassified rejections and transport ambiguity stop without inventing quota evidence;
/// known blocks produce no dispatch rows.
/// All positive rejected candidates create only abandoned rows, not an extra selection row.
///
/// # Errors
///
/// Returns fixture, execution, storage, or accounting failures.
///
/// # Panics
///
/// Panics if selection broadens failover or invents evidence or rows.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn unrelated_failures_stop_and_exhausted_selection_never_invents_a_row()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let rejection = |status: u16, body: &str, missing_bytes: usize| {
        MockAction::Respond(format!(
            "HTTP/1.1 {status} Rejected\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len() + missing_bytes
        ))
    };
    let exact = r#"{"error":{"code":"quota_exhausted"}}"#;
    let oversized = format!("{exact}{}", " ".repeat(4096));
    let release_timeout = Arc::new(tokio::sync::Notify::new());
    for (action, upstream_status, should_timeout) in [
        (MockAction::Close, None, false),
        (rejection(429, "{}", 0), Some(429), false),
        (
            rejection(429, r#"{"error":{"code":"overloaded"}}"#, 0),
            Some(429),
            false,
        ),
        (rejection(503, exact, 0), Some(503), false),
        (rejection(429, "{", 0), Some(429), false),
        (rejection(429, &format!("{exact}x"), 0), Some(429), false),
        (rejection(429, exact, 1), Some(429), false),
        (rejection(429, &oversized, 0), Some(429), false),
        (
            MockAction::RespondAfterSignal {
                response: String::new(),
                release: Arc::clone(&release_timeout),
            },
            None,
            true,
        ),
    ] {
        let (mut context, _database, _directory, servers, targets, _) =
            pool(vec![action, MockAction::respond_json("{}".into())]).await?;
        if should_timeout {
            context.data_plane = reqwest::Client::builder()
                .timeout(std::time::Duration::from_millis(50))
                .build()?;
        }
        let selected = run(&context).await?;
        if should_timeout {
            release_timeout.notify_one();
        }
        assert_eq!(
            Some(selected.routing.provider.id),
            targets.first().map(|target| target.0)
        );
        assert!(rows(&context).await?.is_empty());
        match selected.outcome {
            AttemptOutcome::Rejected { error, attempt }
            | AttemptOutcome::DispatchedFailure { error, attempt } => {
                match (&error, upstream_status) {
                    (Error::Upstream { status, .. }, Some(expected)) => {
                        assert_eq!(status.as_u16(), expected);
                    }
                    (Error::Unavailable { .. }, None) => {}
                    _ => return Err("unclassified failure changed its normal error kind".into()),
                }
                let (outcome, status) = crate::api::Error::from(error).accounting();
                assert_eq!(outcome, ApiLogOutcome::UpstreamError);
                assert_eq!(status, i64::from(upstream_status.unwrap_or(502)));
                attempt.finalize(outcome, Some(status)).await;
            }
            #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
            _ => panic!("unrelated failure is final"),
        }
        let rows = rows(&context).await?;
        assert_eq!(rows.len(), 1);
        #[expect(clippy::expect_used, reason = "one finalized attempt was asserted")]
        let row = rows.first().expect("actual attempt row");
        assert_eq!(row.upstream_http_status, upstream_status.map(i64::from));
        assert_eq!(
            row.http_status,
            Some(i64::from(upstream_status.unwrap_or(502)))
        );
        assert_eq!(
            Some((row.provider_id, row.model_id)),
            targets.first().copied()
        );
        for (provider_id, _) in &targets {
            #[expect(clippy::expect_used, reason = "the physical fixture account exists")]
            let quota = storage::quota_blocks::load_state(&context.database, *provider_id)
                .await?
                .expect("account quota state");
            assert!(
                quota.blocks.is_empty(),
                "unclassified errors never create blocks"
            );
        }
        assert_eq!(
            servers.last().map(|(_, requests)| {
                #[expect(clippy::expect_used, reason = "fixture lock is unpoisoned")]
                requests.lock().expect("requests").len()
            }),
            Some(0)
        );
    }
    // A renewal rejection precedes data-plane dispatch, even when its body matches the
    // account's generation classifier. It creates neither an inference row nor a quota block.
    let (context, _database, _directory, servers, targets, _) =
        pool(vec![exhausted(), MockAction::respond_json("{}".into())]).await?;
    #[expect(
        clippy::expect_used,
        reason = "the pool has its first physical account"
    )]
    let provider_id = targets.first().expect("first physical account").0;
    #[expect(clippy::expect_used, reason = "the fixture account was just created")]
    let provider = storage::providers::find_provider_by_id(&context.database, provider_id)
        .await?
        .expect("fixture account");
    #[expect(
        clippy::expect_used,
        reason = "the fixture account has its mock base URL"
    )]
    let base = provider.base_url.as_deref().expect("mock base URL");
    let token_url = format!("{base}/token");
    let mut payload =
        test_support::oauth_definition_payload(test_support::OAuthDefinitionFixture {
            format: UpstreamFormat::OpenAiChatCompletion,
            inference_suffix: Some("/chat/completions"),
            client_id: "fixture-client",
            initial_slots: &[],
            grant: test_support::OAuthGrantFixture::AuthorizationCode,
            exchange_url: &token_url,
            response_fields: vec![test_support::OAuthResponseField::new(
                "refresh_token",
                test_support::OAuthTarget::RefreshToken,
            )],
            terminal_codes: &["invalid_grant"],
            refresh: test_support::OAuthRefreshFixture::Grant {
                url: &token_url,
                response: None,
            },
            provisioning: None,
        });
    payload.inference.quota_exhaustion = provider.definition.payload.inference.quota_exhaustion;
    let definition = test_support::insert_definition(&context.database, "renewal", payload).await;
    let credentials = StoredCredentials::OAuth {
        access_token: "expired".into(),
        refresh_token: Some("refresh".into()),
        slots: Default::default(),
        expires_at: Some(1),
        refresh_started_at: None,
    };
    storage::providers::update_provider(
        &context.database,
        provider_id,
        storage::providers::ProviderUpdate {
            model_prefix: "",
            name: &provider.name,
            base_url: provider.base_url.as_deref(),
            is_enabled: true,
            definition: Some((&definition.name, definition.version)),
            credentials: Some(&credentials),
        },
    )
    .await?;
    let selected = run(&context).await?;
    assert_eq!(selected.routing.provider.id, provider_id);
    #[expect(clippy::panic, reason = "renewal must fail before inference dispatch")]
    let AttemptOutcome::PredispatchFailure { error, attempt } = selected.outcome else {
        panic!("renewal is a predispatch failure");
    };
    assert!(matches!(
        error,
        Error::Provider(crate::provider::Error::Rejected { status: 429, .. })
    ));
    let (outcome, status) = crate::api::Error::from(error).accounting();
    attempt.finalize(outcome, Some(status)).await;
    assert!(rows(&context).await?.is_empty());
    for (index, (_, recorded)) in servers.iter().enumerate() {
        #[expect(clippy::expect_used, reason = "fixture recording locks are unpoisoned")]
        let recorded = recorded.lock().expect("requests");
        assert_eq!(recorded.len(), usize::from(index == 0));
        if let Some(request) = recorded.first() {
            assert_eq!(request.target, "/token");
        }
    }
    #[expect(
        clippy::expect_used,
        reason = "the rebound physical account still exists"
    )]
    let state = storage::quota_blocks::load_state(&context.database, provider_id)
        .await?
        .expect("account state");
    assert!(state.blocks.is_empty());

    for is_blocked_before_dispatch in [false, true] {
        let (context, _database, _directory, servers, targets, _) =
            pool(vec![exhausted(), exhausted()]).await?;
        if is_blocked_before_dispatch {
            for (provider_id, _) in &targets {
                storage::quota_blocks::record_rejection(
                    &context.database,
                    *provider_id,
                    0,
                    &["account".into()],
                    &QuotaApplicability::AllModels,
                    crate::unix_time::unix_now_micros(),
                )
                .await?;
            }
        }
        assert!(matches!(
            run(&context).await,
            Err(Error::PoolQuotaExhausted {
                retry_after: None,
                ..
            })
        ));
        let rows = rows(&context).await?;
        assert_eq!(rows.len(), if is_blocked_before_dispatch { 0 } else { 2 });
        assert!(
            rows.iter()
                .all(|row| row.http_status.is_none() && row.upstream_http_status == Some(429))
        );
        for (_, requests) in servers {
            #[expect(clippy::expect_used, reason = "fixture lock is unpoisoned")]
            let count = requests.lock().expect("requests").len();
            assert_eq!(count, usize::from(!is_blocked_before_dispatch));
        }
    }
    Ok(())
}

/// Every ordinary outward codec uses the same quota scan for both result modes; public aliases,
/// actual-account rows, and no-response abandonments survive the real authenticated router.
///
/// # Errors
///
/// Returns fixture, router, body, timeout, or accounting failures.
///
/// # Panics
///
/// Panics if any surface bypasses selection or publishes the physical model as the public name.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn all_ordinary_surfaces_share_selection_and_actual_account_finalization()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    for is_streaming in [false, true] {
        let requests = [
            (
                "/api/openai-v1/chat/completions".to_string(),
                serde_json::json!({
                    "model":"public-model","messages":[{"role":"user","content":"hello"}],
                    "stream":is_streaming
                }),
            ),
            (
                "/api/openai-v1/responses".to_string(),
                serde_json::json!({
                    "model":"public-model","input":"hello","stream":is_streaming
                }),
            ),
            (
                format!(
                    "/api/gemini-v1beta/models/public-model:{}",
                    if is_streaming {
                        "streamGenerateContent?alt=sse"
                    } else {
                        "generateContent"
                    }
                ),
                serde_json::json!({
                    "contents":[{"role":"user","parts":[{"text":"hello"}]}]
                }),
            ),
            (
                format!(
                    "/api/gemini-v1beta/interactions{}",
                    if is_streaming { "?alt=sse" } else { "" }
                ),
                serde_json::json!({"model":"public-model","input":"hello","stream":is_streaming}),
            ),
            (
                "/api/anthropic-v1/messages".to_string(),
                serde_json::json!({
                    "model":"public-model","max_tokens":16,
                    "messages":[{"role":"user","content":"hello"}],"stream":is_streaming
                }),
            ),
        ];
        for (path, body) in requests {
            let accepted = if is_streaming {
                let first = serde_json::json!({"id":"upstream-id","model":"physical-model",
                    "choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},
                        "finish_reason":null}]});
                let last = serde_json::json!({"id":"upstream-id","model":"physical-model",
                    "choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":10,"completion_tokens":2}});
                let body = format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n");
                MockAction::Respond(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                    Content-Length: {}\r\n\r\n{body}",
                    body.len()
                ))
            } else {
                MockAction::respond_json(serde_json::json!({
                    "id":"upstream-id","model":"physical-model",
                    "choices":[{"index":0,"message":{"role":"assistant","content":"hello"},
                        "finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":2}
                }).to_string())
            };
            let (context, _database, _directory, servers, targets, route_id) =
                pool(vec![exhausted(), accepted]).await?;
            let key = storage::api_keys::insert_api_key(
                &context.database,
                &storage::api_keys::ApiKeyFields {
                    name: "router",
                    key: "gateway-key",
                    is_enabled: true,
                    access_control: &crate::test_support::allow_all_access(),
                },
            )
            .await?;
            let app = axum::Router::new()
                .nest("/api", crate::api::router(context.database.clone()))
                .with_state(context.clone());
            let request_body = body.to_string();
            let response = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(&path)
                        .header("Content-Type", "application/json")
                        .header("Authorization", "Bearer gateway-key")
                        .header("x-goog-api-key", "gateway-key")
                        .header("x-api-key", "gateway-key")
                        .header("anthropic-version", "2023-06-01")
                        .body(axum::body::Body::from(request_body.clone()))?,
                )
                .await?;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
            let body = String::from_utf8(body.to_vec())?;
            assert!(body.contains("hello"), "{path}: {body}");
            let continuation_body = if is_streaming && path.contains("chat/completions") {
                replay_rendered_text_stream(&request_body, &body, "/choices/0/delta/content")?
            } else if is_streaming && path.contains("streamGenerateContent") {
                replay_rendered_generate_stream(&request_body, &body)?
            } else if is_streaming && path.contains("responses") {
                replay_rendered_response_stream(&request_body, &body)?
            } else if is_streaming && path.contains("messages") {
                replay_rendered_text_stream(&request_body, &body, "/delta/text")?
            } else if is_streaming {
                replay_rendered_interaction_text_stream(&request_body, &body)?
            } else {
                replay_rendered_completion(&path, &request_body, &body)?
            };
            assert!(
                !body.contains("physical-model"),
                "the outward alias is preserved: {path}"
            );
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if rows(&context).await?.len() == 2 {
                        break Ok::<(), StorageError>(());
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await??;
            let rows = rows(&context).await?;
            for (index, row) in rows.iter().rev().enumerate() {
                assert_eq!(
                    Some((row.provider_id, row.model_id)),
                    targets.get(index).copied()
                );
                assert_eq!(row.model_route_id, route_id);
                assert_eq!(row.api_key_id, key.id);
                assert_eq!(row.http_status, (index == 1).then_some(200));
                assert_eq!(row.input_tokens, (index == 1).then_some(10));
            }
            // A has a known absolute reset, B an unknown reset. The latter must not suppress
            // A's recheck header, and preflight exhaustion must not invent another row.
            for (provider_id, _) in &targets {
                storage::quota_blocks::record_rejection(
                    &context.database,
                    *provider_id,
                    0,
                    &["account".into()],
                    &QuotaApplicability::AllModels,
                    crate::unix_time::unix_now_micros(),
                )
                .await?;
            }
            let reset = crate::unix_time::unix_now_micros() + 12_000_000;
            if let Some((provider_id, _)) = targets.first() {
                context
                    .database
                    .execute(
                        "UPDATE provider_quota_blocks SET blocked_until = ?1 \
                    WHERE provider_id = ?2",
                        crate::database::params![reset, provider_id],
                    )
                    .await?;
                context
                    .database
                    .execute(
                        "UPDATE providers SET quota_revision = quota_revision + 1 \
                    WHERE id = ?1",
                        crate::database::params![provider_id],
                    )
                    .await?;
            }
            let failed = app
                .clone()
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(&path)
                        .header("Content-Type", "application/json")
                        .header("Authorization", "Bearer gateway-key")
                        .header("x-goog-api-key", "gateway-key")
                        .header("x-api-key", "gateway-key")
                        .header("anthropic-version", "2023-06-01")
                        .body(axum::body::Body::from(
                            request_body.replace("hello", "new-request"),
                        ))?,
                )
                .await?;
            assert_eq!(failed.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            #[expect(
                clippy::expect_used,
                reason = "complete future reset evidence requires a header"
            )]
            let seconds = failed
                .headers()
                .get("Retry-After")
                .expect("quota recheck header")
                .to_str()?
                .parse::<u64>()?;
            assert!((1..=12).contains(&seconds));
            let failure: serde_json::Value =
                serde_json::from_slice(&axum::body::to_bytes(failed.into_body(), 262_144).await?)?;
            let wire_code = if path.contains("/openai-v1/") {
                assert_eq!(
                    failure
                        .pointer("/error/type")
                        .and_then(serde_json::Value::as_str),
                    Some("rate_limit_error"),
                    "{path}"
                );
                failure.pointer("/error/code")
            } else if path.contains("/gemini-v1beta/") {
                failure.pointer("/error/status")
            } else {
                failure.pointer("/error/type")
            };
            let expected = if path.contains("/openai-v1/") {
                "pool_quota_exhausted"
            } else if path.contains("/gemini-v1beta/") {
                "RESOURCE_EXHAUSTED"
            } else {
                "rate_limit_error"
            };
            assert_eq!(
                wire_code.and_then(serde_json::Value::as_str),
                Some(expected),
                "{path}"
            );
            assert_eq!(self::rows(&context).await?.len(), 2);
            let bound = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri(&path)
                        .header("Content-Type", "application/json")
                        .header("Authorization", "Bearer gateway-key")
                        .header("x-goog-api-key", "gateway-key")
                        .header("x-api-key", "gateway-key")
                        .header("anthropic-version", "2023-06-01")
                        .body(axum::body::Body::from(continuation_body))?,
                )
                .await?;
            assert_eq!(bound.status(), StatusCode::TOO_MANY_REQUESTS, "{path}");
            assert!(
                bound.headers().get("Retry-After").is_none(),
                "the bound account's unknown reset cannot use another account's reset"
            );
            let bound: serde_json::Value =
                serde_json::from_slice(&axum::body::to_bytes(bound.into_body(), 262_144).await?)?;
            if path.contains("/openai-v1/") {
                assert_eq!(
                    bound
                        .pointer("/error/code")
                        .and_then(serde_json::Value::as_str),
                    Some("bound_account_quota_exhausted")
                );
            } else if path.contains("/gemini-v1beta/") {
                assert_eq!(
                    bound
                        .pointer("/error/status")
                        .and_then(serde_json::Value::as_str),
                    Some("RESOURCE_EXHAUSTED")
                );
            } else {
                assert_eq!(
                    bound
                        .pointer("/error/type")
                        .and_then(serde_json::Value::as_str),
                    Some("rate_limit_error")
                );
            }
            assert_eq!(
                self::rows(&context).await?.len(),
                2,
                "a blocked bound request never dispatches or creates a row"
            );
            for (_, recorded) in servers {
                #[expect(clippy::expect_used, reason = "fixture lock is unpoisoned")]
                let recorded = recorded.lock().expect("requests");
                assert_eq!(recorded.len(), 1, "{path}");
            }
        }
    }
    Ok(())
}

/// Account restrictions span pools while exact model restrictions leave other models eligible.
/// Direct routes bypass preflight and mirror a classified rejection through their final owner.
///
/// # Errors
///
/// Returns fixture, execution, storage, or accounting failures.
///
/// # Panics
///
/// Panics if quota is scoped to a pool, applicability is broadened, or direct routes scan accounts.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn exact_model_blocks_and_direct_routes_preserve_their_distinct_contracts()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory, servers, targets, _) = pool_scripts(vec![vec![
        MockAction::respond_json("malformed".into()),
        MockAction::respond_json("malformed".into()),
    ]])
    .await?;
    #[expect(clippy::expect_used, reason = "the fixture has one physical target")]
    let (provider_id, _) = *targets.first().expect("physical target");
    insert_model_checked(
        &context.database,
        ModelCreate {
            provider_id,
            upstream_id: "another-model",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;
    #[expect(clippy::expect_used, reason = "the fixture creates one initial pool")]
    let first_pool = storage::routing::list_pools(&context.database)
        .await?
        .first()
        .expect("first pool")
        .id;
    let second_pool = storage::routing::create_pool(
        &context.database,
        "shared",
        "pool-shared-",
        true,
        &[ProviderPoolMember {
            provider_id,
            priority: 0,
        }],
    )
    .await?;
    for (pool_id, prefix) in [(first_pool, "first"), (second_pool, "second")] {
        for (suffix, upstream_id) in [("M", "physical-model"), ("N", "another-model")] {
            storage::routing::create_route(
                &context.database,
                &format!("{prefix}-{suffix}"),
                true,
                &ModelRouteTarget::Pool {
                    pool_id,
                    upstream_id: upstream_id.into(),
                },
            )
            .await?;
        }
    }
    storage::quota_blocks::record_rejection(
        &context.database,
        provider_id,
        0,
        &["another-model".into()],
        &QuotaApplicability::Models {
            upstream_ids: vec!["another-model".into()],
        },
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    let select = |public_id: &str| {
        let key_database = context.database.clone();
        let mut ir = super::tests::fixture_ir();
        ir.model = public_id.into();
        let context = &context;
        async move {
            execute_selection(
                context,
                ordinary_request(ir),
                fresh_selection_key(&key_database).await?,
                ApiLogEndpointType::OpenaiChatCompletions,
            )
            .await
        }
    };
    for public_id in ["first-M", "second-M"] {
        let selected = select(public_id).await?;
        assert_eq!(selected.routing.provider.id, provider_id);
        assert_eq!(selected.routing.model.upstream_id, "physical-model");
        #[expect(
            clippy::panic,
            reason = "another exact model remains eligible in each pool"
        )]
        let AttemptOutcome::Accepted { attempt, .. } = selected.outcome else {
            panic!("M remains eligible despite N's restriction");
        };
        attempt
            .finalize(ApiLogOutcome::UpstreamError, Some(502))
            .await;
    }
    for public_id in ["first-N", "second-N"] {
        assert!(matches!(
            select(public_id).await,
            Err(Error::PoolQuotaExhausted { .. })
        ));
    }
    let reset = storage::quota_blocks::reset(&context.database, provider_id).await?;
    storage::quota_blocks::record_rejection(
        &context.database,
        provider_id,
        reset.generation,
        &["account".into()],
        &QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    for public_id in ["first-M", "first-N", "second-M", "second-N"] {
        assert!(matches!(
            select(public_id).await,
            Err(Error::PoolQuotaExhausted { .. })
        ));
    }
    assert_eq!(
        rows(&context).await?.len(),
        2,
        "blocked scans create no extra attempts"
    );
    for (_, recorded) in servers {
        #[expect(clippy::expect_used, reason = "fixture recording locks are unpoisoned")]
        let recorded = recorded.lock().expect("requests");
        assert_eq!(recorded.len(), 2);
    }

    let (context, _database, _directory, _servers, targets, _) = pool(vec![exhausted()]).await?;
    #[expect(clippy::expect_used, reason = "the fixture has one physical target")]
    let (provider_id, model_id) = *targets.first().expect("physical target");
    let direct = storage::routing::create_route(
        &context.database,
        "direct",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    storage::quota_blocks::record_rejection(
        &context.database,
        provider_id,
        0,
        &["account".into()],
        &QuotaApplicability::AllModels,
        crate::unix_time::unix_now_micros(),
    )
    .await?;
    let mut ir = super::tests::fixture_ir();
    ir.model = "direct".into();
    let selected = execute_selection(
        &context,
        ordinary_request(ir),
        fresh_selection_key(&context.database).await?,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await?;
    assert!(selected.affinity.is_none());
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Rejected { error, attempt } = selected.outcome else {
        panic!("direct rejection remains an unfinalized final error");
    };
    assert!(matches!(
        error,
        Error::QuotaRejected {
            status: StatusCode::TOO_MANY_REQUESTS,
            ..
        }
    ));
    let (outcome, status) = crate::api::Error::from(error).accounting();
    attempt.finalize(outcome, Some(status)).await;
    let rows = rows(&context).await?;
    assert_eq!(rows.len(), 1);
    #[expect(clippy::expect_used, reason = "the direct attempt wrote one row")]
    let row = rows.first().expect("direct row");
    assert_eq!(
        (row.provider_id, row.model_id, row.model_route_id),
        (provider_id, model_id, direct)
    );
    assert_eq!(
        (row.http_status, row.upstream_http_status),
        (Some(429), Some(429))
    );
    Ok(())
}

/// Accepted headers establish a binding even when parsing fails; later quota rejection keeps
/// the real account row and removing structural eligibility cannot fall through to a peer.
///
/// # Errors
///
/// Returns fixture, dispatch, storage, or finalization failures.
///
/// # Panics
///
/// Panics if account commitment or actual-attempt accounting changes.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "configuration fixtures serialize complete tests"
)]
async fn accepted_headers_bind_through_body_failure_and_quota() -> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory, servers, targets, route_id) = pool_scripts(vec![
        vec![MockAction::respond_json("malformed".into()), exhausted()],
        vec![MockAction::respond_json("unused".into())],
    ])
    .await?;
    let key = storage::api_keys::insert_api_key(
        &context.database,
        &storage::api_keys::ApiKeyFields {
            name: "binding",
            key: "binding-key",
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await?;
    let request = || {
        let mut ir = super::tests::fixture_ir();
        ir.model = "public-model".into();
        ordinary_request(ir)
    };
    let first = execute_selection(
        &context,
        request(),
        key.id,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await?;
    #[expect(
        clippy::expect_used,
        reason = "the fixture contains the selected account"
    )]
    let (provider_id, model_id) = *targets.first().expect("first account");
    assert_eq!(first.routing.provider.id, provider_id);
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Accepted {
        mut events,
        attempt,
    } = first.outcome
    else {
        panic!("successful headers establish the account before parsing");
    };
    assert!(matches!(
        events.next().await,
        Some(Err(Error::Unavailable { .. }))
    ));
    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(502))
        .await;
    drop(events);
    let second = execute_selection(
        &context,
        request(),
        key.id,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await?;
    assert_eq!(second.routing.provider.id, provider_id);
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Rejected { error, attempt } = second.outcome else {
        panic!("a bound quota rejection cannot advance to the other account");
    };
    assert!(matches!(
        error,
        Error::BoundAccountQuotaExhausted {
            retry_after: None,
            ..
        }
    ));
    let (outcome, status) = crate::api::Error::from(error).accounting();
    attempt.finalize(outcome, Some(status)).await;
    let recorded_rows = rows(&context).await?;
    assert_eq!(recorded_rows.len(), 2);
    for row in &recorded_rows {
        assert_eq!(
            (row.provider_id, row.model_id, row.model_route_id),
            (provider_id, model_id, route_id)
        );
        assert_eq!(row.api_key_id, key.id);
        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
    }
    assert!(
        recorded_rows
            .iter()
            .any(|row| row.upstream_http_status == Some(200) && row.http_status == Some(502))
    );
    assert!(
        recorded_rows
            .iter()
            .any(|row| row.upstream_http_status == Some(429) && row.http_status == Some(429))
    );
    // A committed binding remains authoritative when its target is structurally unavailable,
    // even though the second authored account is still an eligible alternative.
    context
        .database
        .execute(
            "UPDATE models SET is_enabled = 0 WHERE id = ?1",
            crate::database::params![model_id],
        )
        .await?;
    assert!(matches!(
        execute_selection(
            &context,
            request(),
            key.id,
            ApiLogEndpointType::OpenaiChatCompletions
        )
        .await,
        Err(Error::BoundAccountUnavailable { .. })
    ));
    assert_eq!(rows(&context).await?.len(), 2);
    for (index, (_, requests)) in servers.into_iter().enumerate() {
        #[expect(clippy::expect_used, reason = "the fixture request lock is unpoisoned")]
        let requests = requests.lock().expect("requests");
        assert_eq!(requests.len(), if index == 0 { 2 } else { 0 });
    }
    Ok(())
}

/// A released follower reloads route gates and target identity before acquiring another lease;
/// changing its route to direct must restore direct rejection and accounting semantics.
///
/// # Errors
///
/// Returns fixture, mutation, task, or synchronization failures.
///
/// # Panics
///
/// Panics if the follower keeps its obsolete pool namespace after route replacement.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "configuration fixtures serialize complete tests"
)]
async fn released_follower_reloads_its_route_target() -> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory, servers, targets, route_id) =
        pool(vec![MockAction::respond_json("unused".into()), exhausted()]).await?;
    let key = storage::api_keys::insert_api_key(
        &context.database,
        &storage::api_keys::ApiKeyFields {
            name: "follower",
            key: "follower-key",
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await?;
    let mut ir = super::tests::fixture_ir();
    ir.model = "public-model".into();
    let request = ordinary_request(ir);
    let evidence = request.affinity.project(&request.ir.messages).evidence();
    let snapshot = storage::route_snapshot::find_route(&context.database, "public-model").await?;
    #[expect(clippy::panic, reason = "a wrong fixture is the test failure report")]
    let Some(storage::route_snapshot::RouteSnapshot {
        target: storage::route_snapshot::TargetSnapshot::Pool { id: pool_id, .. },
        ..
    }) = snapshot
    else {
        panic!("fixture pool route");
    };
    let namespace = crate::affinity::Namespace {
        api_key_id: key.id,
        pool_id,
    };
    #[expect(clippy::panic, reason = "a wrong fixture is the test failure report")]
    let crate::affinity::Decision::Fresh(leader) =
        context.affinity_cache.acquire(namespace, &evidence)
    else {
        panic!("first request reserves establishment");
    };
    let task_context = context.clone();
    let follower = tokio::spawn(async move {
        execute_selection(
            &task_context,
            request,
            key.id,
            ApiLogEndpointType::OpenaiChatCompletions,
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !context.affinity_cache.has_waiting_request(namespace) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    #[expect(
        clippy::expect_used,
        reason = "the fixture contains a second physical target"
    )]
    let (provider_id, model_id) = *targets.last().expect("direct target");
    storage::routing::update_route(
        &context.database,
        route_id,
        "public-model",
        true,
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;
    drop(leader);
    let selected = tokio::time::timeout(std::time::Duration::from_secs(2), follower).await???;
    assert!(selected.affinity.is_none());
    assert_eq!(selected.routing.provider.id, provider_id);
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Rejected {
        error: Error::QuotaRejected { status, .. },
        attempt,
    } = selected.outcome
    else {
        panic!("the direct route mirrors its original rejection");
    };
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(429))
        .await;
    let recorded_rows = rows(&context).await?;
    assert_eq!(recorded_rows.len(), 1);
    assert!(
        recorded_rows
            .iter()
            .all(|row| row.provider_id == provider_id
                && row.upstream_http_status == Some(429)
                && row.http_status == Some(429))
    );
    for (index, (_, recorded)) in servers.into_iter().enumerate() {
        #[expect(clippy::expect_used, reason = "fixture request locks are unpoisoned")]
        let recorded = recorded.lock().expect("requests");
        assert_eq!(recorded.len(), usize::from(index == 1));
    }
    Ok(())
}

/// Failed optional quota reads preserve durable restrictions but do not block an otherwise
/// eligible account; only actual inference dispatches own public-route accounting.
///
/// # Errors
///
/// Returns fixture, mutation, router, body, timeout, or accounting failures.
///
/// # Panics
///
/// Panics if failed observation invents availability evidence or misattributes a primary result.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "routing fixtures serialize global configuration"
)]
async fn failed_quota_reads_preserve_blocks_and_allow_unrestricted_accounts()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let accepted = MockAction::respond_json(
        serde_json::json!({"id":"upstream", "model":"physical-model",
            "choices":[{"index":0,"message":{"role":"assistant","content":"eligible answer"},
                "finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}})
        .to_string(),
    );
    for has_existing_block in [false, true] {
        let (context, _database, _directory, servers, targets, route_id) = pool_scripts(vec![
            vec![MockAction::Close, accepted.clone()],
            vec![accepted.clone()],
        ])
        .await?;
        #[expect(clippy::expect_used, reason = "the pool fixture has a primary account")]
        let provider_id = targets.first().expect("primary account").0;
        #[expect(clippy::expect_used, reason = "the primary account was just created")]
        let provider = storage::providers::find_provider_by_id(&context.database, provider_id)
            .await?
            .expect("primary account");
        let mut payload =
            test_support::static_key_definition_payload(UpstreamFormat::OpenAiChatCompletion);
        payload.quota = Some(serde_json::from_value(serde_json::json!({
            "request":{"method":"GET","url":[{"type":"base"},
                {"type":"literal","value":"/quota"}]},
            "sources":[{"type":"object","key":"account","at":[],"is_required":true,
                "scope":{"type":"unknown"},"entries":[{"key":"weekly","label":"Weekly",
                    "at":[],"is_required":true,"routing":{"type":"all_models"},
                    "window":{"type":"unknown"},"reset":{"type":"unknown"},
                    "measurement":{"type":"remaining","value":{
                        "field":{"base":"entry","path":["remaining"],"is_required":true},
                        "encoding":"number"},"unit":"requests"}}]}]
        }))?);
        let definition =
            test_support::insert_definition(&context.database, "readable", payload).await;
        storage::providers::update_provider(
            &context.database,
            provider_id,
            storage::providers::ProviderUpdate {
                model_prefix: "",
                name: &provider.name,
                base_url: provider.base_url.as_deref(),
                is_enabled: true,
                definition: Some((&definition.name, definition.version)),
                credentials: None,
            },
        )
        .await?;
        #[expect(clippy::expect_used, reason = "rebinding retains the physical account")]
        let before = storage::quota_blocks::load_state(&context.database, provider_id)
            .await?
            .expect("current generation");
        if has_existing_block {
            storage::quota_blocks::record_rejection(
                &context.database,
                provider_id,
                before.fence.generation,
                &["account".into(), "weekly".into()],
                &QuotaApplicability::AllModels,
                crate::unix_time::unix_now_micros(),
            )
            .await?;
        }
        storage::api_keys::insert_api_key(
            &context.database,
            &storage::api_keys::ApiKeyFields {
                name: "optional-quota",
                key: "optional-quota-key",
                is_enabled: true,
                access_control: &crate::test_support::allow_all_access(),
            },
        )
        .await?;
        let app = axum::Router::new()
            .nest("/api", crate::api::router(context.database.clone()))
            .with_state(context.clone());
        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/openai-v1/chat/completions")
                    .header("Content-Type", "application/json")
                    .header("Authorization", "Bearer optional-quota-key")
                    .body(axum::body::Body::from(
                        serde_json::json!({"model":"public-model",
                    "messages":[{"role":"user","content":"quota is optional"}]})
                        .to_string(),
                    ))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(
            body.get("model").and_then(serde_json::Value::as_str),
            Some("public-model")
        );
        assert_eq!(
            body.pointer("/choices/0/message/content")
                .and_then(serde_json::Value::as_str),
            Some("eligible answer")
        );
        #[expect(clippy::expect_used, reason = "the queried account remains stored")]
        let after = storage::quota_blocks::load_state(&context.database, provider_id)
            .await?
            .expect("account state");
        assert_eq!(after.blocks.len(), usize::from(has_existing_block));
        #[expect(clippy::expect_used, reason = "the queried account remains stored")]
        let (blocks, refresh) = context
            .quota_coordinator
            .availability(&context.database, provider_id)
            .await?
            .expect("stored availability");
        assert_eq!(blocks, after.blocks);
        assert!(matches!(
            refresh,
            crate::provider::quota::coordinator::LastRefresh::Failed {
                reason: crate::provider::quota::coordinator::RefreshFailure::Transport,
                ..
            }
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while rows(&context).await?.len() != 1 {
                tokio::task::yield_now().await;
            }
            Ok::<_, StorageError>(())
        })
        .await??;
        let rows = rows(&context).await?;
        #[expect(clippy::expect_used, reason = "one actual inference row was asserted")]
        let row = rows.first().expect("actual inference");
        #[expect(clippy::expect_used, reason = "the pool has both physical targets")]
        let selected = targets
            .get(usize::from(has_existing_block))
            .expect("eligible target");
        assert_eq!((row.provider_id, row.model_id), *selected);
        assert_eq!(row.model_route_id, route_id);
        assert_eq!(row.upstream_http_status, Some(200));
        assert_eq!(row.http_status, Some(200));
        assert_eq!((row.input_tokens, row.output_tokens), (Some(3), Some(2)));
        for (index, (_, recorded)) in servers.iter().enumerate() {
            #[expect(clippy::expect_used, reason = "fixture recording locks are unpoisoned")]
            let recorded = recorded.lock().expect("requests");
            let generation_count = usize::from(index == usize::from(has_existing_block));
            assert_eq!(recorded.len(), generation_count + usize::from(index == 0));
            for request in recorded.iter() {
                if request.target == "/quota" {
                    assert_eq!(request.method, "GET");
                } else {
                    let body: serde_json::Value = serde_json::from_str(&request.body)?;
                    assert_eq!(
                        body.get("model").and_then(serde_json::Value::as_str),
                        Some("physical-model")
                    );
                }
            }
        }
    }
    Ok(())
}

/// Fresh followers wait through quota failover, survive another follower's cancellation, and
/// execute their own generation only after the leader commits the accepting account.
///
/// # Errors
///
/// Returns fixture, router, body, task, timeout, or accounting failures.
///
/// # Panics
///
/// Panics if pending selection permits duplicate dispatch or shares the leader's output.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "routing fixtures serialize global configuration"
)]
async fn fresh_followers_wait_for_account_commit_and_generate_independently()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let release_rejection = Arc::new(tokio::sync::Notify::new());
    let release_acceptance = Arc::new(tokio::sync::Notify::new());
    let MockAction::Respond(rejection) = exhausted() else {
        return Err("quota fixture must contain a complete response".into());
    };
    let answer = |text: &str| {
        MockAction::respond_json(
            serde_json::json!({"id":"upstream", "model":"physical-model",
                "choices":[{"index":0,"message":{"role":"assistant","content":text},
                    "finish_reason":"stop"}]})
            .to_string(),
        )
    };
    let MockAction::Respond(accepted) = answer("leader answer") else {
        return Err("answer fixture must contain a complete response".into());
    };
    let (context, _database, _directory, servers, targets, route_id) = pool_scripts(vec![
        vec![MockAction::RespondAfterSignal {
            response: rejection,
            release: Arc::clone(&release_rejection),
        }],
        vec![
            MockAction::RespondAfterSignal {
                response: accepted,
                release: Arc::clone(&release_acceptance),
            },
            answer("follower answer"),
        ],
    ])
    .await?;
    let key = storage::api_keys::insert_api_key(
        &context.database,
        &storage::api_keys::ApiKeyFields {
            name: "concurrent",
            key: "concurrent-key",
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await?;
    let snapshot = storage::route_snapshot::find_route(&context.database, "public-model").await?;
    let Some(storage::route_snapshot::RouteSnapshot {
        target: storage::route_snapshot::TargetSnapshot::Pool { id: pool_id, .. },
        ..
    }) = snapshot
    else {
        return Err("concurrent fixture requires its authored pool".into());
    };
    let namespace = crate::affinity::Namespace {
        api_key_id: key.id,
        pool_id,
    };
    let app = axum::Router::new()
        .nest("/api", crate::api::router(context.database.clone()))
        .with_state(context.clone());
    let send = || {
        let app = app.clone();
        tokio::spawn(async move {
            let request = axum::http::Request::builder()
                .method("POST")
                .uri("/api/openai-v1/chat/completions")
                .header("Content-Type", "application/json")
                .header("Authorization", "Bearer concurrent-key")
                .body(axum::body::Body::from(
                    serde_json::json!({"model":"public-model",
                        "messages":[{"role":"user","content":"same initial history"}]})
                    .to_string(),
                ))?;
            let response = app.oneshot(request).await?;
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
            Ok::<_, Box<dyn StdError + Send + Sync>>((status, body))
        })
    };
    let count = |index: usize| {
        #[expect(clippy::expect_used, reason = "fixture account index exists")]
        let (_, recorded) = servers.get(index).expect("physical account");
        #[expect(clippy::expect_used, reason = "fixture recording lock is unpoisoned")]
        let recorded = recorded.lock().expect("requests");
        recorded.len()
    };
    let leader = send();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while count(0) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let cancelled = send();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !context.affinity_cache.has_waiting_request(namespace) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    cancelled.abort();
    assert!(cancelled.await.is_err_and(|error| error.is_cancelled()));
    assert!(!context.affinity_cache.has_waiting_request(namespace));
    assert!(!leader.is_finished());
    let follower = send();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while !context.affinity_cache.has_waiting_request(namespace) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert_eq!((count(0), count(1)), (1, 0));
    assert!(rows(&context).await?.is_empty());
    release_rejection.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while count(1) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(context.affinity_cache.has_waiting_request(namespace));
    assert!(!follower.is_finished());
    assert_eq!((count(0), count(1)), (1, 1));
    assert_eq!(
        rows(&context).await?.len(),
        1,
        "A finalized before B dispatch"
    );
    release_acceptance.notify_one();
    for (task, expected) in [(leader, "leader answer"), (follower, "follower answer")] {
        let (status, body) = tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await??
            .map_err(|error| error as Box<dyn StdError>)?;
        assert_eq!(status, StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&body)?;
        assert_eq!(
            body.pointer("/choices/0/message/content")
                .and_then(serde_json::Value::as_str),
            Some(expected)
        );
    }
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while rows(&context).await?.len() != 3 {
            tokio::task::yield_now().await;
        }
        Ok::<_, StorageError>(())
    })
    .await??;
    let rows = rows(&context).await?;
    for row in &rows {
        assert_eq!(row.model_route_id, route_id);
        let is_rejected = targets
            .first()
            .is_some_and(|target| target.0 == row.provider_id);
        assert_eq!(
            row.upstream_http_status,
            Some(if is_rejected { 429 } else { 200 })
        );
        assert_eq!(row.http_status, (!is_rejected).then_some(200));
    }
    assert_eq!((count(0), count(1)), (1, 2));
    Ok(())
}

/// Completed outward prefixes retain their account across branches, envelope/mode changes,
/// and priority swaps; new histories follow priority while missing bound models fail explicitly.
///
/// # Errors
///
/// Returns real routing, mutation, transport, or replay parsing failures.
///
/// # Panics
///
/// Panics if priority/model changes redirect bound history or fail to select eligible fresh
/// accounts.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "routing fixtures serialize global configuration"
)]
async fn priority_swaps_preserve_history_and_redirect_fresh_requests()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let accepted = MockAction::respond_json(
        serde_json::json!({
            "id":"upstream-id", "model":"physical-model",
            "choices":[{"index":0,"message":{"role":"assistant","content":"hello"},
                "finish_reason":"stop"}]
        })
        .to_string(),
    );
    let first = serde_json::json!({"id":"upstream-id","model":"physical-model",
        "choices":[{"index":0,"delta":{"role":"assistant","content":"hello"},
            "finish_reason":null}]});
    let last = serde_json::json!({"id":"upstream-id","model":"physical-model",
        "choices":[{"index":0,"delta":{},"finish_reason":"stop"}]});
    let stream = format!("data: {first}\n\ndata: {last}\n\ndata: [DONE]\n\n");
    let streaming = MockAction::Respond(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\n\r\n{stream}",
        stream.len()
    ));
    let (context, _database, _directory, servers, targets, _) = pool_scripts(vec![
        vec![
            accepted.clone(),
            accepted.clone(),
            accepted.clone(),
            streaming,
        ],
        vec![accepted.clone(), accepted],
    ])
    .await?;
    storage::api_keys::insert_api_key(
        &context.database,
        &storage::api_keys::ApiKeyFields {
            name: "mutation",
            key: "mutation-key",
            is_enabled: true,
            access_control: &crate::test_support::allow_all_access(),
        },
    )
    .await?;
    let app = axum::Router::new()
        .nest("/api", crate::api::router(context.database.clone()))
        .with_state(context.clone());
    let send = |body: serde_json::Value, expected: StatusCode| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    axum::http::Request::builder()
                        .method("POST")
                        .uri("/api/openai-v1/chat/completions")
                        .header("Content-Type", "application/json")
                        .header("Authorization", "Bearer mutation-key")
                        .body(axum::body::Body::from(body.to_string()))?,
                )
                .await?;
            assert_eq!(response.status(), expected);
            let bytes = axum::body::to_bytes(response.into_body(), 262_144).await?;
            Ok::<_, Box<dyn StdError>>(String::from_utf8(bytes.to_vec())?)
        }
    };
    let original = serde_json::json!({"model":"public-model",
        "messages":[{"role":"user","content":"original question"}]});
    let response = send(original.clone(), StatusCode::OK).await?;
    let snapshot = storage::route_snapshot::find_route(&context.database, "public-model").await?;
    #[expect(clippy::panic, reason = "fixture creates one explicit pool route")]
    let Some(storage::route_snapshot::RouteSnapshot {
        target: storage::route_snapshot::TargetSnapshot::Pool { id: pool_id, .. },
        ..
    }) = snapshot
    else {
        panic!("pool route fixture");
    };
    let members = targets
        .iter()
        .rev()
        .zip(0_u32..)
        .map(|((provider_id, _), priority)| ProviderPoolMember {
            provider_id: *provider_id,
            priority,
        })
        .collect::<Vec<_>>();
    storage::routing::update_pool(
        &context.database,
        pool_id,
        "reordered",
        "pool-reordered-",
        true,
        &members,
    )
    .await?;
    let continuation = replay_rendered_completion(
        "/api/openai-v1/chat/completions",
        &original.to_string(),
        &response,
    )?;
    let mut continuation: serde_json::Value = serde_json::from_str(&continuation)?;
    #[expect(
        clippy::indexing_slicing,
        reason = "actual replay has its native message array"
    )]
    let messages = &mut continuation["messages"];
    #[expect(
        clippy::expect_used,
        reason = "the rendered Chat replay has a message array"
    )]
    messages
        .as_array_mut()
        .expect("replay messages")
        .push(serde_json::json!({"role":"user","content":"next question"}));
    let continuation = continuation.to_string();
    send(serde_json::from_str(&continuation)?, StatusCode::OK).await?;
    storage::routing::create_route(
        &context.database,
        "public-alias",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "physical-model".into(),
        },
    )
    .await?;
    let mut branch: serde_json::Value = serde_json::from_str(&continuation)?;
    #[expect(clippy::expect_used, reason = "the Chat replay is an object")]
    let envelope = branch.as_object_mut().expect("replay envelope");
    envelope.insert("model".into(), serde_json::json!("public-alias"));
    envelope.insert("temperature".into(), serde_json::json!(0.7));
    #[expect(
        clippy::expect_used,
        reason = "actual replay has its native message array"
    )]
    let messages = envelope
        .get_mut("messages")
        .and_then(serde_json::Value::as_array_mut)
        .expect("replay messages");
    #[expect(clippy::expect_used, reason = "the new user turn was just appended")]
    let user = messages.last_mut().expect("last user turn");
    *user = serde_json::json!({"role":"user","content":"a different branch"});
    messages.insert(
        0,
        serde_json::json!({"role":"system","content":"new instructions"}),
    );
    let branched = send(branch.clone(), StatusCode::OK).await?;
    let branched: serde_json::Value = serde_json::from_str(&branched)?;
    assert_eq!(
        branched.get("model").and_then(serde_json::Value::as_str),
        Some("public-alias")
    );
    #[expect(clippy::expect_used, reason = "the branch retains its object envelope")]
    branch
        .as_object_mut()
        .expect("branch envelope")
        .insert("stream".into(), serde_json::json!(true));
    let streamed = send(branch, StatusCode::OK).await?;
    assert!(streamed.contains("data: [DONE]"));
    assert!(streamed.contains("public-alias"));
    send(
        serde_json::json!({"model":"public-model",
        "messages":[{"role":"user","content":"independent question"}]}),
        StatusCode::OK,
    )
    .await?;
    #[expect(
        clippy::expect_used,
        reason = "the fixture includes a later physical account"
    )]
    let (provider_id, _) = *targets.last().expect("later account");
    insert_model_checked(
        &context.database,
        ModelCreate {
            provider_id,
            upstream_id: "another-physical",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await?;
    storage::routing::create_route(
        &context.database,
        "public-other",
        true,
        &ModelRouteTarget::Pool {
            pool_id,
            upstream_id: "another-physical".into(),
        },
    )
    .await?;
    let mut switched: serde_json::Value = serde_json::from_str(&continuation)?;
    #[expect(
        clippy::indexing_slicing,
        reason = "actual replay fixture has an object model field"
    )]
    let model = &mut switched["model"];
    *model = serde_json::json!("public-other");
    let failure = send(switched, StatusCode::SERVICE_UNAVAILABLE).await?;
    let failure: serde_json::Value = serde_json::from_str(&failure)?;
    #[expect(
        clippy::indexing_slicing,
        reason = "the first-party error has its concrete nested fields"
    )]
    let code = failure["error"]["code"].as_str();
    assert_eq!(code, Some("bound_account_unavailable"));
    #[expect(
        clippy::indexing_slicing,
        reason = "the first-party error has its concrete nested fields"
    )]
    let message = failure["error"]["message"].as_str();
    assert_eq!(
        message,
        Some("The model `public-other` cannot use its required account.")
    );
    send(
        serde_json::json!({"model":"public-other",
        "messages":[{"role":"user","content":"independent other-model question"}]}),
        StatusCode::OK,
    )
    .await?;
    for (index, (_, recorded)) in servers.into_iter().enumerate() {
        #[expect(clippy::expect_used, reason = "fixture recording locks are unpoisoned")]
        let recorded = recorded.lock().expect("requests");
        assert_eq!(
            recorded.len(),
            if index == 0 { 4 } else { 2 },
            "account {index}"
        );
        for (request_index, request) in recorded.iter().enumerate() {
            let request: serde_json::Value = serde_json::from_str(&request.body)?;
            let expected = if index == 1 && request_index == 1 {
                "another-physical"
            } else {
                "physical-model"
            };
            assert_eq!(
                request.get("model").and_then(serde_json::Value::as_str),
                Some(expected)
            );
        }
    }
    Ok(())
}

/// Replays actual rendered conversation fields through an unchanged native client request.
///
/// # Errors
///
/// Returns fixture JSON parsing failures.
///
/// # Panics
///
/// Panics if fixture conversation fields do not match their protocol shape.
fn replay_rendered_completion(
    path: &str,
    request: &str,
    response: &str,
) -> Result<String, serde_json::Error> {
    let mut request: serde_json::Value = serde_json::from_str(request)?;
    let response: serde_json::Value = serde_json::from_str(response)?;
    #[expect(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "fixtures contain the native conversation arrays"
    )]
    if path.contains("chat/completions") {
        request["messages"]
            .as_array_mut()
            .unwrap()
            .push(response["choices"][0]["message"].clone());
    } else if path.contains("responses") {
        if let Some(text) = request["input"].as_str() {
            request["input"] = serde_json::json!([{"type":"message","role":"user","content":text}]);
        }
        request["input"]
            .as_array_mut()
            .unwrap()
            .extend(response["output"].as_array().unwrap().iter().cloned());
    } else if path.contains("interactions") {
        if let Some(text) = request["input"].as_str() {
            request["input"] = serde_json::json!([{"type":"user_input",
                "content":[{"type":"text","text":text}]}]);
        }
        request["input"]
            .as_array_mut()
            .unwrap()
            .extend(response["steps"].as_array().unwrap().iter().cloned());
    } else if path.contains("generateContent") {
        request["contents"]
            .as_array_mut()
            .unwrap()
            .push(response["candidates"][0]["content"].clone());
    } else {
        request["messages"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({
            "role":"assistant","content":response["content"]}));
    }
    Ok(request.to_string())
}

/// Replays the actual text-only native fixture answer, excluding envelope metadata.
///
/// # Errors
///
/// Returns fixture SSE or JSON parsing failures.
///
/// # Panics
///
/// Panics if the native fixture lacks its conversation array.
fn replay_rendered_text_stream(
    request: &str,
    response: &str,
    text_path: &str,
) -> Result<String, Box<dyn StdError>> {
    let text = rendered_fixture_text(response, text_path)?;
    let mut request: serde_json::Value = serde_json::from_str(request)?;
    #[expect(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        reason = "the fixture contains its native conversation array"
    )]
    request["messages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
        "role":"assistant","content":text}));
    Ok(request.to_string())
}

/// Replays actual streamed GenerateContent parts in their emitted order.
///
/// # Errors
///
/// Returns fixture SSE or JSON parsing failures.
///
/// # Panics
///
/// Panics if the native fixture lacks its conversation array.
fn replay_rendered_generate_stream(
    request: &str,
    response: &str,
) -> Result<String, Box<dyn StdError>> {
    let mut parser = crate::sse::SseParser::new();
    let mut parts = Vec::new();
    for frame in parser.ingest(response.as_bytes())? {
        let frame: serde_json::Value = serde_json::from_str(&frame.data)?;
        if let Some(actual) = frame
            .pointer("/candidates/0/content/parts")
            .and_then(serde_json::Value::as_array)
        {
            parts.extend(actual.iter().cloned());
        }
    }
    let mut request: serde_json::Value = serde_json::from_str(request)?;
    #[expect(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        reason = "the fixture contains its native conversation array"
    )]
    request["contents"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
        "role":"model","parts":parts}));
    Ok(request.to_string())
}

/// Replays the actual Responses terminal output, including its outward item ordering.
///
/// # Errors
///
/// Returns fixture SSE or JSON parsing failures.
///
/// # Panics
///
/// Panics if the successful fixture emits no completed response object.
fn replay_rendered_response_stream(
    request: &str,
    response: &str,
) -> Result<String, Box<dyn StdError>> {
    let mut parser = crate::sse::SseParser::new();
    for frame in parser.ingest(response.as_bytes())? {
        let body: serde_json::Value = serde_json::from_str(&frame.data)?;
        if body.get("type").and_then(serde_json::Value::as_str) == Some("response.completed") {
            #[expect(
                clippy::expect_used,
                reason = "the completed fixture carries its response"
            )]
            let response = body.get("response").expect("completed response");
            return Ok(replay_rendered_completion(
                "responses",
                request,
                &response.to_string(),
            )?);
        }
    }
    #[expect(clippy::panic, reason = "missing completion is a fixture failure")]
    {
        panic!("the successful fixture emits a completed response");
    }
}

/// Collects text bytes actually emitted by a text-only native stream fixture.
///
/// # Errors
///
/// Returns fixture SSE or JSON parsing failures.
fn rendered_fixture_text(response: &str, text_path: &str) -> Result<String, Box<dyn StdError>> {
    let mut parser = crate::sse::SseParser::new();
    let mut text = String::new();
    for frame in parser.ingest(response.as_bytes())? {
        if frame.data == "[DONE]" {
            continue;
        }
        let frame: serde_json::Value = serde_json::from_str(&frame.data)?;
        if let Some(part) = frame.pointer(text_path).and_then(serde_json::Value::as_str) {
            text.push_str(part);
        }
    }
    Ok(text)
}

/// Replays the actual text-only Interactions fixture in its native output step.
///
/// # Errors
///
/// Returns fixture SSE or JSON parsing failures.
fn replay_rendered_interaction_text_stream(
    request: &str,
    response: &str,
) -> Result<String, Box<dyn StdError>> {
    let text = rendered_fixture_text(response, "/delta/text")?;
    let response = serde_json::json!({"steps":[{"type":"model_output",
        "content":[{"type":"text","text":text}]}]});
    Ok(replay_rendered_completion(
        "interactions",
        request,
        &response.to_string(),
    )?)
}

/// Newly discovered models dispatch through standalone or pool prefixes on their actual account.
///
/// # Errors
///
/// Returns fixture, discovery, dispatch, or decoding errors.
///
/// # Panics
///
/// Panics if exposure ownership or the exact physical upstream model diverges.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "global configuration is isolated from reload fixtures"
)]
async fn discovered_models_dispatch_without_pool_membership_or_authored_aliases()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory, servers, targets, _) = pool(vec![
        MockAction::respond_json("malformed".into()),
        MockAction::respond_json("malformed".into()),
    ])
    .await?;
    #[expect(clippy::expect_used, reason = "the fixture creates one pool")]
    let pool_id = storage::routing::list_pools(&context.database)
        .await?
        .first()
        .expect("pool")
        .id;
    #[expect(clippy::expect_used, reason = "the fixture creates two accounts")]
    let (inside, _) = *targets.last().expect("inside account");
    storage::routing::update_pool(
        &context.database,
        pool_id,
        "pool",
        "pool-pool-",
        true,
        &[ProviderPoolMember {
            provider_id: inside,
            priority: 0,
        }],
    )
    .await?;
    for ((provider_id, _), (_, recorded)) in targets.iter().zip(&servers) {
        storage::models::apply_discovered_models(
            &context.database,
            *provider_id,
            &[crate::provider::DiscoveredModel {
                upstream_id: "new-model".into(),
                thinking_levels: None,
                max_input_tokens: None,
                max_output_tokens: None,
                compaction: None,
            }],
        )
        .await?;
        #[expect(clippy::expect_used, reason = "the fixture account exists")]
        let provider = storage::providers::find_provider_by_id(&context.database, *provider_id)
            .await?
            .expect("account");
        let mut ir = super::tests::fixture_ir();
        ir.model = if *provider_id == inside {
            "pool-pool-new-model".into()
        } else {
            format!("{}new-model", provider.model_prefix)
        };
        let selected = execute_selection(
            &context,
            ordinary_request(ir),
            fresh_selection_key(&context.database).await?,
            ApiLogEndpointType::OpenaiChatCompletions,
        )
        .await?;
        assert_eq!(selected.routing.provider.id, *provider_id);
        #[expect(clippy::expect_used, reason = "the discovered public route exists")]
        let snapshot =
            storage::route_snapshot::find_route(&context.database, &selected.routing.public_id)
                .await?
                .expect("route exists");
        assert_eq!(
            snapshot.owner(),
            if *provider_id == inside {
                crate::access_control::RouteOwner::Pool(pool_id)
            } else {
                crate::access_control::RouteOwner::Provider(*provider_id)
            }
        );
        assert_eq!(selected.routing.model.upstream_id, "new-model");
        #[expect(clippy::expect_used, reason = "mock request recording is unpoisoned")]
        let requests = recorded.lock().expect("requests");
        assert_eq!(requests.len(), 1);
        #[expect(
            clippy::expect_used,
            reason = "one direct upstream request was recorded"
        )]
        let request: serde_json::Value =
            serde_json::from_str(&requests.first().expect("request").body)?;
        assert_eq!(
            request.get("model").and_then(serde_json::Value::as_str),
            Some("new-model")
        );
    }
    Ok(())
}

/// A real client for structural fixtures that also exercise Direct routes.
async fn fresh_selection_key(database: &Database) -> Result<i64, Error> {
    Ok(storage::api_keys::insert_api_key(
        database,
        &storage::api_keys::ApiKeyFields {
            name: "Fresh selection",
            key: &format!("fresh-{}", rand::random::<u64>()),
            is_enabled: true,
            access_control: &test_support::allow_all_access(),
        },
    )
    .await?
    .id)
}

/// A real, independently authenticated client for fixtures asserting fresh selection behavior.
async fn fresh_pool_selection_key(database: &Database) -> Result<i64, Error> {
    // Every eligible pool member remains usable even when its Direct-owner default denies.
    let owner_defaults = storage::providers::list_providers(database)
        .await?
        .into_iter()
        .map(
            |provider| crate::access_control::ApiKeyOwnerDefault::Provider {
                provider_id: provider.id,
                access: crate::access_control::AccessDecision::Deny,
            },
        )
        .chain(
            storage::routing::list_pools(database)
                .await?
                .into_iter()
                .map(|pool| crate::access_control::ApiKeyOwnerDefault::Pool {
                    pool_id: pool.id,
                    access: crate::access_control::AccessDecision::Allow,
                }),
        )
        .collect();
    let policy = crate::access_control::ApiKeyAccessControl {
        default_access: crate::access_control::AccessDecision::Deny,
        owner_defaults,
        route_overrides: Vec::new(),
    };
    Ok(storage::api_keys::insert_api_key(
        database,
        &storage::api_keys::ApiKeyFields {
            name: "Fresh selection",
            key: &format!("fresh-{}", rand::random::<u64>()),
            is_enabled: true,
            access_control: &policy,
        },
    )
    .await?
    .id)
}

/// Affinity reevaluation reauthorizes policy/key state before any account work, while a bound
/// selection deliberately retains its admission until its existing operation finishes.
///
/// # Panics
/// Panics if waiting requests dispatch after revocation, retarget denial, or key removal.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "selection fixtures serialize global configuration"
)]
async fn released_affinity_waiters_reauthorize_coherent_admission() -> Result<(), Box<dyn StdError>>
{
    let _configuration = test_support::configuration_lock();
    for change in 0..4 {
        let (context, _database, _directory, servers, targets, route_id) =
            pool(vec![MockAction::Close]).await?;
        let pool_id = storage::routing::list_pools(&context.database)
            .await?
            .first()
            .map(|pool| pool.id)
            .ok_or("fixture pool")?;
        let policy = crate::access_control::ApiKeyAccessControl {
            default_access: crate::access_control::AccessDecision::Deny,
            owner_defaults: vec![crate::access_control::ApiKeyOwnerDefault::Pool {
                pool_id,
                access: crate::access_control::AccessDecision::Allow,
            }],
            route_overrides: Vec::new(),
        };
        let key = storage::api_keys::insert_api_key(
            &context.database,
            &storage::api_keys::ApiKeyFields {
                name: "Waiter",
                key: "waiter-key",
                is_enabled: true,
                access_control: &policy,
            },
        )
        .await?;
        let mut ir = super::tests::fixture_ir();
        ir.model = "public-model".into();
        let request = ordinary_request(ir);
        let evidence = request.affinity.project(&request.ir.messages).evidence();
        let namespace = crate::affinity::Namespace {
            api_key_id: key.id,
            pool_id,
        };
        #[expect(
            clippy::panic,
            reason = "the first fixture request reserves a fresh lease"
        )]
        let crate::affinity::Decision::Fresh(leader) =
            context.affinity_cache.acquire(namespace, &evidence)
        else {
            panic!("fresh fixture lease");
        };
        let task_context = context.clone();
        let follower = tokio::spawn(async move {
            execute_selection(
                &task_context,
                request,
                key.id,
                ApiLogEndpointType::OpenaiChatCompletions,
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !context.affinity_cache.has_waiting_request(namespace) {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        match change {
            0 => {
                storage::api_keys::update_api_key(
                    &context.database,
                    key.id,
                    storage::api_keys::ApiKeyFields {
                        name: "Waiter",
                        key: "waiter-key",
                        is_enabled: true,
                        access_control: &crate::access_control::ApiKeyAccessControl {
                            default_access: crate::access_control::AccessDecision::Deny,
                            owner_defaults: Vec::new(),
                            route_overrides: Vec::new(),
                        },
                    },
                )
                .await?
            }
            1 => {
                storage::api_keys::update_api_key(
                    &context.database,
                    key.id,
                    storage::api_keys::ApiKeyFields {
                        name: "Waiter",
                        key: "waiter-key",
                        is_enabled: false,
                        access_control: &policy,
                    },
                )
                .await?
            }
            2 => {
                storage::api_keys::delete_api_key(&context.database, key.id).await?;
            }
            _ => {
                let model_id = targets
                    .first()
                    .map(|(_, model_id)| *model_id)
                    .ok_or("fixture target")?;
                storage::routing::update_route(
                    &context.database,
                    route_id,
                    "public-model",
                    true,
                    &ModelRouteTarget::Direct { model_id },
                )
                .await?;
            }
        }
        drop(leader);
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), follower).await??;
        if change == 1 || change == 2 {
            assert!(matches!(
                result,
                Err(Error::Storage(StorageError::InactiveApiKey { .. }))
            ));
        } else {
            assert!(matches!(result, Err(Error::ModelNotFound { .. })));
        }
        assert!(rows(&context).await?.is_empty());
        for (_, recorded) in servers {
            #[expect(clippy::expect_used, reason = "the request recorder is unpoisoned")]
            let has_requests = !recorded.lock().expect("requests").is_empty();
            assert!(!has_requests);
        }
    }
    Ok(())
}

/// Pool access includes members added after policy authoring; once dispatch begins, its admitted
/// priority scan can finish across accounts even when later admissions see revoked permission.
///
/// # Errors
/// Returns fixture, transport, storage, or task failures.
///
/// # Panics
/// Panics if revocation interrupts admitted fallback or later requests reach an account.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "selection configuration fixtures serialize reloads"
)]
async fn admitted_pool_fallback_survives_revocation_and_includes_future_members()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let release = Arc::new(tokio::sync::Notify::new());
    let MockAction::Respond(rejection) = exhausted() else {
        return Err("complete rejection fixture".into());
    };
    let (context, _database, _directory, servers, targets, _) = pool(vec![
        MockAction::RespondAfterSignal {
            response: rejection,
            release: Arc::clone(&release),
        },
        MockAction::respond_json(
            serde_json::json!({
                "choices":[{"index":0,"message":{"role":"assistant","content":"accepted"},
                    "finish_reason":"stop"}]
            })
            .to_string(),
        ),
    ])
    .await?;
    let pool_id = storage::routing::list_pools(&context.database)
        .await?
        .first()
        .map(|pool| pool.id)
        .ok_or("fixture pool")?;
    let members = targets
        .iter()
        .zip(0_u32..)
        .map(|((provider_id, _), priority)| ProviderPoolMember {
            provider_id: *provider_id,
            priority,
        })
        .collect::<Vec<_>>();
    let first = members.first().cloned().ok_or("first member")?;
    storage::routing::update_pool(
        &context.database,
        pool_id,
        "pool",
        "pool-pool-",
        true,
        &[first],
    )
    .await?;
    let key_id = fresh_pool_selection_key(&context.database).await?;
    storage::routing::update_pool(
        &context.database,
        pool_id,
        "pool",
        "pool-pool-",
        true,
        &members,
    )
    .await?;
    let execution_context = context.clone();
    let running = tokio::spawn(async move {
        let mut ir = super::tests::fixture_ir();
        ir.model = "public-model".into();
        execute_selection(
            &execution_context,
            ordinary_request(ir),
            key_id,
            ApiLogEndpointType::OpenaiChatCompletions,
        )
        .await
    });
    let (_, requests) = servers.first().ok_or("first transport")?;
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            #[expect(clippy::expect_used, reason = "the recorder lock is not poisoned")]
            let has_started = !requests.lock().expect("recorder").is_empty();
            if has_started {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await?;
    let key = storage::api_keys::find_api_key_by_id(&context.database, key_id)
        .await?
        .ok_or("admitted key")?;
    storage::api_keys::update_api_key(
        &context.database,
        key_id,
        storage::api_keys::ApiKeyFields {
            name: &key.name,
            key: &key.key,
            is_enabled: true,
            access_control: &crate::access_control::ApiKeyAccessControl {
                default_access: crate::access_control::AccessDecision::Deny,
                owner_defaults: Vec::new(),
                route_overrides: Vec::new(),
            },
        },
    )
    .await?;
    release.notify_one();
    let accepted = running.await??;
    assert!(matches!(accepted.outcome, AttemptOutcome::Accepted { .. }));
    assert_eq!(
        targets.get(1).map(|(provider_id, _)| *provider_id),
        Some(accepted.routing.provider.id)
    );
    let mut ir = super::tests::fixture_ir();
    ir.model = "public-model".into();
    assert!(matches!(
        execute_selection(
            &context,
            ordinary_request(ir),
            key_id,
            ApiLogEndpointType::OpenaiChatCompletions
        )
        .await,
        Err(Error::ModelNotFound { .. })
    ));
    for (_, requests) in &servers {
        #[expect(clippy::expect_used, reason = "the recorder lock is not poisoned")]
        let count = requests.lock().expect("recorder").len();
        assert_eq!(count, 1);
    }
    Ok(())
}
