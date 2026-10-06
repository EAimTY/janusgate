//! Owned single-account execution and cancellation across the header/classifier boundary.

use super::*;
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    provider::{DiscoveryState, definition::template::StringExpr},
    storage::api_logs::{ApiLogStore, UsageFilter, list_api_logs},
    test_support::{self, MockAction, MockUpstream, TempDirGuard},
};
use std::{error::Error as StdError, num::NonZeroU32, sync::Arc, time::Duration};
use tokio::sync::Notify;

/// One physical account and model, with distinct route identity for accounting attribution.
fn routing(base: &str) -> ModelRouting {
    let mut provider = super::tests::fixture_provider(None);
    provider.definition.payload.inference.url = StringExpr::literal(format!("{base}/generate"));
    ModelRouting {
        model_route_id: 7,
        public_id: "public-model".into(),
        provider,
        model: crate::provider::Model {
            id: 3,
            upstream_id: "physical-model".into(),
            is_enabled: true,
            discovery_state: DiscoveryState::Manual,
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support: None,
            compaction_support_override: None,
        },
    }
}

/// A draft whose stamp is controlled exclusively by the data-plane executor.
fn draft() -> ApiLogDraft {
    ApiLogDraft::predispatch(2, 1, 3, 7, ApiLogEndpointType::OpenaiChatCompletions, false)
}

/// Isolates request accounting while retaining the ordinary dispatch clients.
async fn context() -> (Context, test_support::TempDbGuard, TempDirGuard) {
    test_support::install_recording_configuration().await;
    let (database, database_guard) = test_support::create_test_database().await;
    let directory = TempDirGuard(
        std::env::temp_dir().join(format!("janusgate-attempt-test-{}", rand::random::<u64>())),
    );
    let context = Context {
        database,
        api_log_store: Some(ApiLogStore::new(directory.0.clone(), NonZeroU32::MIN)),
        data_plane: crate::http_client::data_plane(),
        control_plane: crate::http_client::control_plane(),
        affinity_cache: std::sync::Arc::new(crate::affinity::Cache::default()),
        quota_coordinator: Arc::new(crate::provider::quota::coordinator::Coordinator::default()),
    };
    (context, database_guard, directory)
}

/// Reads actual rows rather than relying on finalization-call counters.
async fn rows(context: &Context) -> Result<Vec<crate::storage::api_logs::ApiLog>, StorageError> {
    list_api_logs(
        context.api_log_store.as_ref(),
        100,
        None,
        &UsageFilter::default(),
    )
    .await
}

/// Rejection and transport errors remain unfinalized until the caller chooses the outward error.
/// Accepted headers remain committed even when their body cannot be parsed.
///
/// # Errors
///
/// Returns fixture, execution, or accounting failures.
///
/// # Panics
///
/// Panics if ownership, rejection timing, or header commitment changes.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn outcomes_retain_accounting_until_the_boundary_decides() -> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory) = context().await;
    let (upstream, _requests) = MockUpstream::start_scripted_recording_bodies(vec![
        MockAction::Respond("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\n\r\n{}".into()),
        MockAction::Close,
        MockAction::respond_json("invalid-json".into()),
    ])
    .await;
    let mut unprepared = super::tests::fixture_ir();
    unprepared.state_items.push(crate::ir::StateItem {
        offset: 0,
        item: serde_json::json!({}),
        is_gateway: true,
        is_compaction: false,
    });
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::PredispatchFailure { error, attempt } = execute_attempt(
        &context,
        routing(&upstream.base()),
        unprepared,
        false,
        draft(),
        None,
    )
    .await
    else {
        panic!("unprepared gateway carriers never dispatch");
    };
    assert!(matches!(error, Error::Internal { .. }));
    attempt.finalize(ApiLogOutcome::Internal, Some(500)).await;
    assert!(rows(&context).await?.is_empty());
    let first = execute_attempt(
        &context,
        routing(&upstream.base()),
        super::tests::fixture_ir(),
        false,
        draft(),
        None,
    )
    .await;
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Rejected { error, attempt } = first else {
        panic!("non-2xx must return a rejected unfinalized attempt");
    };
    assert!(matches!(
        error,
        Error::Upstream {
            status: StatusCode::TOO_MANY_REQUESTS,
            ..
        }
    ));
    assert!(attempt.draft().can_abandon_quota_rejection());
    assert!(rows(&context).await?.is_empty());
    // A coordinator abandoning this attempt must never invent an outward status.
    attempt.finalize(ApiLogOutcome::UpstreamError, None).await;

    let second = execute_attempt(
        &context,
        routing(&upstream.base()),
        super::tests::fixture_ir(),
        false,
        draft(),
        None,
    )
    .await;
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::DispatchedFailure { error, attempt } = second else {
        panic!("transport failure must remain distinct from a rejected response");
    };
    assert!(matches!(error, Error::Unavailable { .. }));
    assert_eq!(rows(&context).await?.len(), 1);
    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(502))
        .await;

    let third = execute_attempt(
        &context,
        routing(&upstream.base()),
        super::tests::fixture_ir(),
        false,
        draft(),
        None,
    )
    .await;
    #[expect(clippy::panic, reason = "a wrong outcome is the test failure report")]
    let AttemptOutcome::Accepted {
        mut events,
        attempt,
    } = third
    else {
        panic!("2xx headers commit before body parsing");
    };
    assert!(!attempt.draft().can_abandon_quota_rejection());
    assert!(matches!(
        events.next().await,
        Some(Err(Error::Unavailable { .. }))
    ));
    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(502))
        .await;
    let rows = rows(&context).await?;
    assert_eq!(rows.len(), 3, "each owner finalizes exactly once");
    let facts: Vec<_> = rows
        .iter()
        .rev()
        .map(|row| {
            (
                row.provider_id,
                row.model_id,
                row.model_route_id,
                row.upstream_http_status,
                row.http_status,
            )
        })
        .collect();
    assert_eq!(
        facts,
        vec![
            (1, 3, 7, Some(429), None),
            (1, 3, 7, None, Some(502)),
            (1, 3, 7, Some(200), Some(502))
        ]
    );
    Ok(())
}

/// Cancellation before headers and while reading rejected classifier evidence each closes one
/// dispatched row, preserving the first headers only when they actually arrived.
///
/// # Errors
///
/// Returns fixture, timeout, join, or accounting failures.
///
/// # Panics
///
/// Panics if cancellation loses or duplicates a draft or invents an outward status.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "the global configuration lock excludes the live reload fixture; no awaited path \
              reacquires it"
)]
async fn cancelled_attempts_own_the_header_and_rejection_windows() -> Result<(), Box<dyn StdError>>
{
    let _configuration = test_support::configuration_lock();
    for has_headers in [false, true] {
        let (context, _database, _directory) = context().await;
        let entered = Arc::new(Notify::new());
        let signal = Arc::clone(&entered);
        let upstream = MockUpstream::start_responding(move || {
            signal.notify_one();
            let header = "HTTP/1.1 429 Too Many Requests\r\nContent-Length: 2\r\n\r\n";
            MockAction::StreamAfterDelay {
                head: if has_headers {
                    header.into()
                } else {
                    String::new()
                },
                frame: if has_headers {
                    "{}".into()
                } else {
                    format!("{header}{{}}")
                },
                delay: Duration::from_millis(300),
            }
        })
        .await;
        let task_context = context.clone();
        let target = routing(&upstream.base());
        let task = tokio::spawn(async move {
            execute_attempt(
                &task_context,
                target,
                super::tests::fixture_ir(),
                false,
                draft(),
                None,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(2), entered.notified()).await?;
        // Let the executor observe the immediate head; the body remains deliberately pending.
        if has_headers {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        task.abort();
        assert!(task.await.is_err());
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if !rows(&context).await?.is_empty() {
                    break Ok::<(), StorageError>(());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await??;
        let rows = rows(&context).await?;
        assert_eq!(rows.len(), 1);
        #[expect(clippy::panic, reason = "a wrong row count is the test failure report")]
        let [row] = rows.as_slice() else {
            panic!("exactly one cancelled attempt");
        };
        assert_eq!(row.outcome, ApiLogOutcome::ClientAborted);
        assert_eq!(row.http_status, None);
        assert_eq!(row.upstream_http_status, has_headers.then_some(429));
    }
    Ok(())
}

/// The send boundary observes rejection headers before quota persistence and keeps counters
/// outside quota policy. A persistence failure never releases rejected evidence to a native
/// unsupported classifier.
///
/// # Errors
///
/// Returns fixture or dispatch failures.
///
/// # Panics
///
/// Panics if quota policy or response observation diverges.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "configuration fixtures serialize their complete execution"
)]
async fn dispatch_orders_quota_evidence_and_keeps_counter_policy_disabled()
-> Result<(), Box<dyn StdError>> {
    let _configuration = test_support::configuration_lock();
    let (context, _database, _directory) = context().await;
    let definition = test_support::insert_static_key_definition(
        &context.database,
        "dispatch",
        UpstreamFormat::OpenAiChatCompletion,
    )
    .await;
    let mut provider = test_support::insert_provider(
        &context.database,
        &definition,
        "dispatch",
        None,
        crate::provider::StoredCredentials::ApiKey {
            api_key: "key".into(),
        },
    )
    .await;
    provider.definition.payload.inference.quota_exhaustion =
        Some(serde_json::from_value(serde_json::json!({"rules":[{
            "name":"quota","statuses":[429],"code":{"at":["error","code"],"value":"quota"},
            "restriction_key":[{"type":"literal","value":"quota"}],
            "applies_to":{"type":"all_models"}
        }]}))?);
    let body = r#"{"error":{"code":"quota"}}"#;
    let response = format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let (upstream, _requests) = MockUpstream::start_scripted_recording_bodies(vec![
        MockAction::Respond(response.clone()),
        MockAction::Respond(response.clone()),
        MockAction::Respond(response.clone()),
    ])
    .await;
    let mut counter = draft();
    let rejection = dispatch(
        &context,
        context.data_plane.post(upstream.base()),
        &provider,
        "physical-model",
        &mut counter,
        None,
        QuotaPolicy::Disabled,
    )
    .await?;
    assert!(matches!(
        rejection,
        DispatchOutcome::Rejected {
            status: StatusCode::TOO_MANY_REQUESTS,
            ..
        }
    ));
    assert_eq!(counter.upstream_http_status, Some(429));
    assert!(
        crate::storage::quota_blocks::load_state(&context.database, provider.id)
            .await?
            .ok_or("account exists")?
            .blocks
            .is_empty()
    );

    let mut generation = draft();
    let rejection = dispatch(
        &context,
        context.data_plane.post(upstream.base()),
        &provider,
        "physical-model",
        &mut generation,
        None,
        QuotaPolicy::Classify,
    )
    .await;
    assert!(matches!(rejection, Err(Error::QuotaRejected { .. })));
    assert_eq!(generation.upstream_http_status, Some(429));
    assert_eq!(
        crate::storage::quota_blocks::load_state(&context.database, provider.id)
            .await?
            .ok_or("account exists")?
            .blocks
            .len(),
        1
    );

    context
        .database
        .execute("DROP TABLE provider_quota_blocks", ())
        .await?;
    let mut failed_persistence = draft();
    let rejection = dispatch(
        &context,
        context.data_plane.post(upstream.base()),
        &provider,
        "physical-model",
        &mut failed_persistence,
        None,
        QuotaPolicy::Classify,
    )
    .await;
    assert!(matches!(rejection, Err(Error::Storage(_))));
    assert_eq!(failed_persistence.upstream_http_status, Some(429));
    assert!(failed_persistence.started_at.is_some());
    Ok(())
}
