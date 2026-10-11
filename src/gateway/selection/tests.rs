//! The ordered-pool-selection tests, driven end to end through
//! [`crate::gateway::execute_selection`] against the mock upstream: the ascending priority
//! scan, the positively-identified-exhaustion switch, the ambiguous-rejection refusal to
//! switch, the transport failure that ends selection, the established binding's required
//! account, and the direct route's bypass of both affinity and quota preflight.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions; a # Panics section would \
              document nothing its name does not"
)]

use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    api::tests_support::{
        TestGateway, chat_definition, json_bytes, lock_recordings, recorded_json,
        seed_allowing_key, seed_definition, seed_model, seed_pool, seed_provider, seed_quota_block,
        seed_route, upstream_chat_json,
    },
    format::{ClientFormat as _, RouteContext},
    gateway,
    gateway::Error as GatewayError,
    ir::IrEvent,
    routing::{ModelRouteTarget, ProviderPoolMember},
    test_support::{
        TempDatabaseGuard,
        mock_upstream::{MockAction, MockUpstream, RecordedRequests},
    },
};
use futures::StreamExt as _;
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::error::Error as StdError;

/// The pool route's public id: both selection scenarios address it through the ordinary
/// OpenAI-format parse the pipeline runs.
const POOL_MODEL: &str = "pool.model";

/// The exact upstream id both interchangeable pool members serve.
const UPSTREAM_ID: &str = "shared-upstream";

/// A pool-selection scenario: two enabled accounts at priorities 1 and 2 sharing the exact
/// upstream model, the pooled public route, and one all-allowing serving key.
struct PoolScenario {
    /// The gateway under test.
    gateway: TestGateway,

    /// The low-priority (first-scanned) account's row id.
    first_provider: i64,

    /// The high-priority (second-scanned) account's row id.
    second_provider: i64,

    /// The serving key's row id.
    api_key_id: i64,
}

/// The complete raw JSON response the mock serves for a non-2xx upstream rejection.
fn raw_json_response(status: u16, reason: &str, body: &Value) -> String {
    let body = body.to_string();

    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The authored 429 quota-exhaustion rule the switch scenarios carry on both accounts.
fn quota_rule() -> Value {
    json!({
        "rules": [{
            "name": "fixture-quota",
            "statuses": [429],
            "code": {"at": ["error", "type"], "value": "insufficient_quota"},
            "restriction_key": [{"type": "literal", "value": "fixture"}],
            "applies_to": {"type": "all_models"}
        }]
    })
}

impl PoolScenario {
    /// Opens the gateway, starts from a fresh temporary database, and seeds both accounts over
    /// one chat-completions definition upstreaming to `base_url`; when `quota_rule` is set, the
    /// definition carries the matching 429 quota-exhaustion rule.
    async fn open(
        base_url: &str,
        quota_rule: Option<Value>,
    ) -> Result<(Self, TempDatabaseGuard), Box<dyn StdError>> {
        let (gateway, guard) = TestGateway::open().await;

        let mut inference_overrides = serde_json::Map::new();

        if let Some(rule) = quota_rule {
            inference_overrides.insert("quota_exhaustion".to_owned(), rule);
        }

        let definition = seed_definition(
            gateway.database(),
            "fixture-definition",
            chat_definition(
                json!([{"type": "base"}, {"type": "literal", "value": "/upstream/v1"}]),
                Value::Object(inference_overrides),
            ),
        )
        .await?;

        let first_provider = seed_provider(
            gateway.database(),
            "first-account",
            "",
            base_url,
            &definition,
        )
        .await?;

        let second_provider = seed_provider(
            gateway.database(),
            "second-account",
            "",
            base_url,
            &definition,
        )
        .await?;

        // The pool exists before any inventory: membership suppresses the members' standalone
        // exposure, so the pool's own automatic entry is the only one the discovery creates.
        let pool_id = seed_pool(
            gateway.database(),
            "fixture-pool",
            "",
            &[
                ProviderPoolMember {
                    provider_id: first_provider,
                    priority: 1,
                },
                ProviderPoolMember {
                    provider_id: second_provider,
                    priority: 2,
                },
            ],
        )
        .await?;

        let _ = seed_model(gateway.database(), first_provider, UPSTREAM_ID).await?;

        let _ = seed_model(gateway.database(), second_provider, UPSTREAM_ID).await?;

        let _ = seed_route(
            gateway.database(),
            POOL_MODEL,
            &ModelRouteTarget::Pool {
                pool_id,
                upstream_id: UPSTREAM_ID.to_owned(),
            },
        )
        .await?;

        let api_key_id = seed_allowing_key(gateway.database()).await?;

        Ok((
            Self {
                gateway,
                first_provider,
                second_provider,
                api_key_id,
            },
            guard,
        ))
    }
}

/// Parses a minimal one-turn chat-completions request body for `public_id`.
fn parsed_request(
    public_id: &str,
    text: &str,
) -> Result<crate::format::ParsedRequest, Box<dyn StdError>> {
    let body = json!({
        "model": public_id,
        "messages": [{"role": "user", "content": text}]
    });

    Ok(
        crate::format::openai::chat_completion::Format::parse_request(
            &json_bytes(body),
            &RouteContext::empty(),
        )?,
    )
}

/// Drives one ordinary selection and returns its outcome.
async fn select_once(scenario: &PoolScenario) -> Result<super::SelectedAttempt, Box<dyn StdError>> {
    let parsed = parsed_request(POOL_MODEL, "Hello")?;

    let selected = gateway::execute_selection(
        scenario.gateway.context(),
        parsed,
        scenario.api_key_id,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await?;

    Ok(selected)
}

/// Drives one ordinary selection expected to fail and returns its typed gateway failure,
/// failing the test when the selection unexpectedly succeeds.
async fn selection_error(scenario: &PoolScenario) -> Result<GatewayError, Box<dyn StdError>> {
    let parsed = parsed_request(POOL_MODEL, "Hello")?;

    match gateway::execute_selection(
        scenario.gateway.context(),
        parsed,
        scenario.api_key_id,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await
    {
        Ok(_) => Err("expected the selection to fail".into()),
        Err(error) => Ok(error),
    }
}

/// Drains the accepted event stream to completion, finalizing the row as a completed response,
/// and returns the events.
async fn drain_accepted(
    selected: super::SelectedAttempt,
) -> Result<Vec<IrEvent>, Box<dyn StdError>> {
    let gateway::AttemptOutcome::Accepted { events, attempt } = selected.outcome else {
        return Err("expected the selection to accept the attempt".into());
    };

    let mut events = events;

    let mut collected = Vec::new();

    while let Some(event) = events.next().await {
        collected.push(event.map_err(|error| format!("the accepted stream failed: {error}"))?);
    }

    attempt.finalize(ApiLogOutcome::Completed, Some(200)).await;

    Ok(collected)
}

/// The texts of the drained events, concatenated.
fn text_of(events: &[IrEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            IrEvent::TextDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

/// The rendered request bodies the mock recorded, parsed as JSON.
fn recorded_bodies(requests: &RecordedRequests) -> Vec<Value> {
    let recorded = lock_recordings(requests);

    recorded
        .iter()
        .map(|request| recorded_json(&request.body))
        .collect()
}

/// The ascending priority scan dispatches the first structurally eligible account only: one
/// recorded upstream request, answered by the low-priority account.
#[tokio::test]
async fn the_priority_scan_dispatches_the_first_eligible_account() -> Result<(), Box<dyn StdError>>
{
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_chat_json("from-first", 3, 2).to_string()]).await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), None).await?;

    let selected = select_once(&scenario).await?;

    assert_eq!(selected.routing.provider.id, scenario.first_provider);

    assert_eq!(text_of(&drain_accepted(selected).await?), "from-first");

    assert_eq!(recorded_bodies(&requests).len(), 1);

    Ok(())
}

/// A first-stage rejection positively matching the definition's quota rule abandons the
/// exhausted account and advances the scan to the next member, which serves the request.
#[tokio::test]
async fn a_positive_quota_rejection_switches_to_the_next_member() -> Result<(), Box<dyn StdError>> {
    let exhausted = json!({"error": {"type": "insufficient_quota", "message": "limit hit"}});

    let (mock, requests) = MockUpstream::start(vec![
        MockAction::Respond(raw_json_response(429, "Too Many Requests", &exhausted)),
        MockAction::json(upstream_chat_json("from-second", 2, 1).to_string()),
    ])
    .await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), Some(quota_rule())).await?;

    let selected = select_once(&scenario).await?;

    assert_eq!(selected.routing.provider.id, scenario.second_provider);

    assert_eq!(text_of(&drain_accepted(selected).await?), "from-second");

    // Exactly one dispatch per account: the rejected account was abandoned, not retried.
    assert_eq!(recorded_bodies(&requests).len(), 2);

    // The rejection persisted positive evidence on the exhausted account.
    let state = crate::storage::quota_blocks::load_state(
        scenario.gateway.database(),
        scenario.first_provider,
    )
    .await?
    .ok_or("the rejected account carries no persisted restriction")?;

    assert!(!state.blocks.is_empty());

    Ok(())
}

/// A bare 429 the authored rule does not positively match is an ambiguous outcome: selection
/// ends on that account with the mirrored upstream error, never switching.
#[tokio::test]
async fn an_ambiguous_rejection_never_switches_accounts() -> Result<(), Box<dyn StdError>> {
    let ambiguous = json!({"error": {"type": "server_overload", "message": "try later"}});

    let (mock, requests) = MockUpstream::start(vec![MockAction::Respond(raw_json_response(
        429,
        "Too Many Requests",
        &ambiguous,
    ))])
    .await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), Some(quota_rule())).await?;

    let selected = select_once(&scenario).await?;

    assert_eq!(selected.routing.provider.id, scenario.first_provider);

    let gateway::AttemptOutcome::Rejected { error, attempt } = selected.outcome else {
        return Err("expected the ambiguous rejection to end selection".into());
    };

    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(429))
        .await;

    let GatewayError::Upstream { status, .. } = error else {
        return Err("expected the mirrored upstream error".into());
    };

    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);

    assert_eq!(recorded_bodies(&requests).len(), 1);

    // No evidence persisted: an ambiguous rejection creates no restriction.
    let state = crate::storage::quota_blocks::load_state(
        scenario.gateway.database(),
        scenario.first_provider,
    )
    .await?
    .ok_or("the attempted account's quota state is missing")?;

    assert!(state.blocks.is_empty());

    Ok(())
}

/// A transport failure without a usable HTTP response ends selection immediately on that
/// account as the upstream-unavailable outcome.
#[tokio::test]
async fn a_closed_transport_maps_to_the_unavailable_outcome() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) = MockUpstream::start(vec![MockAction::Close]).await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), None).await?;

    let selected = select_once(&scenario).await?;

    let gateway::AttemptOutcome::DispatchedFailure { error, attempt } = selected.outcome else {
        return Err("expected the transport failure to end selection as dispatched".into());
    };

    attempt
        .finalize(ApiLogOutcome::UpstreamError, Some(502))
        .await;

    let GatewayError::Unavailable { detail } = error else {
        return Err("expected the transport failure to map to Unavailable".into());
    };

    assert!(detail.contains("upstream unreachable"));

    assert_eq!(recorded_bodies(&requests).len(), 1);

    Ok(())
}

/// An established same-account binding is required: with a recorded restriction on the bound
/// account, selection fails with the bound-account exhaustion instead of switching to the
/// healthy second member — before any dispatch, so no attempt outcome exists.
#[tokio::test]
async fn a_bound_account_with_quota_evidence_is_required_not_switched()
-> Result<(), Box<dyn StdError>> {
    let (mock, _requests) = MockUpstream::start_answering(|| {
        MockAction::json(upstream_chat_json("bound-answer", 1, 1).to_string())
    })
    .await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), None).await?;

    // The first request establishes the binding on the low-priority account.
    let established = select_once(&scenario).await?;

    assert_eq!(established.routing.provider.id, scenario.first_provider);

    drain_accepted(established).await?;

    // A recorded restriction now blocks the bound account.
    seed_quota_block(scenario.gateway.database(), scenario.first_provider).await?;

    let GatewayError::BoundAccountQuotaExhausted { model, retry_after } =
        selection_error(&scenario).await?
    else {
        return Err("expected the bound-account quota exhaustion".into());
    };

    assert_eq!(model, POOL_MODEL);

    // No complete future reset evidence, no advertised delay.
    assert_eq!(retry_after, None);

    Ok(())
}

/// Every pool member carrying persisted applicable quota evidence is exhausted: the scan visits
/// the whole snapshot, dispatches nothing, and fails with the pool-quota exhaustion carrying no
/// advertised delay while no candidate's reset is proven.
#[tokio::test]
async fn every_blocked_member_leaves_the_pool_quota_exhausted() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_chat_json("never-served", 1, 1).to_string()]).await;

    let (scenario, _guard) = PoolScenario::open(&mock.base(), None).await?;

    seed_quota_block(scenario.gateway.database(), scenario.first_provider).await?;

    seed_quota_block(scenario.gateway.database(), scenario.second_provider).await?;

    let GatewayError::PoolQuotaExhausted { model, retry_after } =
        selection_error(&scenario).await?
    else {
        return Err("expected the pool-quota exhaustion".into());
    };

    assert_eq!(model, POOL_MODEL);

    // No complete future reset evidence on any candidate, no advertised delay.
    assert_eq!(retry_after, None);

    // The preflight never dispatched: the exhaustion was decided on stored evidence alone.
    assert_eq!(recorded_bodies(&requests).len(), 0);

    Ok(())
}

/// A direct route bypasses both affinity and quota preflight and dispatches once, even with a
/// recorded restriction on its own account.
#[tokio::test]
async fn a_direct_route_bypasses_quota_and_affinity() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) =
        MockUpstream::start_json(vec![upstream_chat_json("direct-answer", 1, 1).to_string()]).await;

    let (gateway, _guard) = TestGateway::open().await;

    let definition = seed_definition(
        gateway.database(),
        "fixture-definition",
        chat_definition(
            json!([{"type": "base"}, {"type": "literal", "value": "/upstream/v1"}]),
            json!({}),
        ),
    )
    .await?;

    let provider_id = seed_provider(
        gateway.database(),
        "direct-account",
        "",
        &mock.base(),
        &definition,
    )
    .await?;

    let model_id = seed_model(gateway.database(), provider_id, "direct-upstream").await?;

    let _ = seed_route(
        gateway.database(),
        "direct.model",
        &ModelRouteTarget::Direct { model_id },
    )
    .await?;

    let api_key_id = seed_allowing_key(gateway.database()).await?;

    // A recorded restriction exists before the request, yet a direct route never preflights it.
    seed_quota_block(gateway.database(), provider_id).await?;

    let parsed = parsed_request("direct.model", "Hello")?;

    let selected = gateway::execute_selection(
        gateway.context(),
        parsed,
        api_key_id,
        ApiLogEndpointType::OpenaiChatCompletions,
    )
    .await?;

    assert_eq!(selected.routing.provider.id, provider_id);

    assert!(selected.affinity.is_none());

    assert!(matches!(
        selected.outcome,
        gateway::AttemptOutcome::Accepted { .. }
    ));

    assert_eq!(recorded_bodies(&requests).len(), 1);

    Ok(())
}
