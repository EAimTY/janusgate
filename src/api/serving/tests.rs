//! Shared HTTP, completion, and delivery contract tests.

use super::{completion::*, delivery::*, *};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    format::{
        ClientFormat, ClientSseRenderer, RouteContext, UpstreamFormat,
        openai::chat_completion::Format as ChatCompletionFormat,
    },
    gateway::{self, IrEventStream},
    ir::{IrEvent, IrFinishReason, IrUsage},
    provider::StoredCredentials,
    storage::{
        api_logs::{self, ApiLogDraft, ApiLogGuard, list_api_logs},
        models::{ModelCreate, insert_model_checked},
    },
    test_support::{
        TempDbGuard, TempDirGuard, create_test_database, insert_provider,
        insert_static_key_definition,
    },
};
use std::num::NonZero;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{StreamExt as _, wrappers::ReceiverStream};

/// Builds a handler context over the given database with no api-log store:
/// the compact pipeline's placeholder failure is pre-dispatch, so no store
/// is ever consulted.
///
/// # Panics
///
/// Panics if an HTTP client fails to build, which the static configuration
/// prevents.
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

/// Capacity of the fixture event channel: wide enough that every fixture send completes
/// without awaiting the consumer.
const EVENT_CHANNEL_BUFFER: usize = 8;

/// Creates an api-log store over a fresh temporary directory, with its cleanup guard.
///
/// # Panics
///
/// Panics if the literal retention is zero, which the positive literal prevents.
fn test_store() -> (api_logs::ApiLogStore, TempDirGuard) {
    let directory =
        std::env::temp_dir().join(format!("janusgate-serving-test-{}", rand::random::<u64>()));

    #[expect(clippy::unwrap_used, reason = "non-zero test retention")]
    let retention = NonZero::new(90).unwrap();

    (
        api_logs::ApiLogStore::new(directory.clone(), retention),
        TempDirGuard(directory),
    )
}

/// The stamped accounting draft fixture: fixed identity ids, a fixed dispatch stamp, and the
/// non-streaming flag — the shape the draft carries once [`gateway::execute`] has stamped it
/// at dispatch.
fn stamped_draft() -> ApiLogDraft {
    let mut draft =
        ApiLogDraft::predispatch(3, 5, 7, 7, ApiLogEndpointType::OpenaiChatCompletions, false);
    draft.started_at = Some(1_000_000_000);
    draft
}

/// The response-boundary log context fixture, composed as [`run_completion`] composes it.
fn log_context() -> String {
    "completion for model `public-model` via provider `acme`".to_string()
}

/// Opens the fake upstream event channel the task under test consumes: the sender rides in
/// the test, and the receiving stream is boxed exactly as [`gateway::execute`] returns it.
fn event_channel() -> (mpsc::Sender<Result<IrEvent, gateway::Error>>, IrEventStream) {
    let (sender, receiver) = mpsc::channel(EVENT_CHANNEL_BUFFER);

    (sender, Box::pin(ReceiverStream::new(receiver)))
}

/// Sends an event sequence through the fixture channel in order.
///
/// # Panics
///
/// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
async fn send_events(
    sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>,
    events: impl IntoIterator<Item = Result<IrEvent, gateway::Error>>,
) {
    for event in events {
        #[expect(
            clippy::expect_used,
            reason = "the fixture channel is wide enough and its receiver lives in the task \
                          under test"
        )]
        sender
            .send(event)
            .await
            .expect("the fixture channel accepts the event");
    }
}

/// The usage observation fixture shared by the clean and failing-with-usage sequences.
fn usage_event() -> IrEvent {
    IrEvent::Usage(IrUsage {
        input_tokens: 11,
        output_tokens: 7,
        cached_tokens: None,
        cache_write_tokens: None,
        reasoning_tokens: None,
    })
}

/// The mid-stream upstream failure sequence — one start event, the transport failure, the
/// event stream's terminal `Done`.
fn failing_events() -> [Result<IrEvent, gateway::Error>; 3] {
    [
        Ok(IrEvent::MessageStart {
            id: "resp-1".to_string(),
            model: "upstream-model".to_string(),
        }),
        Err(gateway::Error::Unavailable {
            detail: "upstream stream failed: broken".to_string(),
        }),
        Ok(IrEvent::Done),
    ]
}

/// Sends one clean completion's event sequence — start, text, usage, finish, done — through
/// the fixture channel and closes the stream.
///
/// # Panics
///
/// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
async fn send_clean_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
    send_events(
        sender,
        [
            IrEvent::MessageStart {
                id: "resp-1".to_string(),
                model: "upstream-model".to_string(),
            },
            IrEvent::TextDelta("hello".to_string()),
            usage_event(),
            IrEvent::finish(IrFinishReason::Stop),
            IrEvent::Done,
        ]
        .map(Ok),
    )
    .await;
}

/// Sends a mid-stream upstream failure — one start event, the transport failure, the stream's
/// terminal `Done` — through the fixture channel and closes the stream.
///
/// # Panics
///
/// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
async fn send_failing_events(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
    send_events(sender, failing_events()).await;
}

/// Sends a mid-stream upstream failure that first delivers a usage observation — one start
/// event, the usage report, the transport failure, the stream's terminal `Done` — through the
/// fixture channel and closes the stream.
///
/// # Panics
///
/// Panics if a fixture send fails, which the wide buffer and the task-owned receiver prevent.
async fn send_failing_events_with_usage(sender: &mpsc::Sender<Result<IrEvent, gateway::Error>>) {
    let [start, tail @ ..] = failing_events();

    send_events(sender, [start, Ok(usage_event())].into_iter().chain(tail)).await;
}

/// Reads every recorded row of `store`.
///
/// # Panics
///
/// Panics if the read fails, which a healthy fixture store prevents.
async fn recorded_rows(store: &api_logs::ApiLogStore) -> Vec<api_logs::ApiLog> {
    #[expect(clippy::expect_used, reason = "a healthy fixture store reads back")]
    let rows = list_api_logs(Some(store), 100, None, &api_logs::UsageFilter::default())
        .await
        .expect("the fixture store reads back");

    rows
}

/// Reads back the single recorded row of `store`.
///
/// # Panics
///
/// Panics if the read fails or the fixture did not record exactly one row.
async fn recorded_row(store: &api_logs::ApiLogStore) -> api_logs::ApiLog {
    let rows = recorded_rows(store).await;

    assert_eq!(rows.len(), 1, "the fixture records exactly one row");

    #[expect(clippy::unwrap_used, reason = "the fixture wrote exactly one row")]
    rows.first().unwrap().clone()
}

/// Verifies the detached non-streaming finalization's abort arm (README §3 *ApiLog*): the
/// handler departs — the oneshot receiver drops — before the upstream stream has ended, the
/// stream still runs to completion, and the row finalizes `client_aborted` with `NULL`
/// status, because no response was ever produced, carrying the usage the stream delivered
/// before the abort.
///
/// # Panics
///
/// Panics if the finalization contract diverges.
#[tokio::test]
async fn finalizes_client_aborted_when_the_handler_departs_before_completion() {
    for render in [
        AggregationRender::Ordinary,
        AggregationRender::WithPrefix {
            checkpoints: Vec::new(),
            iterations: Vec::new(),
        },
    ] {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let started = std::sync::Arc::new(tokio::sync::Notify::new());
        let poll_started = started.clone();
        let events: IrEventStream = Box::pin(futures::StreamExt::chain(
            futures::stream::once(async move {
                poll_started.notify_one();
                Ok(IrEvent::MessageStart {
                    id: "delayed".into(),
                    model: "public-model".into(),
                })
            }),
            events,
        ));
        let mut accounting = ApiLogGuard::new(Some(store.clone()), stamped_draft());
        let handler = tokio::spawn(detached_aggregation::<ChatCompletionFormat>(
            events,
            "public-model".into(),
            Some(store.clone()),
            accounting.take(),
            log_context(),
            render,
            None,
        ));
        drop(accounting);
        // Polling the source proves the spawned aggregation has taken over before cancellation.
        started.notified().await;
        handler.abort();
        let _ = handler.await;
        send_clean_events(&events_sender).await;
        drop(events_sender);
        #[expect(
            clippy::expect_used,
            reason = "detached aggregation drains and finalizes promptly"
        )]
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while recorded_rows(&store).await.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("detached aggregation finalizes");

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::ClientAborted);
        assert_eq!(row.http_status, None);
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
    }
}

/// Verifies the detached non-streaming finalization's success arm (README §3 *ApiLog*): the
/// client stays, the rendered aggregated body is handed to the handler, and the row finalizes
/// `completed` with the declared 200 and the stream's usage.
///
/// # Panics
///
/// Panics if the finalization contract diverges or the handed response is not the aggregated
/// rendering.
#[tokio::test]
async fn finalizes_completed_and_hands_the_rendered_response_to_the_handler() {
    for render in [
        AggregationRender::Ordinary,
        AggregationRender::WithPrefix {
            checkpoints: Vec::new(),
            iterations: Vec::new(),
        },
    ] {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
            None,
            render.clone(),
        );

        send_clean_events(&events_sender).await;
        drop(events_sender);

        // The task future is driven to completion first: it hands the outcome into the oneshot,
        // whose single slot buffers it for the receive below.
        task.await;

        #[expect(
            clippy::expect_used,
            reason = "the task always hands its outcome, and the fixture aggregation succeeds"
        )]
        let response = outcome_receiver
            .await
            .expect("the task always hands its outcome to the handler")
            .expect("the fixture aggregation succeeds");

        assert_eq!(response.status(), StatusCode::OK);

        #[expect(clippy::expect_used, reason = "the fixture body reads and parses")]
        let body = serde_json::from_slice::<serde_json::Value>(
            &axum::body::to_bytes(response.into_body(), MAX_BODY_BYTES)
                .await
                .expect("the fixture body reads"),
        )
        .expect("the fixture body parses");

        assert_eq!(
            body.pointer("/model"),
            Some(&serde_json::json!("public-model"))
        );
        assert_eq!(
            body.pointer("/choices/0/message/content"),
            Some(&serde_json::json!("hello"))
        );

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::Completed);
        assert_eq!(row.http_status, Some(200));
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
    }
}

/// Verifies the detached non-streaming finalization's upstream-failure arm (README §3
/// *ApiLog*): a failed stream classifies the row through the shared error taxonomy
/// regardless of the client — `upstream_error` with the transport-failure 502 both when the
/// handler receives the failure for rendering and when the handler has already departed, the
/// departure never upgrading to `client_aborted`.
///
/// # Panics
///
/// Panics if the finalization contract diverges.
#[tokio::test]
async fn finalizes_the_classified_upstream_failure_regardless_of_the_client() {
    for render in [
        AggregationRender::Ordinary,
        AggregationRender::WithPrefix {
            checkpoints: Vec::new(),
            iterations: Vec::new(),
        },
    ] {
        // The client stays: the failure is handed to the handler for rendering.
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
            None,
            render.clone(),
        );

        send_failing_events(&events_sender).await;
        drop(events_sender);

        // The task future is driven to completion first: it hands the outcome into the oneshot,
        // whose single slot buffers it for the receive below.
        task.await;

        #[expect(clippy::expect_used, reason = "the task always hands its outcome")]
        let outcome = outcome_receiver
            .await
            .expect("the task always hands its outcome to the handler");

        assert!(
            matches!(outcome, Err(Error::UpstreamUnavailable { .. })),
            "the handler receives the stream failure for rendering"
        );

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(row.http_status, Some(502));

        // The failing stream delivered no usage observation: the five token columns stay
        // `NULL`.
        assert_eq!(row.input_tokens, None);
        assert_eq!(row.output_tokens, None);
        assert_eq!(row.cached_tokens, None);
        assert_eq!(row.cache_write_tokens, None);
        assert_eq!(row.reasoning_tokens, None);

        // The client departed before the failure: the row still classifies the upstream failure
        // — never `client_aborted` — because the stream, not the client, is what
        // failed.
        let (departed_store, _departed_guard) = test_store();
        let (departed_events_sender, departed_events) = event_channel();

        let (departed_outcome_sender, departed_outcome_receiver) = oneshot::channel();

        let departed_task = aggregated_response_task::<ChatCompletionFormat>(
            departed_events,
            "public-model".to_string(),
            Some(departed_store.clone()),
            stamped_draft(),
            log_context(),
            departed_outcome_sender,
            None,
            render.clone(),
        );

        drop(departed_outcome_receiver);

        send_failing_events(&departed_events_sender).await;
        drop(departed_events_sender);

        departed_task.await;

        let departed_row = recorded_row(&departed_store).await;

        assert_eq!(departed_row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(departed_row.http_status, Some(502));

        assert_eq!(departed_row.input_tokens, None);
        assert_eq!(departed_row.output_tokens, None);
        assert_eq!(departed_row.cached_tokens, None);
        assert_eq!(departed_row.cache_write_tokens, None);
        assert_eq!(departed_row.reasoning_tokens, None);
    }
}

/// Verifies the detached non-streaming finalization records the usage a failed stream
/// delivered before its failure (README §3 *ApiLog*, §6.2's last-observation rule): an
/// upstream that reports usage and then fails mid-generation finalizes the classified
/// `upstream_error` row carrying that usage — the same attribution its streaming twin
/// records, and `NULL` columns only when the stream delivered no usage at all.
///
/// # Panics
///
/// Panics if the finalization contract diverges.
#[tokio::test]
async fn finalizes_the_usage_a_failed_stream_delivered_before_its_failure() {
    for render in [
        AggregationRender::Ordinary,
        AggregationRender::WithPrefix {
            checkpoints: Vec::new(),
            iterations: Vec::new(),
        },
    ] {
        let (store, _guard) = test_store();
        let (events_sender, events) = event_channel();

        let (outcome_sender, _outcome_receiver) = oneshot::channel();

        let task = aggregated_response_task::<ChatCompletionFormat>(
            events,
            "public-model".to_string(),
            Some(store.clone()),
            stamped_draft(),
            log_context(),
            outcome_sender,
            None,
            render.clone(),
        );

        send_failing_events_with_usage(&events_sender).await;
        drop(events_sender);

        task.await;

        let row = recorded_row(&store).await;

        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(row.http_status, Some(502));
        assert_eq!(row.input_tokens, Some(11));
        assert_eq!(row.output_tokens, Some(7));
        assert_eq!(row.cached_tokens, None);
        assert_eq!(row.cache_write_tokens, None);
        assert_eq!(row.reasoning_tokens, None);
    }
}

/// Builds one static-key provider of the Responses format with an enabled
/// model whose public id equals `upstream_id`, over a fresh test database.
///
/// # Panics
///
/// Panics if a fixture insert fails, which the fixture prevents.
async fn routed_model_database() -> (crate::database::Database, TempDbGuard) {
    let (database, guard) = create_test_database().await;

    let definition =
        insert_static_key_definition(&database, "static-key", UpstreamFormat::OpenAiResponse).await;

    let provider = insert_provider(
        &database,
        &definition,
        "mock",
        Some("https://mock.test"),
        StoredCredentials::ApiKey {
            api_key: "upstream-secret".to_string(),
        },
    )
    .await;

    #[expect(clippy::expect_used, reason = "the fixture model inserts")]
    let model = insert_model_checked(
        &database,
        ModelCreate {
            provider_id: provider.id,
            upstream_id: "test-model",
            is_enabled: true,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support_override: None,
        },
    )
    .await
    .expect("the fixture model inserts");
    #[expect(clippy::expect_used, reason = "the fixture route publishes")]
    crate::storage::routing::create_route(
        &database,
        "test-model",
        true,
        &crate::routing::ModelRouteTarget::Direct {
            model_id: model.model.id,
        },
    )
    .await
    .expect("fixture route publishes");

    #[expect(
        clippy::expect_used,
        reason = "the serving fixture explicitly grants route access"
    )]
    crate::test_support::structural_api_key(&database)
        .await
        .expect("fixture key");
    (database, guard)
}

/// The compact pipeline's pre-dispatch rejections classify through the
/// shared taxonomy exactly as the ordinary completion pipeline's: a
/// malformed body is the surface's 400, an unroutable model the 404.
///
/// # Panics
///
/// Panics if a rejection lands outside its contract classification.
#[tokio::test]
async fn compact_rejections_classify_through_the_shared_taxonomy() {
    let (database, _guard) = create_test_database().await;

    #[expect(
        clippy::expect_used,
        reason = "the fixture explicitly creates a real authenticated key"
    )]
    crate::test_support::structural_api_key(&database)
        .await
        .expect("fixture key");
    let context = context(database);

    #[expect(
        clippy::expect_used,
        reason = "the malformed body is constructed to be rejected"
    )]
    let error = run_compact(&context, b"{not json", 1)
        .await
        .expect_err("a malformed body must be rejected");

    assert!(matches!(error, Error::MalformedRequest { .. }));

    assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);

    let body = serde_json::json!({"model": "ghost", "input": "hi"});

    #[expect(
        clippy::expect_used,
        reason = "the unknown model is constructed to be rejected"
    )]
    let error = run_compact(&context, body.to_string().as_bytes(), 1)
        .await
        .expect_err("an unknown model must be rejected");

    assert!(matches!(error, Error::ModelNotFound { .. }));

    assert_eq!(error.http_status(), StatusCode::NOT_FOUND);
}

/// A parsed, routed compact request reaches the real executor: the
/// fixture's single user turn is mandatory-retained wholesale by the
/// checkpoint planner, leaving no older history to summarize, so the
/// request fails with the budget-exceeded rejection — a client fault at
/// 400 whose stable code rides the rendered message — after parse and
/// routing both succeeded, and with no dispatch, no accounting row exists
/// for it (README §3 *ApiLog*).
///
/// # Panics
///
/// Panics if the outcome diverges from the documented rejection.
#[tokio::test]
async fn a_routed_compact_request_reaches_the_executor() {
    let (database, _guard) = routed_model_database().await;

    let context = context(database);

    // The compact parse itself stays fully functional: the same valid body
    // parses and routes, which the compaction rejection — not a parse or
    // routing failure — proves.
    let body = serde_json::json!({"model": "test-model", "input": "hi"});

    #[expect(clippy::expect_used, reason = "the fixture is constructed to fail")]
    let error = run_compact(&context, body.to_string().as_bytes(), 1)
        .await
        .expect_err("the planner must reject a window with nothing to summarize");

    assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);

    assert!(
        error.to_string().contains("compaction_budget_exceeded"),
        "the stable code rides the rendered message: {error}"
    );
}

/// A routed compact request whose model routes through a non-Responses
/// definition still reaches the executor: routing is format-agnostic, and
/// the compact parse never inspects the definition.
///
/// # Panics
///
/// Panics if the outcome diverges from the placeholder contract.
#[tokio::test]
async fn compact_request_routing_is_format_agnostic() {
    let (database, _guard) = routed_model_database().await;

    let context = context(database);

    // An item-array input with state parses exactly as on the ordinary
    // endpoint before routing: the state vocabulary is shared.
    let body = serde_json::json!({
        "model": "test-model",
        "input": [
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "compaction_trigger"},
        ],
    });

    // The trigger is valid on the ordinary endpoint, but the compact
    // endpoint rejects it before routing — an ambiguity beside the route's
    // own standalone control.
    #[expect(
        clippy::expect_used,
        reason = "the trigger body is constructed to be rejected"
    )]
    let error = run_compact(&context, body.to_string().as_bytes(), 1)
        .await
        .expect_err("a trigger on the compact endpoint must be rejected");

    assert!(matches!(error, Error::MalformedRequest { .. }));

    assert_eq!(error.http_status(), StatusCode::BAD_REQUEST);
}
/// Cancellation during internal compaction finalizes one row with the latest billed usage.
///
/// # Panics
///
/// Panics if the asynchronous finalization does not produce the expected row.
#[tokio::test]
async fn compaction_cancellation_preserves_billed_work() {
    let (store, _directory) = test_store();
    let mut draft = stamped_draft();
    draft.stamp_billed_usage(Some(IrUsage {
        input_tokens: 20,
        output_tokens: 5,
        ..IrUsage::default()
    }));
    drop(ApiLogGuard::new(Some(store.clone()), draft));
    #[expect(clippy::expect_used, reason = "a dispatched draft finalizes promptly")]
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while recorded_rows(&store).await.is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("cancellation row");
    let row = recorded_row(&store).await;
    assert_eq!(row.outcome, ApiLogOutcome::ClientAborted);
    assert_eq!(row.http_status, None);
    assert_eq!(row.input_tokens, Some(20));
    assert_eq!(row.output_tokens, Some(5));
}

/// Native iteration reports replace top-level usage on both aggregated and streaming paths.
///
/// # Panics
///
/// Panics if either response path double-counts or loses the reported compaction work.
#[tokio::test]
async fn native_compaction_iterations_are_accounted_once_on_both_response_paths() {
    let iterations =
        vec![serde_json::json!({"type":"compaction","input_tokens":20,"output_tokens":5})];
    for is_streaming in [false, true] {
        let (store, _directory) = test_store();
        let events: IrEventStream = Box::pin(futures::stream::iter(vec![
            Ok(IrEvent::MessageStart {
                id: "native".into(),
                model: "public".into(),
            }),
            Ok(IrEvent::Usage(IrUsage::default())),
            Ok(IrEvent::CompactionMetadata {
                is_paused: true,
                iterations: iterations.clone().into_iter().map(Into::into).collect(),
            }),
            Ok(IrEvent::finish(IrFinishReason::Stop)),
            Ok(IrEvent::Done),
        ]));
        if is_streaming {
            let response = sse_response(
                events,
                Box::new(crate::format::anthropic::messages::SseRenderer::new(
                    "public".into(),
                )),
                Some(store.clone()),
                stamped_draft(),
                None,
            );
            #[expect(clippy::expect_used, reason = "the healthy fixture stream reads")]
            let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .expect("stream");
        } else {
            let (sender, receiver) = oneshot::channel();
            aggregated_response_task::<crate::format::anthropic::messages::Format>(
                events,
                "public".into(),
                Some(store.clone()),
                stamped_draft(),
                log_context(),
                sender,
                None,
                AggregationRender::Ordinary,
            )
            .await;
            #[expect(
                clippy::expect_used,
                reason = "the healthy fixture response is handed off"
            )]
            let _ = receiver
                .await
                .expect("response")
                .expect("successful response");
        }
        #[expect(
            clippy::expect_used,
            reason = "a completed response finalizes promptly"
        )]
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while recorded_rows(&store).await.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("completed row");
        let row = recorded_row(&store).await;
        assert_eq!(row.input_tokens, Some(20));
        assert_eq!(row.output_tokens, Some(5));
    }
}
/// Failure before and after response ownership transfer records exactly one compaction row.
///
/// # Panics
///
/// Panics if the coordinator loses billed work or duplicates a failed request's row.
#[tokio::test]
async fn compaction_failures_finalize_once_before_and_after_response_transfer() {
    for should_continue in [false, true] {
        let (database, _database_guard) = routed_model_database().await;
        let (store, _directory) = test_store();
        let content = if should_continue { "summary" } else { "" };
        let body = serde_json::json!({"id":"native",
"model":"test-model",
"status":"completed",
"output":[{"type":"message",
"role":"assistant",
"content":[{"type":"output_text",
"text":content}]}],
"usage":{"input_tokens":20,
"output_tokens":5}});
        let mut actions = vec![crate::test_support::MockAction::respond_json(
            body.to_string(),
        )];
        if should_continue {
            actions.push(crate::test_support::MockAction::respond_json(
                "malformed JSON".into(),
            ));
        }
        let (mock, _requests) =
            crate::test_support::MockUpstream::start_scripted_recording_bodies(actions).await;
        let mut context = context(database);
        context.api_log_store = Some(store.clone());
        #[expect(
            clippy::expect_used,
            reason = "the fixture routing and compaction settings exist"
        )]
        let mut routing = gateway::resolve_routing(&context.database, "test-model")
            .await
            .expect("routing");
        routing.provider.base_url = Some(mock.base());
        #[expect(
            clippy::expect_used,
            reason = "the fixture declares valid compaction settings"
        )]
        {
            routing.provider.definition.payload.compaction = Some(
                serde_json::from_value(
                    serde_json::json!({"strategy":"gateway_summary","retention_budget":1}),
                )
                .expect("section"),
            );
        }
        let request = serde_json::json!({
            "model": "test-model",
            "input": [
                {"type": "message", "role": "user", "content": "old history"},
                {"type": "message", "role": "user", "content": "pending"}
            ],
            "context_management": [{"type": "compaction", "compact_threshold": 1}]
        });
        #[expect(clippy::expect_used, reason = "the automatic request is valid")]
        let parsed = crate::format::openai::response::Format::parse_request(
            request.to_string().as_bytes(),
            &RouteContext::empty(),
        )
        .expect("request");
        #[expect(clippy::expect_used, reason = "the fixture's state authenticates")]
        let preparation = prepare_compaction_boundaries(&parsed, "completion")
            .expect("authenticated compaction state");
        let outcome = run_compaction_pipeline::<crate::format::openai::response::Format>(
            &context,
            parsed,
            preparation,
            gateway::CompactionSelection::direct(routing),
            stamped_draft(),
            log_context(),
            false,
            true,
            "test-model".into(),
            OperationKind::Completion,
        )
        .await;
        assert!(outcome.is_err());
        let row = recorded_row(&store).await;
        assert_eq!(row.outcome, ApiLogOutcome::UpstreamError);
        assert_eq!(row.input_tokens, Some(20));
        assert_eq!(row.output_tokens, Some(5));
    }
}

/// Completed JSON output becomes a history anchor before handoff; known departures and
/// failed bodies retain only their request retry hint, never a completed output anchor.
/// Namespace deletion fences the detached task's later callback as well.
///
/// # Errors
///
/// Returns fixture parser, channel, or task failures.
///
/// # Panics
///
/// Panics if result ownership or completion/disconnect anchoring diverges.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "affinity fixtures serialize configuration"
)]
async fn json_history_registration_obeys_completion_and_owned_task_lifetime()
-> Result<(), Box<dyn std::error::Error>> {
    let _configuration = crate::test_support::configuration_lock();
    crate::test_support::install_global_configuration();
    for (is_departed, has_failure, is_deleted) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let cache = std::sync::Arc::new(crate::affinity::Cache::default());
        let namespace = crate::affinity::Namespace {
            api_key_id: 3,
            pool_id: 11,
        };
        let original = ChatCompletionFormat::parse_request(
            br#"{
                "model":"M","messages":[{"role":"user","content":"input"}]}
                "#,
            &RouteContext::empty(),
        )?;
        let projector = original.affinity.project(&original.ir.messages);
        let evidence = projector.evidence();
        #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
        let crate::affinity::Decision::Fresh(lease) = cache.acquire(namespace, &evidence) else {
            panic!("new request establishment");
        };
        let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
        let (sender, events) = event_channel();
        let (outcome_sender, outcome_receiver) = oneshot::channel();
        let task = tokio::spawn(aggregated_response_task::<ChatCompletionFormat>(
            events,
            "M".into(),
            None,
            stamped_draft(),
            log_context(),
            outcome_sender,
            Some(affinity),
            AggregationRender::Ordinary,
        ));
        let outcome_receiver = if is_departed {
            drop(outcome_receiver);
            None
        } else {
            Some(outcome_receiver)
        };
        if is_deleted {
            cache.remove_api_key(namespace.api_key_id);
        }
        if has_failure {
            send_events(
                &sender,
                [
                    Ok(IrEvent::TextDelta("hello".into())),
                    Err(gateway::Error::Unavailable {
                        detail: "failed".into(),
                    }),
                    Ok(IrEvent::Done),
                ],
            )
            .await;
        } else {
            send_clean_events(&sender).await;
        }
        drop(sender);
        if let Some(receiver) = outcome_receiver {
            let response = receiver.await?;
            assert_eq!(response.is_err(), has_failure);
            if let Ok(response) = response {
                let body = axum::body::to_bytes(response.into_body(), 262_144).await?;
                let body: serde_json::Value = serde_json::from_slice(&body)?;
                assert_eq!(
                    body.pointer("/choices/0/message/content"),
                    Some(&serde_json::Value::String("hello".into()))
                );
            }
        }
        task.await?;
        let replay = ChatCompletionFormat::parse_request(
            br#"{
                "model":"changed-public-name","messages":[
                    {"role":"user","content":"input"},
                    {"role":"assistant","content":"hello"},
                    {"role":"user","content":"next"}]}
                "#,
            &RouteContext::empty(),
        )?;
        let replay = replay.affinity.project(&replay.ir.messages).evidence();
        match cache.acquire(namespace, &replay) {
            crate::affinity::Decision::Bound(binding) => {
                assert!(!is_departed && !has_failure && !is_deleted);
                assert_eq!(binding.provider_id(), 5);
            }
            crate::affinity::Decision::Fresh(_) => {
                assert!(is_departed || has_failure || is_deleted);
            }
            #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
            _ => panic!("replay is an established completed output or a fresh miss"),
        }
        if !is_deleted {
            #[expect(clippy::panic, reason = "a wrong decision is the test failure report")]
            let crate::affinity::Decision::Bound(binding) = cache.acquire(namespace, &evidence)
            else {
                panic!("accepted headers retain their independent retry hint");
            };
            assert_eq!(binding.provider_id(), 5);
        }
    }
    Ok(())
}

/// Prepared prefixes and live native checkpoints register at their block stop and survive a
/// later failure, and suppress hints after an already known departure or namespace
/// deletion.
///
/// # Errors
///
/// Returns fixture parser, channel, or response-body failures.
///
/// # Panics
///
/// Panics if the fixture lacks its artifact or violates delivery ownership.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "affinity fixtures serialize configuration"
)]
async fn prepared_checkpoint_registration_obeys_block_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::format::anthropic::messages::SseRenderer as MessagesRenderer;
    let _configuration = crate::test_support::configuration_lock();
    crate::test_support::install_global_configuration();
    for client_format in [
        "messages",
        "messages_checkpoint",
        "responses_checkpoint",
        "messages_native",
    ] {
        for is_threshold in [false, true] {
            for (has_failure, is_departed, is_deleted) in [
                (false, false, false),
                (true, false, false),
                (false, true, false),
                (false, false, true),
            ] {
                let cache = std::sync::Arc::new(crate::affinity::Cache::default());
                let namespace = crate::affinity::Namespace {
                    api_key_id: 3,
                    pool_id: 11,
                };
                let original = ChatCompletionFormat::parse_request(
                    br#"{"model":"M","messages":[{"role":"user","content":"input"}]}"#,
                    &RouteContext::empty(),
                )?;
                let projector = original.affinity.project(&original.ir.messages);
                #[expect(
                    clippy::panic,
                    reason = "unexpected fixture establishment fails the test"
                )]
                let crate::affinity::Decision::Fresh(lease) =
                    cache.acquire(namespace, &projector.evidence())
                else {
                    panic!("fresh prefix fixture");
                };
                let block = if client_format == "responses_checkpoint" {
                    serde_json::json!({"type":"compaction","encrypted_content":"native state"})
                } else {
                    serde_json::json!({"type":"compaction","content":"summary",
                        "signature":"native signature","tool_changes":[{"z":2,"a":1}]})
                };
                #[expect(
                    clippy::expect_used,
                    reason = "the fixture uses accepted checkpoint fields"
                )]
                let (window, anchors) = if client_format == "responses_checkpoint" {
                    let window = crate::format::openai::response::gateway_checkpoint_projection(
                        std::slice::from_ref(&block.clone().into()),
                    )
                    .expect("accepted Responses state");
                    let mut anchors = window.completed_anchors();
                    anchors.push(crate::affinity::Anchor::Artifact(
                        window.native_artifact(&block),
                    ));
                    (window, anchors)
                } else {
                    crate::format::anthropic::messages::completed_compaction_projection(
                        &block.clone().into(),
                        None,
                    )
                    .expect("accepted Messages state")
                };
                #[expect(
                    clippy::expect_used,
                    reason = "the valid fixture block supplies its artifact"
                )]
                let artifact = anchors
                    .iter()
                    .find_map(|anchor| match anchor {
                        crate::affinity::Anchor::Artifact(digest) => Some(*digest),
                        crate::affinity::Anchor::History(_) => None,
                    })
                    .expect("native artifact");
                let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
                let native_payload = block.clone();
                let affinity = if client_format == "messages_native" {
                    affinity
                } else if client_format == "messages" {
                    affinity.completed_prefix(anchors)
                } else {
                    affinity.completed_checkpoint(window, anchors)
                };
                let (sender, events) = event_channel();
                let renderer: Box<dyn ClientSseRenderer + Send> = if client_format
                    == "responses_checkpoint"
                {
                    Box::new(
                        crate::format::openai::response::SseRenderer::new("M".into())
                            .with_prefix_items(vec![block.into()]),
                    )
                } else if client_format == "messages_native" {
                    Box::new(
                        MessagesRenderer::new("M".into()).with_threshold_compaction(is_threshold),
                    )
                } else {
                    Box::new(
                        MessagesRenderer::with_compaction_prefix(
                            "M".into(),
                            vec![block.into()],
                            Some("compaction".into()),
                        )
                        .with_threshold_compaction(is_threshold),
                    )
                };
                let response =
                    sse_response(events, renderer, None, stamped_draft(), Some(affinity));
                let mut body = response.into_body().into_data_stream();
                if is_deleted {
                    cache.remove_api_key(namespace.api_key_id);
                }
                if is_departed {
                    drop(body);
                    send_events(
                        &sender,
                        [
                            Ok(IrEvent::MessageStart {
                                id: "upstream".into(),
                                model: "physical".into(),
                            }),
                            Ok(if client_format == "messages_native" {
                                IrEvent::CompactionState {
                                    payload: native_payload.clone(),
                                }
                            } else {
                                IrEvent::Done
                            }),
                            Ok(IrEvent::Done),
                        ],
                    )
                    .await;
                    drop(sender);
                    tokio::task::yield_now().await;
                } else {
                    send_events(
                        &sender,
                        [
                            Ok(IrEvent::MessageStart {
                                id: "upstream".into(),
                                model: "physical".into(),
                            }),
                            Ok(IrEvent::Usage(IrUsage::default())),
                            Ok(if client_format == "messages_native" {
                                IrEvent::CompactionState {
                                    payload: native_payload.clone(),
                                }
                            } else {
                                IrEvent::Usage(IrUsage::default())
                            }),
                        ],
                    )
                    .await;
                    loop {
                        #[expect(
                            clippy::expect_used,
                            reason = "the fixture emits a complete prefix"
                        )]
                        let frame = body.next().await.expect("prefix frame")?;
                        let boundary = if client_format == "responses_checkpoint" {
                            "event: response.output_item.done"
                        } else {
                            "event: content_block_stop"
                        };
                        if std::str::from_utf8(&frame)?.contains(boundary) {
                            break;
                        }
                    }
                    if has_failure {
                        send_events(
                            &sender,
                            [Err(gateway::Error::Unavailable {
                                detail: "later failure".into(),
                            })],
                        )
                        .await;
                    } else {
                        send_events(&sender, [Ok(IrEvent::Done)]).await;
                    }
                    drop(sender);
                    while let Some(frame) = body.next().await {
                        frame?;
                    }
                }
                let evidence = crate::affinity::Evidence {
                    artifact: Some(artifact),
                    ..crate::affinity::Evidence::default()
                };
                match cache.acquire(namespace, &evidence) {
                    crate::affinity::Decision::Bound(binding) => {
                        assert!(!is_departed && !is_deleted);
                        assert_eq!(binding.provider_id(), 5);
                    }
                    crate::affinity::Decision::Fresh(_) => assert!(is_departed || is_deleted),
                    #[expect(
                        clippy::panic,
                        reason = "an unexpected fixture decision fails the test"
                    )]
                    _ => panic!("unexpected prefix affinity decision"),
                }
            }
        }
    }
    Ok(())
}

/// Streaming completion registers actual outward history at its terminal frame, while an
/// upstream error, known disconnect, or namespace deletion cannot create that output hint.
///
/// # Errors
///
/// Returns fixture parser, channel, or response-body failures.
///
/// # Panics
///
/// Panics if terminal commitment or known cancellation changes affinity ownership.
#[tokio::test]
#[expect(
    clippy::await_holding_lock,
    reason = "affinity fixtures serialize configuration"
)]
async fn streamed_history_registration_obeys_terminal_delivery()
-> Result<(), Box<dyn std::error::Error>> {
    let _configuration = crate::test_support::configuration_lock();
    crate::test_support::install_global_configuration();
    for client_format in ["chat", "generate", "responses", "messages", "interactions"] {
        for (has_failure, is_departed, is_deleted) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            let (store, _directory) = test_store();
            let cache = std::sync::Arc::new(crate::affinity::Cache::default());
            let namespace = crate::affinity::Namespace {
                api_key_id: 3,
                pool_id: 11,
            };
            let original = ChatCompletionFormat::parse_request(
                br#"{
                "model":"M","messages":[{"role":"user","content":"input"}]}"#,
                &RouteContext::empty(),
            )?;
            let projector = original.affinity.project(&original.ir.messages);
            let evidence = projector.evidence();
            #[expect(
                clippy::panic,
                reason = "an unexpected decision is the test failure report"
            )]
            let crate::affinity::Decision::Fresh(lease) = cache.acquire(namespace, &evidence)
            else {
                panic!("new request establishment");
            };
            let native_item = serde_json::json!({"type":"compaction","id":"cmp_complete",
                    "encrypted_content":"native state"});
            let artifact = projector.native_artifact(&native_item);
            let affinity = crate::affinity::Accepted::new(lease.commit(5), projector);
            let (sender, events) = event_channel();
            let response = sse_response(
                events,
                match client_format {
                    "generate" => crate::format::gemini::generate_content::Format::sse_renderer(
                        false,
                        "M".into(),
                    ),
                    "responses" => {
                        crate::format::openai::response::Format::sse_renderer(false, "M".into())
                    }
                    "messages" => {
                        crate::format::anthropic::messages::Format::sse_renderer(false, "M".into())
                    }
                    "interactions" => {
                        crate::format::gemini::interaction::Format::sse_renderer(false, "M".into())
                    }
                    _ => ChatCompletionFormat::sse_renderer(true, "M".into()),
                },
                Some(store.clone()),
                stamped_draft(),
                Some(affinity),
            );
            send_events(
                &sender,
                [
                    Ok(IrEvent::MessageStart {
                        id: "upstream".into(),
                        model: "physical-model".into(),
                    }),
                    Ok(if client_format == "responses" {
                        IrEvent::Usage(IrUsage::default())
                    } else {
                        IrEvent::TextDelta("hello".into())
                    }),
                ],
            )
            .await;
            let mut body = response.into_body().into_data_stream();
            for _ in 0..match client_format {
                "generate" => 1,
                "responses" => 2,
                "messages" | "interactions" => 3,
                _ => 2,
            } {
                #[expect(
                    clippy::expect_used,
                    reason = "the fixture emits its role and text frames"
                )]
                body.next().await.expect("intermediate streamed frame")?;
            }
            if is_deleted {
                cache.remove_api_key(namespace.api_key_id);
            }
            if is_departed {
                drop(body);
                if client_format == "responses" {
                    send_events(
                        &sender,
                        [Ok(IrEvent::CompactionState {
                            payload: native_item.clone(),
                        })],
                    )
                    .await;
                }
                send_events(&sender, [Ok(IrEvent::Done)]).await;
                drop(sender);
                // Let the owned renderer observe its closed client channel and release
                // metadata.
                tokio::task::yield_now().await;
            } else {
                if client_format == "responses" {
                    send_events(
                        &sender,
                        [
                            Ok(IrEvent::CompactionState {
                                payload: native_item.clone(),
                            }),
                            Ok(IrEvent::TextDelta("hello".into())),
                        ],
                    )
                    .await;
                }
                if has_failure {
                    send_events(
                        &sender,
                        [Err(gateway::Error::Unavailable {
                            detail: "failed".into(),
                        })],
                    )
                    .await;
                } else {
                    send_events(&sender, [Ok(IrEvent::finish(IrFinishReason::Stop))]).await;
                }
                send_events(&sender, [Ok(IrEvent::Done)]).await;
                drop(sender);
                while let Some(frame) = body.next().await {
                    frame?;
                }
            }
            let row = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Some(row) = recorded_rows(&store).await.into_iter().next() {
                        break row;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await?;
            assert_eq!(
                row.outcome,
                if is_departed {
                    ApiLogOutcome::ClientAborted
                } else if has_failure {
                    ApiLogOutcome::UpstreamError
                } else {
                    ApiLogOutcome::Completed
                }
            );
            assert_eq!(row.http_status, Some(200));
            let replay = ChatCompletionFormat::parse_request(
                br#"{
                "model":"renamed","messages":[{"role":"user","content":"input"},
                    {"role":"assistant","content":"hello"},{"role":"user","content":"next"}]}"#,
                &RouteContext::empty(),
            )?;
            let replay = if client_format == "responses" {
                let replay = crate::format::openai::response::Format::parse_request(
                    serde_json::json!({"model":"renamed","input":[
                        native_item,
                        {"type":"message","role":"assistant","content":"hello"},
                        {"type":"message","role":"user","content":"next"}
                    ]})
                    .to_string()
                    .as_bytes(),
                    &RouteContext::empty(),
                )?;
                #[expect(
                    clippy::expect_used,
                    reason = "the fixture contains its native boundary"
                )]
                let compaction = replay.compaction.as_ref().expect("native replay boundary");
                let active =
                    crate::compaction::history::BoundaryPreparation::authenticate(compaction)?
                        .routing_preparation(&replay)?;
                // Probe only the completed state-plus-answer unit count. The independently
                // released checkpoint remains usable after failure, but cannot establish an
                // unfinished answer's full window.
                crate::affinity::Evidence {
                    history: active
                        .projector
                        .evidence()
                        .history
                        .into_iter()
                        .filter(|prefix| prefix.units.get() == 2)
                        .collect(),
                    ..crate::affinity::Evidence::default()
                }
            } else {
                replay.affinity.project(&replay.ir.messages).evidence()
            };
            match cache.acquire(namespace, &replay) {
                crate::affinity::Decision::Bound(binding) => {
                    assert!(!has_failure && !is_departed && !is_deleted);
                    assert_eq!(binding.provider_id(), 5);
                }
                crate::affinity::Decision::Fresh(_) => {
                    assert!(has_failure || is_departed || is_deleted);
                }
                #[expect(
                    clippy::panic,
                    reason = "an unexpected decision is the test failure report"
                )]
                _ => panic!("completed output binding or a fresh history miss"),
            }
            if client_format == "responses" {
                match cache.acquire(
                    namespace,
                    &crate::affinity::Evidence {
                        artifact: Some(artifact),
                        ..crate::affinity::Evidence::default()
                    },
                ) {
                    crate::affinity::Decision::Bound(binding) => {
                        assert!(!is_departed && !is_deleted);
                        assert_eq!(binding.provider_id(), 5);
                    }
                    crate::affinity::Decision::Fresh(_) => assert!(is_departed || is_deleted),
                    #[expect(
                        clippy::panic,
                        reason = "an unexpected decision is the test failure report"
                    )]
                    _ => panic!("complete released artifact survives a later stream failure"),
                }
            }
            if !is_deleted {
                #[expect(
                    clippy::panic,
                    reason = "an unexpected decision is the test failure report"
                )]
                let crate::affinity::Decision::Bound(binding) = cache.acquire(namespace, &evidence)
                else {
                    panic!("accepted headers retain their independent retry hint");
                };
                assert_eq!(binding.provider_id(), 5);
            }
        }
    }
    Ok(())
}
