//! Compaction-executor tests: the decision order end to end against a real mock upstream —
//! gateway summary sealing, native passthrough and its adapters, classified-unsupported
//! fallback, threshold emulation, replay restoration, and the failure paths that must not
//! fabricate artifacts or fallbacks.
//!
//! Every scenario holds the shared test configuration's lock: the executor seals carriers under
//! the live configuration's keys while the mock upstream serves the recorded dispatches.

#![expect(
    clippy::await_holding_lock,
    clippy::missing_panics_doc,
    reason = "every scenario deliberately holds the shared test configuration's lock across \
              its awaits: the executor seals carriers under the live configuration's keys \
              while the mock upstream serves the recorded dispatches, and the tests' \
              assertions panic; a # Panics section on every test would restate that"
)]

use super::{MessagesOuterOutcome, ResponsesOuterOutcome};
use crate::{
    accounting::ApiLogEndpointType,
    api::Error as ApiError,
    compaction::history::RestoredBoundary,
    compaction_model::{
        boundary::{ActiveBoundary, BoundaryPreparation},
        envelope::{
            EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
            NativeWindowPayload, ReplacementMode, RetainedUnit,
        },
        mechanism::{Mechanism, MechanismSupport},
        sealing,
        support_facts::CompactionSupportFacts,
    },
    context::Context,
    format::{
        BoundaryPlacement, ClientFormat, CompactionBoundary, NativeArtifact, ParsedCompaction,
        ParsedRequest, RouteContext, UpstreamFormat,
        anthropic::messages as messages_format,
        compaction_wire::Checkpoint,
        openai::response::{self as responses_format, parse_compact_request},
    },
    ir::{IrFinishReason, IrMessage, IrPart, IrRole},
    provider::{
        DiscoveryState, Model, ModelRouting, Provider, StoredCredentials,
        definition::{
            DefinitionPayload, ProviderDefinition,
            compaction::CompactionSection,
            template::{ParamRow, StringExpr, ValueExpr},
        },
        quota::coordinator::Coordinator,
    },
    storage::api_logs::{ApiLogDraft, ApiLogStore},
    test_support::{
        self,
        mock_upstream::{MockAction, MockUpstream, RecordedRequest, RecordedRequests},
    },
};
use serde_json::Value;
use std::{error::Error as StdError, num::NonZero, sync::Arc};

/// The on-demand compaction beta of the inbound Messages protocol.
const ON_DEMAND_BETA: &str = "compact-2026-09-04";

/// The threshold compaction beta of the inbound Messages protocol.
const THRESHOLD_BETA: &str = "compact-2026-01-12";

/// The upstream summary answer of the Responses wire: one completed text message and full usage.
fn responses_summary_body() -> Value {
    serde_json::json!({
        "id": "resp_upstream",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "model": "upstream-model",
        "output": [
            {"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": "Gateway summary text."}
            ]}
        ],
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

/// The upstream summary answer of the Messages wire.
fn messages_summary_body() -> Value {
    serde_json::json!({
        "id": "msg_upstream",
        "type": "message",
        "role": "assistant",
        "model": "upstream-model",
        "content": [{"type": "text", "text": "Gateway summary text."}],
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

/// The upstream Messages answer carrying one complete signed native compaction block.
fn messages_compaction_block_body() -> Value {
    serde_json::json!({
        "id": "msg_compact",
        "type": "message",
        "role": "assistant",
        "model": "upstream-model",
        "content": [
            {"type": "compaction", "content": "Native block text.", "signature": "native-signature"}
        ],
        "stop_reason": "compaction",
        "usage": {"input_tokens": 10, "output_tokens": 5}
    })
}

/// The upstream answer of a native standalone compact dispatch: one compaction item beside
/// usage.
fn responses_compact_body() -> Value {
    serde_json::json!({
        "id": "resp_compact",
        "object": "response",
        "created_at": 1,
        "status": "completed",
        "model": "upstream-model",
        "output": [{"type": "compaction", "id": "c_1", "encrypted_content": "native-ciphertext"}],
        "usage": {"input_tokens": 7, "output_tokens": 3}
    })
}

/// The complete raw non-success response of the given status line and JSON body.
fn rejected(status_line: &str, body: &Value) -> MockAction {
    let body = body.to_string();

    MockAction::Respond(format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    ))
}

/// The offline harness: the installed shared configuration, a temporary database and api-log
/// directory, a mock upstream serving the scripted actions, and a Responses or Messages routing
/// over the mock. `compaction` builds the definition's compaction section JSON from the mock's
/// base URL, so an operation URL can address the mock; `Value::Null` declares no section.
struct Harness {
    /// The request context the executor entry points take.
    context: Context,

    /// The single physical route every scenario dispatches through.
    routing: ModelRouting,

    /// The temporary database guard, dropped with the harness.
    _database_guard: crate::test_support::TempDatabaseGuard,

    /// The mock upstream, shut down when the harness drops.
    _upstream: MockUpstream,

    /// The requests the mock recorded, in dispatch order.
    requests: RecordedRequests,
}

/// Builds the harness for one scenario.
async fn harness(
    format: UpstreamFormat,
    compaction: impl FnOnce(&str) -> Value,
    max_input_tokens: Option<i64>,
    actions: Vec<MockAction>,
) -> Result<Harness, Box<dyn StdError>> {
    test_support::install_configuration();

    let (database, database_guard) = test_support::temp_database().await;

    let (upstream, requests) = MockUpstream::start(actions).await;

    let inference_path = match format {
        UpstreamFormat::OpenAiResponse => "/v1/responses",
        UpstreamFormat::AnthropicMessages => "/v1/messages",
        _ => return Err("the fixture formats are Responses and Messages only".into()),
    };

    let payload = serde_json::json!({
        "format": format.as_str(),
        "inference": {
            "url": [{"type": "literal",
                "value": format!("{}{inference_path}", upstream.base())}],
            "upstream_streaming": "match_client"
        },
        "credential": {"type": "api_key"},
        "compaction": compaction(&upstream.base())
    });

    let section = serde_json::from_value(
        payload
            .get("compaction")
            .ok_or("the fixture carries its compaction section")?
            .clone(),
    )?;

    let definition = DefinitionPayload {
        format,
        inference: serde_json::from_value(
            payload
                .get("inference")
                .ok_or("the fixture carries its inference")?
                .clone(),
        )?,
        credential: serde_json::from_value(
            payload
                .get("credential")
                .ok_or("the fixture carries its credential")?
                .clone(),
        )?,
        provisioning: None,
        discovery: None,
        quota: None,
        compaction: section,
    };

    let routing = ModelRouting {
        model_route_id: 1,
        public_id: String::from("public-model"),
        provider: Provider {
            id: 7,
            name: String::from("mock"),
            model_prefix: String::new(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: String::from("test-key"),
            },
            is_enabled: true,
            quota_generation: 0,
            quota_revision: 0,
            definition: ProviderDefinition {
                name: String::from("mock-definition"),
                version: NonZero::new(1).ok_or("the fixture version is nonzero")?,
                payload: definition,
                created_at: 1_000,
            },
        },
        model: Model {
            id: 9,
            upstream_id: String::from("upstream-model"),
            is_enabled: true,
            discovery_state: DiscoveryState::Discovered,
            thinking_levels: None,
            max_input_tokens,
            max_output_tokens: None,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support: None,
            compaction_support_override: None,
        },
    };

    let api_logs_directory = std::env::temp_dir().join(format!(
        "janusgate-test-executor-logs-{}",
        rand::random::<u64>()
    ));

    let context = Context {
        database,
        api_log_store: ApiLogStore::new(
            api_logs_directory,
            NonZero::new(1).ok_or("the fixture retention is nonzero")?,
        ),
        data_plane: reqwest::Client::new(),
        control_plane: reqwest::Client::new(),
        quota_coordinator: Arc::new(Coordinator::default()),
        affinity_cache: Arc::new(crate::affinity::Cache::default()),
    };

    Ok(Harness {
        context,
        routing,
        _database_guard: database_guard,
        _upstream: upstream,
        requests,
    })
}

/// The cloned request list the mock recorded so far.
fn recorded(requests: &RecordedRequests) -> Result<Vec<RecordedRequest>, Box<dyn StdError>> {
    let guard = requests
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    Ok(guard.clone())
}

/// Parses a Responses completion request body through the client codec.
fn responses_request(body: &Value) -> Result<ParsedRequest, Box<dyn StdError>> {
    Ok(responses_format::Format::parse_request(
        serde_json::to_vec(body)?.as_slice(),
        &RouteContext::empty(),
    )?)
}

/// Parses a Messages completion request body through the client codec under the given inbound
/// beta header entries.
fn messages_request(body: &Value, betas: &[&str]) -> Result<ParsedRequest, Box<dyn StdError>> {
    Ok(messages_format::Format::parse_request(
        serde_json::to_vec(body)?.as_slice(),
        &RouteContext {
            anthropic_betas: betas.iter().map(|beta| String::from(*beta)).collect(),
            ..RouteContext::default()
        },
    )?)
}

/// Parses a Responses standalone compact request body through the compact codec.
fn compact_request(body: &Value) -> Result<ParsedRequest, Box<dyn StdError>> {
    Ok(parse_compact_request(serde_json::to_vec(body)?.as_slice())?)
}

/// Authenticates the parsed request's compaction facts the way the serving boundary does before
/// account selection.
fn preparation(parsed: &ParsedRequest) -> Result<BoundaryPreparation, Box<dyn StdError>> {
    match parsed.compaction.as_ref() {
        Some(compaction) => Ok(BoundaryPreparation::authenticate(compaction)?),
        None => Ok(BoundaryPreparation::default()),
    }
}

/// The accounting draft of one fixture attempt.
fn log_draft() -> ApiLogDraft {
    ApiLogDraft::predispatch(1, 7, 9, 1, ApiLogEndpointType::OpenaiResponses, false)
}

/// A user-text Responses input item.
fn responses_user_input(text: &str) -> Value {
    serde_json::json!({
        "type": "message",
        "role": "user",
        "content": [{"type": "input_text", "text": text}]
    })
}

/// A user-text Messages message.
fn messages_user_message(text: &str) -> Value {
    serde_json::json!({"role": "user", "content": text})
}

/// Serializes a checkpoint into its wire value.
fn checkpoint_value(checkpoint: &Checkpoint) -> Result<Value, Box<dyn StdError>> {
    Ok(serde_json::to_value(checkpoint)?)
}

/// A sealed gateway summary carrier under the live configuration, for replay fixtures.
fn sealed_summary_carrier(summary_text: &str) -> Result<String, Box<dyn StdError>> {
    let envelope = EnvelopeV1::new(
        String::from("cmp_replay"),
        ReplacementMode::HistoricalCheckpoint,
        EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
            summary_text: String::from(summary_text),
            preserved_instructions: Vec::new(),
            retained_units: vec![RetainedUnit {
                messages: vec![IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("kept"),
                    }],
                }],
            }],
            retained_state_items: Vec::new(),
            overlap_identities: Vec::new(),
        }),
        None,
    );

    Ok(sealing::seal_with_live_config(&envelope, None)?)
}

/// A sealed wrapped-native-window carrier under the live configuration, for producer
/// compatibility fixtures.
fn sealed_native_carrier(producer: &str, model: &str) -> Result<String, Box<dyn StdError>> {
    let envelope = EnvelopeV1::new(
        String::from("cmp_native"),
        ReplacementMode::CanonicalWindow,
        EnvelopePayload::NativeWindow(NativeWindowPayload {
            producer_instance_id: String::from(producer),
            upstream_model_id: String::from(model),
            mechanism: Mechanism::ResponsesStandaloneCompact,
            native_format: UpstreamFormat::OpenAiResponse,
            required_prefix_identity: None,
            retained_messages: Vec::new(),
            retained_state_items: Vec::new(),
            window: NativeReplayWindow::ResponsesWindow {
                items: vec![serde_json::json!({"type": "compaction"})],
            },
        }),
        None,
    );

    Ok(sealing::seal_with_live_config(&envelope, None)?)
}

/// The compaction section declaring the standalone compact operation at the given URL, with the
/// given operation-level unsupported classifier. The retention budget collapses to one token so a
/// summary fallback over the fixtures' small histories always has an older prefix to summarize.
fn standalone_section(base: &str, unsupported: Value) -> Value {
    serde_json::json!({
        "strategy": "native_preferred",
        "retention_budget": 1,
        "mechanisms": {
            "responses_standalone_compact": {
                "default": "supported",
                "operation": {
                    "url": [{"type": "literal",
                        "value": format!("{base}/v1/responses/compact")}],
                    "unsupported": unsupported
                }
            }
        }
    })
}

/// The compact endpoint's gateway-summary flow: with no native operation the executor
/// summarizes through one internal dispatch, renders the `response.compaction` body with the
/// sealed carrier, stamps the ledger's billed totals onto the accounting draft, and the emitted
/// carrier unseals back to the summary through the boundary consumer.
#[tokio::test]
async fn compact_gateway_summary_seals_and_stamps_usage() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| serde_json::json!({"retention_budget": 1}),
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = compact_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello world")]
    }))?;

    let mut log = log_draft();

    let body = super::execute_responses_compact(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let rendered = serde_json::to_value(&body)?;

    assert_eq!(
        crate::test_support::at(&rendered, "/object"),
        &serde_json::json!("response.compaction")
    );

    let carrier = crate::test_support::at(&rendered, "/output/0/encrypted_content")
        .as_str()
        .ok_or("the carrier rides the output item")?
        .to_string();

    assert!(
        carrier.starts_with("janusgate.compaction.v1."),
        "the gateway carrier is sealed under the gateway namespace"
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/usage/input_tokens"),
        &serde_json::json!(10)
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/usage/output_tokens"),
        &serde_json::json!(5)
    );

    let billed = log
        .billed_usage
        .ok_or("the native-less flow stamps its totals")?;

    assert_eq!((billed.input_tokens, billed.output_tokens), (10, 5));

    let replay = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Gateway {
            offset: 0,
            carrier,
            visible_content: None,
        }],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&replay)?;

    let Some(ActiveBoundary::Gateway { envelope, .. }) = preparation.active() else {
        return Err("the emitted carrier replays through the boundary consumer".into());
    };

    let EnvelopePayload::GatewaySummary(payload) = &envelope.payload else {
        return Err("the compact endpoint's carrier wraps a gateway summary".into());
    };

    assert_eq!(payload.summary_text, "Gateway summary text.");

    let dispatches = recorded(&harness.requests)?;

    assert_eq!(dispatches.len(), 1, "exactly one summary dispatch ran");

    let dispatch = dispatches.first().ok_or("the recorded summary dispatch")?;

    let summary_body: Value = serde_json::from_str(&dispatch.body)?;

    assert_eq!(
        crate::test_support::at(&summary_body, "/model"),
        &serde_json::json!("upstream-model"),
        "the internal dispatch speaks the upstream model identifier"
    );

    Ok(())
}

/// The compact endpoint's native flow: an eligible standalone operation dispatches the compact
/// body against the operation's own URL and passes the complete ordered window through,
/// including its usage.
#[tokio::test]
async fn compact_native_passthrough_preserves_the_window() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |base| standalone_section(base, Value::Null),
        None,
        vec![MockAction::json(responses_compact_body().to_string())],
    )
    .await?;

    let parsed = compact_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello world")]
    }))?;

    let mut log = log_draft();

    let body = super::execute_responses_compact(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let rendered = serde_json::to_value(&body)?;

    assert_eq!(
        crate::test_support::at(&rendered, "/output/0/encrypted_content"),
        &serde_json::json!("native-ciphertext"),
        "the native window passes through verbatim"
    );

    assert_eq!(
        crate::test_support::at(&rendered, "/usage/input_tokens"),
        &serde_json::json!(7)
    );

    let billed = log
        .billed_usage
        .ok_or("the native flow stamps its totals")?;

    assert_eq!((billed.input_tokens, billed.output_tokens), (7, 3));

    let dispatches = recorded(&harness.requests)?;

    let [dispatch] = dispatches.as_slice() else {
        return Err("exactly the compact operation dispatched".into());
    };

    assert!(
        dispatch.target.ends_with("/v1/responses/compact"),
        "the operation rides its own connection, got {}",
        dispatch.target
    );

    let dispatched_body: Value = serde_json::from_str(&dispatch.body)?;

    assert_eq!(
        crate::test_support::at(&dispatched_body, "/model"),
        &serde_json::json!("upstream-model")
    );

    Ok(())
}

/// A positively classified unsupported answer at the native compact call permits exactly one
/// same-account summary fallback: two dispatches, one carrier, and no quota-style error.
#[tokio::test]
async fn compact_classified_unsupported_falls_back_to_one_summary() -> Result<(), Box<dyn StdError>>
{
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |base| standalone_section(base, serde_json::json!({"statuses": [410]})),
        None,
        vec![
            rejected("410 Gone", &serde_json::json!({"error": {"code": "gone"}})),
            MockAction::json(responses_summary_body().to_string()),
        ],
    )
    .await?;

    let parsed = compact_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello world")]
    }))?;

    let mut log = log_draft();

    let body = super::execute_responses_compact(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let rendered = serde_json::to_value(&body)?;

    assert!(
        crate::test_support::at(&rendered, "/output/0/encrypted_content")
            .as_str()
            .ok_or("the carrier rides the output item")?
            .starts_with("janusgate.compaction.v1."),
        "the classified boundary fell back to the gateway summary"
    );

    let dispatches = recorded(&harness.requests)?;

    assert_eq!(
        dispatches.len(),
        2,
        "the native attempt and the single summary fallback dispatched"
    );

    let dispatch = dispatches.first().ok_or("the recorded native attempt")?;

    assert!(
        dispatch.target.ends_with("/v1/responses/compact"),
        "the fallback ran after, not instead of, the native attempt"
    );

    let billed = log.billed_usage.ok_or("the fallback stamps its totals")?;

    assert_eq!(
        (billed.input_tokens, billed.output_tokens),
        (10, 5),
        "the failed native attempt reported no usage; only the summary billed"
    );

    Ok(())
}

/// An unclassified upstream rejection propagates unchanged: no summary fallback runs, no
/// artifact is fabricated, and the one dispatched attempt is all the accounting shows.
#[tokio::test]
async fn compact_unclassified_failure_propagates_without_fallback() -> Result<(), Box<dyn StdError>>
{
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |base| standalone_section(base, Value::Null),
        None,
        vec![rejected(
            "503 Service Unavailable",
            &serde_json::json!({"error": {"code": "overloaded"}}),
        )],
    )
    .await?;

    let parsed = compact_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello world")]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_compact(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await;

    let error: ApiError = match outcome {
        Ok(_) => return Err("an unclassified rejection propagates".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("overloaded"),
        "the upstream failure keeps its own body semantics, got {error}"
    );

    assert_eq!(
        recorded(&harness.requests)?.len(),
        1,
        "no summary fallback dispatched behind the failure"
    );

    Ok(())
}

/// A damaged gateway marker in a compact request's history is a client-state failure naming the
/// stable code — never reclassified as native state and never summarized.
#[tokio::test]
async fn damaged_carrier_fails_the_request_explicitly() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| Value::Null,
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = compact_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input("covered"),
            {"type": "compaction",
                "encrypted_content": "janusgate.compaction.v1.bad.!!!not-base64!!!"}
        ]
    }))?;

    // The serving boundary authenticates before account selection, so the damaged marker fails
    // the preparation — the assertion pins the failure that `?` would otherwise swallow.
    let failure = match preparation(&parsed) {
        Ok(_) => return Err("a damaged marker fails authentication".into()),

        Err(error) => error,
    };

    assert!(
        failure
            .to_string()
            .contains("compaction_invalid_gateway_state"),
        "the stable code names the failed carrier, got {failure}"
    );

    assert_eq!(
        recorded(&harness.requests)?.len(),
        0,
        "a damaged marker is never summarized and never forwarded upstream"
    );

    Ok(())
}

/// A replaying gateway checkpoint restores ahead of any dispatch: a pure replay of a summary
/// carrier renders the restored window through the ordinary outcome with no new compaction call.
#[tokio::test]
async fn replaying_a_checkpoint_restores_the_window_without_new_work()
-> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| Value::Null,
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let carrier = sealed_summary_carrier("Sealed summary.")?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input("covered-history"),
            {"type": "compaction", "encrypted_content": carrier},
            responses_user_input("fresh-turn")
        ]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::Ordinary {
        ir,
        context_management,
        overlay_headers,
    } = outcome
    else {
        return Err("a pure replay continues through the ordinary pipeline".into());
    };

    assert_eq!(context_management, None);

    assert!(overlay_headers.is_empty());

    let [summary, kept, fresh] = ir.messages.as_slice() else {
        return Err(
            "the restored window is the summary, the retained unit, and the fresh suffix".into(),
        );
    };

    let Some(IrPart::Text { text: summary_text }) = summary.parts.first() else {
        return Err("the restored checkpoint heads the window".into());
    };

    assert!(
        summary_text.starts_with("The following is a summary of the earlier conversation:")
            && summary_text.ends_with("Sealed summary."),
        "the summary restores as user-role context"
    );

    let Some(IrPart::Text { text: kept_text }) = kept.parts.first() else {
        return Err("the retained unit restores whole".into());
    };

    assert_eq!(kept_text, "kept");

    let Some(IrPart::Text { text: fresh_text }) = fresh.parts.first() else {
        return Err("the fresh suffix follows the checkpoint".into());
    };

    assert_eq!(
        fresh_text, "fresh-turn",
        "the covered history is superseded while the fresh suffix survives"
    );

    assert_eq!(
        recorded(&harness.requests)?.len(),
        0,
        "a replay adds no compaction call"
    );

    Ok(())
}

/// A wrapped native window binds its producer: replaying against another provider or model is
/// inaccessible state, while the recorded producer's own replay restores the exact window.
#[tokio::test]
async fn wrapped_native_windows_replay_only_for_their_producer() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| Value::Null,
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let foreign = sealed_native_carrier("provider:999", "other-model")?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input("covered"),
            {"type": "compaction", "encrypted_content": foreign}
        ]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await;

    let error: ApiError = match outcome {
        Ok(_) => return Err("another producer's state is inaccessible".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("compaction_inaccessible_state"),
        "the stable code names the incompatible producer, got {error}"
    );

    let own = sealed_native_carrier("provider:7", "upstream-model")?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input("covered"),
            {"type": "compaction", "encrypted_content": own}
        ]
    }))?;

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::Ordinary { ir, .. } = outcome else {
        return Err("the producer's own window replays ordinarily".into());
    };

    assert!(
        ir.messages.is_empty(),
        "the canonical window replaces the covered history"
    );

    let [window] = ir.state_items.as_slice() else {
        return Err("the exact native window replays as state".into());
    };

    assert!(
        window.is_compaction && window.offset == 0 && !window.is_gateway,
        "the window rides as the producer's own compaction state"
    );

    Ok(())
}

/// The explicit trigger compacts through the decision order's final mechanism when no native
/// operation exists, producing exactly one completed carrier item as a compact-only lifecycle.
#[tokio::test]
async fn trigger_compacts_through_the_gateway_summary() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| serde_json::json!({"retention_budget": 1}),
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input("hello"),
            {"type": "compaction_trigger"}
        ]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::CompactionOnly {
        items,
        artifact_id,
        usage,
        ..
    } = outcome
    else {
        return Err("a trigger resolves to a compact-only lifecycle".into());
    };

    let [item] = items.as_slice() else {
        return Err("the trigger contract demands exactly one artifact".into());
    };

    let rendered = checkpoint_value(item)?;

    assert_eq!(
        crate::test_support::at(&rendered, "/type"),
        &serde_json::json!("compaction")
    );

    assert!(
        artifact_id.starts_with("cmp_"),
        "the artifact identity is the sealer-proven id"
    );

    assert_eq!((usage.input_tokens, usage.output_tokens), (10, 5));

    Ok(())
}

/// A supported native automatic mechanism passes its controls through upstream untouched on the
/// ordinary pipeline, preserving the entries for the native upstream.
#[tokio::test]
async fn automatic_passthrough_when_supported() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| {
            serde_json::json!({
                "strategy": "native_preferred",
                "mechanisms": {
                    "responses_automatic_compaction": {"default": "supported"}
                }
            })
        },
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello")],
        "context_management": [{"type": "compaction"}]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::Ordinary {
        context_management,
        overlay_headers,
        ..
    } = outcome
    else {
        return Err("the native automatic mechanism rides the ordinary pipeline".into());
    };

    let Some(entries) = context_management else {
        return Err("the automatic controls pass through upstream".into());
    };

    let [entry] = entries.as_slice() else {
        return Err("the parsed entry passes through in order".into());
    };

    assert_eq!(
        entry.compact_threshold, None,
        "an unstated threshold keeps the producer's own default"
    );

    assert!(overlay_headers.is_empty());

    assert_eq!(
        recorded(&harness.requests)?.len(),
        0,
        "the executor itself dispatches nothing"
    );

    Ok(())
}

/// Emulated automatic compaction without a client threshold needs the model's declared capacity:
/// a model with no effective input-token fact fails explicitly instead of inventing a window.
#[tokio::test]
async fn automatic_emulation_fails_without_declared_capacity() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| Value::Null,
        None,
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello")],
        "context_management": [{"type": "compaction"}]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await;

    let error: ApiError = match outcome {
        Ok(_) => return Err("a missing capacity fails the gateway default".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("compaction_unknown_capacity"),
        "the stable code names the missing capacity, got {error}"
    );

    Ok(())
}

/// Emulated automatic compaction evaluates once at the request boundary: below the capacity
/// default the controls strip and the request continues ordinarily without any dispatch.
#[tokio::test]
async fn automatic_emulation_below_threshold_is_ordinary() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| Value::Null,
        Some(10),
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hi")],
        "context_management": [{"type": "compaction"}]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::Ordinary {
        ir,
        context_management,
        ..
    } = outcome
    else {
        return Err("a below-threshold request continues ordinarily".into());
    };

    assert_eq!(
        context_management, None,
        "the controls strip below the threshold"
    );

    assert_eq!(
        ir.messages.len(),
        1,
        "the request continues on its own history"
    );

    assert_eq!(recorded(&harness.requests)?.len(), 0);

    Ok(())
}

/// Emulated automatic compaction at the capacity default installs one gateway checkpoint and
/// continues on the compacted window: the pending user turn stays mandatory state, and the
/// checkpoint precedes the answer.
#[tokio::test]
async fn automatic_emulation_compacts_and_continues() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::OpenAiResponse,
        |_base| serde_json::json!({"retention_budget": 4}),
        Some(10),
        vec![MockAction::json(responses_summary_body().to_string())],
    )
    .await?;

    let parsed = responses_request(&serde_json::json!({
        "model": "public-model",
        "input": [
            responses_user_input(&"a".repeat(24)),
            responses_user_input("pending question")
        ],
        "context_management": [{"type": "compaction"}]
    }))?;

    let mut log = log_draft();

    let outcome = super::execute_responses_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let ResponsesOuterOutcome::ContinueWithCheckpoint {
        prefix_items,
        artifact_id,
        ir,
        ledger,
        ..
    } = outcome
    else {
        return Err("an at-threshold request compacts and continues".into());
    };

    let [item] = prefix_items.as_slice() else {
        return Err("one checkpoint installs before the answer".into());
    };

    let rendered = checkpoint_value(item)?;

    assert!(
        crate::test_support::at(&rendered, "/encrypted_content")
            .as_str()
            .ok_or("the carrier rides the checkpoint")?
            .starts_with("janusgate.compaction.v1."),
    );

    assert!(artifact_id.starts_with("cmp_"));

    let [summary, pending] = ir.messages.as_slice() else {
        return Err(
            "the continuation restores the checkpoint window beside the pending turn".into(),
        );
    };

    let Some(IrPart::Text { text: summary_text }) = summary.parts.first() else {
        return Err("the checkpoint's summary heads the continuation".into());
    };

    assert!(summary_text.ends_with("Gateway summary text."));

    let Some(IrPart::Text { text: pending_text }) = pending.parts.first() else {
        return Err("the pending user turn rides the continuation".into());
    };

    assert_eq!(
        pending_text, "pending question",
        "the pending turn is mandatory retained state"
    );

    let totals = ledger
        .reported_billed_totals()
        .ok_or("the summary billed")?;

    assert_eq!((totals.input_tokens, totals.output_tokens), (10, 5));

    Ok(())
}

/// The Messages on-demand gateway-summary flow seals a carrier bound to the visible summary,
/// reports the compaction iterations separately, and honors the caller's custom instructions in
/// the summarizer dispatch.
#[tokio::test]
async fn on_demand_summary_binds_the_visible_content() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |_base| serde_json::json!({"retention_budget": 1}),
        None,
        vec![MockAction::json(messages_summary_body().to_string())],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [messages_user_message("hello")],
            "compaction": {"type": "summarize", "instructions": "Custom instructions."}
        }),
        &[ON_DEMAND_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::CompactionOnly {
        block,
        artifact_id,
        iterations,
        ..
    } = outcome
    else {
        return Err("an on-demand request resolves to a compact-only lifecycle".into());
    };

    let rendered = checkpoint_value(&block)?;

    assert_eq!(
        crate::test_support::at(&rendered, "/content"),
        &serde_json::json!("Gateway summary text."),
        "the visible summary content rides the carrier block"
    );

    let carrier = crate::test_support::at(&rendered, "/signature")
        .as_str()
        .ok_or("the carrier rides the signature")?
        .to_string();

    assert!(carrier.starts_with("janusgate.compaction.v1."));

    let artifact = artifact_id.ok_or("a gateway carrier carries its identity")?;

    assert!(artifact.starts_with("cmp_"));

    let [iteration] = iterations.as_slice() else {
        return Err("the summary call reports one compaction iteration".into());
    };

    let rendered = serde_json::to_value(iteration)?;

    assert_eq!(
        rendered,
        serde_json::json!({"type": "compaction", "input_tokens": 10, "output_tokens": 5}),
        "compaction iterations report separately from message work"
    );

    let replay = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Gateway {
            offset: 0,
            carrier,
            visible_content: Some(String::from("Gateway summary text.")),
        }],
        instruction_items: Vec::new(),
    };

    assert!(
        BoundaryPreparation::authenticate(&replay).is_ok(),
        "the carrier is bound to the visible summary it traveled with"
    );

    let dispatches = recorded(&harness.requests)?;

    let [dispatch] = dispatches.as_slice() else {
        return Err("exactly the summary dispatch ran".into());
    };

    let dispatched_body: Value = serde_json::from_str(&dispatch.body)?;

    assert_eq!(
        crate::test_support::at(&dispatched_body, "/system"),
        &serde_json::json!("Custom instructions."),
        "supplied compaction instructions replace the default summarization prompt"
    );

    Ok(())
}

/// The native on-demand mechanism returns the upstream's own signed block as the compact-only
/// artifact — no gateway identity, the declaration's overlay headers riding the dispatch.
#[tokio::test]
async fn on_demand_native_success_returns_the_block() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |_base| {
            serde_json::json!({
                "strategy": "native_preferred",
                "mechanisms": {
                    "messages_on_demand_compaction": {
                        "default": "supported",
                        "overlay_headers": [
                            {"name": "anthropic-beta",
                             "value": [{"type": "literal", "value": "compact-pod"}]}
                        ]
                    }
                }
            })
        },
        None,
        vec![MockAction::json(
            messages_compaction_block_body().to_string(),
        )],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [messages_user_message("hello")],
            "compaction": {"type": "summarize"}
        }),
        &[ON_DEMAND_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::CompactionOnly {
        block,
        artifact_id,
        iterations,
        ..
    } = outcome
    else {
        return Err("a native success resolves to a compact-only lifecycle".into());
    };

    let Checkpoint::Native(native) = &block else {
        return Err("the upstream's own block passes through verbatim".into());
    };

    assert_eq!(
        crate::test_support::at(native, "/signature"),
        &serde_json::json!("native-signature"),
        "native signatures remain unchanged"
    );

    assert_eq!(
        artifact_id, None,
        "a raw native block carries no gateway identity"
    );

    assert!(
        iterations.is_empty(),
        "the upstream reported no usage iterations"
    );

    let dispatches = recorded(&harness.requests)?;

    let [dispatch] = dispatches.as_slice() else {
        return Err("exactly the on-demand dispatch ran".into());
    };

    assert_eq!(
        dispatch.header("anthropic-beta"),
        Some("compact-pod"),
        "the declaration's overlay headers ride the ordinary connection"
    );

    Ok(())
}

/// The native on-demand operation's documented no-summary outcome stands: the aggregation
/// renders with its own semantics, and no gateway artifact is fabricated from it.
#[tokio::test]
async fn on_demand_native_no_summary_preserves_the_protocol_outcome()
-> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |_base| {
            serde_json::json!({
                "strategy": "native_preferred",
                "mechanisms": {
                    "messages_on_demand_compaction": {"default": "supported"}
                }
            })
        },
        None,
        vec![MockAction::json(messages_summary_body().to_string())],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [messages_user_message("hello")],
            "compaction": {"type": "summarize"}
        }),
        &[ON_DEMAND_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::NativeNoSummary { aggregation, .. } = outcome else {
        return Err("a plain native answer is the no-summary protocol outcome".into());
    };

    assert_eq!(aggregation.text, "Gateway summary text.");

    assert!(
        aggregation.compaction_states.is_empty(),
        "no block arrived, so no artifact exists"
    );

    Ok(())
}

/// Emulated threshold compaction below the trigger strips the controls and generates ordinarily,
/// dispatching nothing and carrying no edits.
#[tokio::test]
async fn threshold_below_the_trigger_is_ordinary() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |_base| Value::Null,
        None,
        vec![MockAction::json(messages_summary_body().to_string())],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [messages_user_message("tiny")],
            "context_management": {"edits": [
                {"type": "compact_20260112",
                 "trigger": {"type": "input_tokens", "value": 50_000}}
            ]}
        }),
        &[THRESHOLD_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::Ordinary {
        threshold_edits,
        overlay_headers,
        ..
    } = outcome
    else {
        return Err("a below-threshold request continues ordinarily".into());
    };

    assert!(
        threshold_edits.is_none(),
        "the edits strip below the threshold"
    );

    assert!(overlay_headers.is_empty());

    assert_eq!(recorded(&harness.requests)?.len(), 0);

    Ok(())
}

/// Emulated threshold compaction at the trigger installs one gateway checkpoint, reports its
/// iteration, and continues on the restored window; a pause request stops at the checkpoint
/// instead.
#[tokio::test]
async fn threshold_at_the_trigger_compacts_then_continues_or_pauses()
-> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    for pause in [false, true] {
        let harness = harness(
            UpstreamFormat::AnthropicMessages,
            |_base| Value::Null,
            None,
            vec![MockAction::json(messages_summary_body().to_string())],
        )
        .await?;

        let parsed = messages_request(
            &serde_json::json!({
                "model": "public-model",
                "max_tokens": 100,
                "messages": [
                    messages_user_message(&"a".repeat(150_000)),
                    messages_user_message("pending")
                ],
                "context_management": {"edits": [
                    {"type": "compact_20260112",
                     "trigger": {"type": "input_tokens", "value": 50_000},
                     "pause_after_compaction": pause}
                ]}
            }),
            &[THRESHOLD_BETA],
        )?;

        let mut log = log_draft();

        let outcome = super::execute_messages_request(
            &harness.context,
            &harness.routing,
            &parsed,
            &preparation(&parsed)?,
            &mut log,
            None,
        )
        .await?;

        if pause {
            let MessagesOuterOutcome::CompactionOnly {
                block, artifact_id, ..
            } = outcome
            else {
                return Err("a pause request stops at its checkpoint".into());
            };

            let rendered = checkpoint_value(&block)?;

            assert!(
                crate::test_support::at(&rendered, "/content")
                    .as_str()
                    .ok_or("the visible summary rides the block")?
                    .ends_with("Gateway summary text."),
            );

            assert!(
                artifact_id
                    .ok_or("a gateway carrier names itself")?
                    .starts_with("cmp_")
            );
        } else {
            let MessagesOuterOutcome::ContinueWithCheckpoint {
                prefix_blocks,
                ir,
                iterations,
                ..
            } = outcome
            else {
                return Err("a threshold hit compacts and continues".into());
            };

            let [block] = prefix_blocks.as_slice() else {
                return Err("one checkpoint block installs before the answer".into());
            };

            let rendered = checkpoint_value(block)?;

            assert_eq!(
                crate::test_support::at(&rendered, "/type"),
                &serde_json::json!("compaction")
            );

            let [summary, pending] = ir.messages.as_slice() else {
                return Err(
                    "the continuation carries the checkpoint window and the pending turn".into(),
                );
            };

            let Some(IrPart::Text { text: pending_text }) = pending.parts.first() else {
                return Err("the pending turn rides the continuation".into());
            };

            assert_eq!(pending_text, "pending", "the pending turn stays whole");

            let Some(IrPart::Text { text: summary_text }) = summary.parts.first() else {
                return Err("the summary heads the restored window".into());
            };

            assert!(summary_text.ends_with("Gateway summary text."));

            let [iteration] = iterations.as_slice() else {
                return Err("the compaction iteration reports beside the continuation".into());
            };

            assert!(
                serde_json::to_value(iteration)?.pointer("/type")
                    == Some(&serde_json::json!("compaction"))
            );
        }

        assert_eq!(
            recorded(&harness.requests)?.len(),
            1,
            "exactly the summary dispatch ran per request"
        );
    }

    Ok(())
}

/// A supported native threshold mechanism passes the vendor-shaped edits through on the ordinary
/// pipeline, joined with the threshold declaration's own overlay.
#[tokio::test]
async fn native_threshold_passthrough_when_supported() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |_base| {
            serde_json::json!({
                "strategy": "native_preferred",
                "mechanisms": {
                    "messages_threshold_compaction": {
                        "default": "supported",
                        "overlay_headers": [
                            {"name": "anthropic-beta",
                             "value": [{"type": "literal", "value": "compact-threshold"}]}
                        ]
                    }
                }
            })
        },
        None,
        vec![MockAction::json(messages_summary_body().to_string())],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [messages_user_message("tiny")],
            "context_management": {"edits": [
                {"type": "compact_20260112",
                 "trigger": {"type": "input_tokens", "value": 50_000}}
            ]}
        }),
        &[THRESHOLD_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::Ordinary {
        threshold_edits,
        overlay_headers,
        ..
    } = outcome
    else {
        return Err("the native threshold mechanism rides the ordinary pipeline".into());
    };

    let Some(edits) = threshold_edits else {
        return Err("the vendor-shaped edits pass through upstream".into());
    };

    let [edit] = edits.as_slice() else {
        return Err("the parsed edit passes through in order".into());
    };

    let rendered = serde_json::to_value(edit)?;

    assert_eq!(
        crate::test_support::at(&rendered, "/trigger/value"),
        &serde_json::json!(50_000),
        "the trigger passes through exactly as the caller stated it"
    );

    let [row] = overlay_headers.as_slice() else {
        return Err("the threshold declaration's overlay rides the passthrough".into());
    };

    assert_eq!(row.name, "anthropic-beta");

    Ok(())
}

/// A supported native explicit trigger rides the ordinary request upstream, and its complete
/// lifecycle validates before any outer success: exactly one completed compaction item under a
/// normal finish. A trigger answer without one fails as inaccessible native state — the
/// missing-terminal degradation that never authorizes history replacement.
#[tokio::test]
async fn native_trigger_lifecycle_validates_before_success() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let section = |_base: &str| {
        serde_json::json!({
            "strategy": "native_preferred",
            "mechanisms": {"responses_explicit_trigger": {"default": "supported"}}
        })
    };

    let parsed_body = serde_json::json!({
        "model": "public-model",
        "input": [responses_user_input("hello"), {"type": "compaction_trigger"}]
    });

    let lifecycle = {
        let harness = harness(
            UpstreamFormat::OpenAiResponse,
            section,
            None,
            vec![MockAction::json(responses_compact_body().to_string())],
        )
        .await?;

        let parsed = responses_request(&parsed_body)?;

        let mut log = log_draft();

        super::execute_responses_request(
            &harness.context,
            &harness.routing,
            &parsed,
            &preparation(&parsed)?,
            &mut log,
            None,
        )
        .await?
    };

    let ResponsesOuterOutcome::NativeLifecycle {
        aggregation,
        ledger,
    } = lifecycle
    else {
        return Err("a supported native trigger resolves to its own lifecycle".into());
    };

    let [state] = aggregation.compaction_states.as_slice() else {
        return Err("the lifecycle carries exactly one completed compaction item".into());
    };

    assert_eq!(
        crate::test_support::at(state, "/type"),
        &serde_json::json!("compaction"),
        "the native compaction item passes through verbatim"
    );

    assert_eq!(
        aggregation.finish,
        Some(IrFinishReason::Stop),
        "the lifecycle ended normally"
    );

    let totals = ledger
        .reported_billed_totals()
        .ok_or("the native trigger billed its inference")?;

    assert_eq!((totals.input_tokens, totals.output_tokens), (7, 3));

    let missing = {
        let harness = harness(
            UpstreamFormat::OpenAiResponse,
            section,
            None,
            vec![MockAction::json(responses_summary_body().to_string())],
        )
        .await?;

        let parsed = responses_request(&parsed_body)?;

        let mut log = log_draft();

        super::execute_responses_request(
            &harness.context,
            &harness.routing,
            &parsed,
            &preparation(&parsed)?,
            &mut log,
            None,
        )
        .await
    };

    let error: ApiError = match missing {
        Ok(_) => return Err("a trigger answer without a completed item fails".into()),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("compaction_inaccessible_state"),
        "the missing terminal never authorizes history replacement, got {error}"
    );

    Ok(())
}

/// A configured native counter decides the threshold measurement: its count triggers compaction
/// even when the text estimate sits far below the trigger, and its measurement never becomes
/// billed usage.
#[tokio::test]
async fn counter_measurement_drives_the_threshold_decision() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let section = |base: &str| {
        serde_json::json!({
            "retention_budget": 1,
            "count_tokens": {
                "url": [{"type": "literal",
                    "value": format!("{base}/v1/messages/count_tokens")}]
            }
        })
    };

    let threshold_request = || {
        serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [
                messages_user_message(&"a".repeat(90)),
                {"role": "assistant", "content": "the answer"},
                messages_user_message("pending")
            ],
            "context_management": {"edits": [
                {"type": "compact_20260112",
                 "trigger": {"type": "input_tokens", "value": 50_000}}
            ]}
        })
    };

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        section,
        None,
        vec![
            MockAction::json(serde_json::json!({"input_tokens": 60_000}).to_string()),
            MockAction::json(messages_summary_body().to_string()),
        ],
    )
    .await?;

    let parsed = messages_request(&threshold_request(), &[THRESHOLD_BETA])?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::ContinueWithCheckpoint {
        prefix_blocks,
        ledger,
        ..
    } = outcome
    else {
        return Err("the counter's measurement at the trigger compacts and continues".into());
    };

    let [block] = prefix_blocks.as_slice() else {
        return Err("one checkpoint installs before the answer".into());
    };

    let rendered = checkpoint_value(block)?;

    assert!(
        crate::test_support::at(&rendered, "/signature")
            .as_str()
            .ok_or("the carrier rides the checkpoint block")?
            .starts_with("janusgate.compaction.v1."),
        "the counter-triggered compaction seals a gateway checkpoint"
    );

    let totals = ledger
        .reported_billed_totals()
        .ok_or("the summary billed its inference")?;

    assert_eq!(
        (totals.input_tokens, totals.output_tokens),
        (10, 5),
        "the counter's 60,000 measurement is not billed usage; only the summary is"
    );

    let dispatches = recorded(&harness.requests)?;

    let [count, summary] = dispatches.as_slice() else {
        return Err("exactly the count and the summary dispatch ran".into());
    };

    assert!(
        count.target.ends_with("/v1/messages/count_tokens"),
        "the counter rides its own connection, got {}",
        count.target
    );

    assert!(
        summary.target.ends_with("/v1/messages"),
        "the summary follows the trigger decision"
    );

    Ok(())
}

/// The counter's dispatch failure falls back to the documented text estimator: the tiny history
/// measures far below the trigger, so the request continues ordinarily with the edits stripped
/// and only the failed count attempt dispatched.
#[tokio::test]
async fn counter_failure_falls_back_to_the_estimator() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    let harness = harness(
        UpstreamFormat::AnthropicMessages,
        |base| {
            serde_json::json!({
                "retention_budget": 1,
                "count_tokens": {
                    "url": [{"type": "literal",
                        "value": format!("{base}/v1/messages/count_tokens")}]
                }
            })
        },
        None,
        vec![rejected(
            "500 Internal Server Error",
            &serde_json::json!({"error": {"type": "overloaded"}}),
        )],
    )
    .await?;

    let parsed = messages_request(
        &serde_json::json!({
            "model": "public-model",
            "max_tokens": 100,
            "messages": [
                messages_user_message(&"a".repeat(90)),
                {"role": "assistant", "content": "the answer"},
                messages_user_message("pending")
            ],
            "context_management": {"edits": [
                {"type": "compact_20260112",
                 "trigger": {"type": "input_tokens", "value": 50_000}}
            ]}
        }),
        &[THRESHOLD_BETA],
    )?;

    let mut log = log_draft();

    let outcome = super::execute_messages_request(
        &harness.context,
        &harness.routing,
        &parsed,
        &preparation(&parsed)?,
        &mut log,
        None,
    )
    .await?;

    let MessagesOuterOutcome::Ordinary {
        threshold_edits,
        overlay_headers,
        ..
    } = outcome
    else {
        return Err("the estimator's measurement below the trigger continues ordinarily".into());
    };

    assert!(
        threshold_edits.is_none(),
        "the edits strip below the trigger"
    );

    assert!(overlay_headers.is_empty());

    assert_eq!(
        recorded(&harness.requests)?.len(),
        1,
        "the failed count attempt is the only dispatch"
    );

    Ok(())
}

/// The routing of one provider/model speaking the given format with the given compaction section
/// JSON and model support facts — no database, upstream, or context, for the pure decision
/// helpers.
fn fixture_routing(
    format: UpstreamFormat,
    compaction: Value,
    model_support: Option<CompactionSupportFacts>,
) -> Result<ModelRouting, Box<dyn StdError>> {
    let definition = DefinitionPayload {
        format,
        inference: serde_json::from_value(serde_json::json!({
            "url": [{"type": "literal", "value": "http://unused.test/v1/messages"}],
            "upstream_streaming": "match_client"
        }))?,
        credential: serde_json::from_value(serde_json::json!({"type": "api_key"}))?,
        provisioning: None,
        discovery: None,
        quota: None,
        compaction: serde_json::from_value(compaction)?,
    };

    Ok(ModelRouting {
        model_route_id: 1,
        public_id: String::from("public-model"),
        provider: Provider {
            id: 7,
            name: String::from("fixture"),
            model_prefix: String::new(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: String::from("test-key"),
            },
            is_enabled: true,
            quota_generation: 0,
            quota_revision: 0,
            definition: ProviderDefinition {
                name: String::from("fixture-definition"),
                version: NonZero::new(1).ok_or("the fixture version is nonzero")?,
                payload: definition,
                created_at: 1_000,
            },
        },
        model: Model {
            id: 9,
            upstream_id: String::from("upstream-model"),
            is_enabled: true,
            discovery_state: DiscoveryState::Discovered,
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support: model_support,
            compaction_support_override: None,
        },
    })
}

/// The native gate opens only for a native-preferred strategy, a resolved-supported mechanism,
/// and a definition speaking the mechanism's wire: a format mismatch, the unknown state, and the
/// gateway-summary strategy each dispatch nothing, while a discovery-owned model fact opens the
/// gate without a declaration.
#[test]
fn the_native_gate_requires_strategy_support_and_format() -> Result<(), Box<dyn StdError>> {
    let supported = serde_json::json!({
        "strategy": "native_preferred",
        "mechanisms": {"responses_standalone_compact": {"default": "supported"}}
    });

    let routing = fixture_routing(UpstreamFormat::OpenAiResponse, supported, None)?;

    let section = routing.provider.definition.payload.compaction.as_ref();

    assert!(super::is_native_eligible(
        &routing,
        section,
        Mechanism::ResponsesStandaloneCompact,
        UpstreamFormat::OpenAiResponse
    ));

    assert!(
        !super::is_native_eligible(
            &routing,
            section,
            Mechanism::ResponsesStandaloneCompact,
            UpstreamFormat::AnthropicMessages
        ),
        "the definition's own wire format gates the mechanism"
    );

    assert!(
        !super::is_native_eligible(
            &routing,
            section,
            Mechanism::MessagesOnDemandCompaction,
            UpstreamFormat::AnthropicMessages
        ),
        "an undeclared mechanism resolves unknown, which never dispatches"
    );

    let gateway_summary = fixture_routing(
        UpstreamFormat::OpenAiResponse,
        serde_json::json!({
            "strategy": "gateway_summary",
            "mechanisms": {"responses_standalone_compact": {"default": "supported"}}
        }),
        None,
    )?;

    assert!(
        !super::is_native_eligible(
            &gateway_summary,
            gateway_summary
                .provider
                .definition
                .payload
                .compaction
                .as_ref(),
            Mechanism::ResponsesStandaloneCompact,
            UpstreamFormat::OpenAiResponse
        ),
        "the gateway-summary strategy skips native selection for new work"
    );

    let unknown = fixture_routing(UpstreamFormat::OpenAiResponse, Value::Null, None)?;

    assert!(
        !super::is_native_eligible(
            &unknown,
            None,
            Mechanism::ResponsesStandaloneCompact,
            UpstreamFormat::OpenAiResponse
        ),
        "nothing known resolves unknown, which never probes upstream"
    );

    let discovered_support = CompactionSupportFacts::from_pairs([(
        Mechanism::ResponsesStandaloneCompact,
        MechanismSupport::Supported,
    )])
    .ok_or("a non-empty pair set yields facts")?;

    let discovered = fixture_routing(
        UpstreamFormat::OpenAiResponse,
        Value::Null,
        Some(discovered_support),
    )?;

    assert!(
        super::is_native_eligible(
            &discovered,
            None,
            Mechanism::ResponsesStandaloneCompact,
            UpstreamFormat::OpenAiResponse
        ),
        "a discovery-owned model fact opens the gate without a declaration"
    );

    Ok(())
}

/// The replay overlay follows the restored boundary's replay contract: an on-demand block —
/// wrapped or native-leading — rides the on-demand declaration's headers, a historical block the
/// threshold declaration's, and every other shape rides none; a non-Anthropic target carries no
/// overlay at all.
#[test]
fn replay_overlays_follow_the_boundarys_replay_contract() -> Result<(), Box<dyn StdError>> {
    let section: CompactionSection = serde_json::from_value(serde_json::json!({
        "mechanisms": {
            "messages_on_demand_compaction": {
                "default": "supported",
                "overlay_headers": [
                    {"name": "anthropic-beta",
                     "value": [{"type": "literal", "value": "on-demand-beta"}]}
                ]
            },
            "messages_threshold_compaction": {
                "default": "supported",
                "overlay_headers": [
                    {"name": "anthropic-beta",
                     "value": [{"type": "literal", "value": "threshold-beta"}]}
                ]
            }
        }
    }))?;

    let summary_boundary = RestoredBoundary::Gateway {
        envelope: EnvelopeV1::new(
            String::from("cmp_overlay_summary"),
            ReplacementMode::HistoricalCheckpoint,
            EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
                summary_text: String::new(),
                preserved_instructions: Vec::new(),
                retained_units: Vec::new(),
                retained_state_items: Vec::new(),
                overlap_identities: Vec::new(),
            }),
            None,
        ),
    };

    let wrapped_block = RestoredBoundary::Gateway {
        envelope: EnvelopeV1::new(
            String::from("cmp_overlay_native"),
            ReplacementMode::CanonicalWindow,
            EnvelopePayload::NativeWindow(NativeWindowPayload {
                producer_instance_id: String::from("provider:7"),
                upstream_model_id: String::from("upstream-model"),
                mechanism: Mechanism::MessagesOnDemandCompaction,
                native_format: UpstreamFormat::AnthropicMessages,
                required_prefix_identity: None,
                retained_messages: Vec::new(),
                retained_state_items: Vec::new(),
                window: NativeReplayWindow::AnthropicOnDemandBlock {
                    block: serde_json::json!({"type": "compaction"}),
                },
            }),
            None,
        ),
    };

    let leading = RestoredBoundary::Native(NativeArtifact::MessagesCompactionBlock {
        is_null_no_op: false,
        placement: BoundaryPlacement::Leading,
    });

    let historical = RestoredBoundary::Native(NativeArtifact::MessagesCompactionBlock {
        is_null_no_op: false,
        placement: BoundaryPlacement::Historical,
    });

    let overlay_of = |value: &str| {
        vec![ParamRow {
            name: String::from("anthropic-beta"),
            value: StringExpr(vec![ValueExpr::Literal {
                value: String::from(value),
            }]),
        }]
    };

    assert_eq!(
        super::replay_overlay_headers(
            Some(&section),
            UpstreamFormat::AnthropicMessages,
            Some(&wrapped_block)
        ),
        overlay_of("on-demand-beta"),
        "a wrapped on-demand block replays under its mechanism's overlay"
    );

    assert_eq!(
        super::replay_overlay_headers(
            Some(&section),
            UpstreamFormat::AnthropicMessages,
            Some(&leading)
        ),
        overlay_of("on-demand-beta"),
        "a native leading block replays under the on-demand overlay"
    );

    assert_eq!(
        super::replay_overlay_headers(
            Some(&section),
            UpstreamFormat::AnthropicMessages,
            Some(&historical)
        ),
        overlay_of("threshold-beta"),
        "a historical block replays under the threshold overlay"
    );

    assert!(
        super::replay_overlay_headers(
            Some(&section),
            UpstreamFormat::AnthropicMessages,
            Some(&summary_boundary)
        )
        .is_empty(),
        "a gateway summary is portable and binds no replay headers"
    );

    assert!(
        super::replay_overlay_headers(Some(&section), UpstreamFormat::AnthropicMessages, None)
            .is_empty()
    );

    assert!(
        super::replay_overlay_headers(
            Some(&section),
            UpstreamFormat::OpenAiResponse,
            Some(&leading)
        )
        .is_empty(),
        "a non-Anthropic target carries no replay overlay"
    );

    assert!(
        super::replay_overlay_headers(None, UpstreamFormat::AnthropicMessages, Some(&historical))
            .is_empty(),
        "no declaration carries no overlay"
    );

    Ok(())
}

/// The overlay merge joins same-named rows in place — the operation's value first, the replayed
/// state's appended after a comma — keeps order for disjoint rows, and appends to the last
/// same-named row when the operation side carries several.
#[test]
fn overlay_rows_join_same_named_values_and_keep_disjoint_order() {
    let row = |name: &str, value: &str| ParamRow {
        name: String::from(name),
        value: StringExpr(vec![ValueExpr::Literal {
            value: String::from(value),
        }]),
    };

    let joined = super::combine_overlay_rows(
        vec![row("anthropic-beta", "op")],
        vec![row("ANTHROPIC-BETA", "replay")],
    );

    assert_eq!(
        joined,
        vec![ParamRow {
            name: String::from("anthropic-beta"),
            value: StringExpr(vec![
                ValueExpr::Literal {
                    value: String::from("op")
                },
                ValueExpr::Literal {
                    value: String::from(",")
                },
                ValueExpr::Literal {
                    value: String::from("replay")
                },
            ]),
        }],
        "the joined row keeps the operation's value first, comma-separated from the replay's"
    );

    let disjoint = super::combine_overlay_rows(vec![row("x-a", "1")], vec![row("y-b", "2")]);

    assert_eq!(disjoint, vec![row("x-a", "1"), row("y-b", "2")]);

    let appended_to_last = super::combine_overlay_rows(
        vec![row("beta", "first"), row("beta", "second")],
        vec![row("beta", "replay")],
    );

    assert_eq!(
        appended_to_last,
        vec![
            row("beta", "first"),
            ParamRow {
                name: String::from("beta"),
                value: StringExpr(vec![
                    ValueExpr::Literal {
                        value: String::from("second")
                    },
                    ValueExpr::Literal {
                        value: String::from(",")
                    },
                    ValueExpr::Literal {
                        value: String::from("replay")
                    },
                ]),
            },
        ],
        "the replay value appends to the last same-named operation row"
    );
}
