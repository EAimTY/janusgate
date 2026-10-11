//! Upstream model discovery tests: the array and map extraction modes with their capability
//! facts, the token and cursor pagination walks under their page budgets, and the
//! identifier-routing gate — all driven through the provider definition's discovery descriptor
//! against the mock upstream (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::discover_upstream_models;
use crate::{
    compaction_model::mechanism::{Mechanism, MechanismSupport},
    database::Database,
    provider::{Provider, definition::DefinitionPayload, discovery, tests_support::failure_detail},
    test_support::{
        mock_upstream::{MockUpstream, RecordedRequest, RecordedRequests},
        temp_database,
    },
    vocabulary::ReasoningLevel,
};
use serde_json::{Value, json};
use std::{error::Error as StdError, num::NonZeroI64};

/// The static credentials of every fixture provider: discovery reads stored credentials.
fn api_key_credentials() -> crate::provider::StoredCredentials {
    crate::provider::StoredCredentials::ApiKey {
        api_key: "key-1".to_string(),
    }
}

/// The in-memory provider carrying `payload` and answering the mock under `base`.
fn provider_with(payload: DefinitionPayload, base: &str) -> Provider {
    Provider {
        id: 1,
        name: "fixture".to_string(),
        model_prefix: "fixture.".to_string(),
        base_url: Some(base.to_string()),
        credentials: api_key_credentials(),
        is_enabled: true,
        quota_generation: 0,
        quota_revision: 0,
        definition: crate::provider::ProviderDefinition {
            name: "fixture-definition".to_string(),
            #[expect(
                clippy::expect_used,
                reason = "the fixture version is a nonzero literal"
            )]
            version: NonZeroI64::new(1).expect("fixture versions are nonzero literals"),
            payload,
            created_at: 1_000,
        },
    }
}

/// Parses one discovery payload fixture whose request addresses the mock's model list.
fn discovery_payload(base: &str, extract: Value, pagination: Value) -> DefinitionPayload {
    #[expect(
        clippy::expect_used,
        reason = "a fixture that cannot parse is that test's failure report"
    )]
    serde_json::from_value(json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/v1/chat/completions"}],
            "upstream_streaming": "match_client",
            "headers": [{"name": "Authorization", "value": [{"type": "secret"}]}]
        },
        "credential": {"type": "api_key"},
        "discovery": {
            "request": {"method": "GET", "url": [
                {"type": "literal", "value": format!("{base}/v1/models")}
            ]},
            "extract": extract,
            "pagination": pagination
        }
    }))
    .expect("the discovery payload fixture parses")
}

/// The array extraction over `data[]` with the fixture `id` field.
fn array_extract() -> Value {
    json!({"mode": "array", "array_at": ["data"], "id_field": ["id"]})
}

/// Serves one JSON page to every request.
async fn page_mock(body: Value) -> MockUpstream {
    let (mock, _) = MockUpstream::start_json(vec![body.to_string()]).await;

    mock
}

/// Runs one discovery to its outcome over a fresh database the engine may refresh through.
async fn discover(
    provider: &Provider,
) -> Result<Vec<discovery::DiscoveredModel>, crate::provider::Error> {
    let (database, _guard) = temp_database().await;

    discover_against(&database, provider).await
}

/// Runs one discovery expected to fail and renders its failure detail; a passing discovery is
/// the test's failure report.
async fn failure_of(provider: &Provider) -> String {
    let (database, _guard) = temp_database().await;

    failure_detail(discover_against(&database, provider).await)
}

/// Runs one discovery against an existing database.
async fn discover_against(
    database: &Database,
    provider: &Provider,
) -> Result<Vec<discovery::DiscoveredModel>, crate::provider::Error> {
    discover_upstream_models(database, &reqwest::Client::new(), provider).await
}

/// The cloned request list the mock recorded so far.
fn recorded(requests: &RecordedRequests) -> Result<Vec<RecordedRequest>, Box<dyn StdError>> {
    let guard = requests
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    Ok(guard.clone())
}

/// The identifiers of one discovery outcome.
fn ids(models: &[discovery::DiscoveredModel]) -> Vec<String> {
    models
        .iter()
        .map(|model| model.upstream_id.clone())
        .collect()
}

/// The array extraction reads each entry's identifier; the map-keys extraction reads object
/// member names with every capability unknown.
#[tokio::test]
async fn extraction_modes_read_their_shapes() -> Result<(), Box<dyn StdError>> {
    let mock = page_mock(json!({"data": [{"id": "m-a"}, {"id": "m-b"}]})).await;

    let models = discover(&provider_with(
        discovery_payload(&mock.base(), array_extract(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await?;

    assert_eq!(ids(&models), vec!["m-a".to_string(), "m-b".to_string()]);

    let first = models.first().expect("one model extracted");

    // Every capability stays unknown without a capability extraction.
    assert_eq!(first.thinking_levels, None);

    assert_eq!(first.max_input_tokens, None);

    assert_eq!(first.compaction, None);

    let mock = page_mock(json!({"models": {"zeta": {}, "alpha": {}}})).await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({"mode": "map_keys", "at": ["models"]}),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await?;

    // The map-keys shape carries no per-entry object: identifiers only, in the lexicographic
    // member order the JSON map iterates.
    assert_eq!(ids(&models), vec!["alpha".to_string(), "zeta".to_string()]);

    Ok(())
}

/// The configured prefix is stripped from every identifier — the Gemini `models/<id>` shape.
#[tokio::test]
async fn array_extraction_strips_the_configured_prefix() -> Result<(), Box<dyn StdError>> {
    let mock = page_mock(json!({"data": [
        {"name": "models/gemini-x"},
        {"name": "models/gemini-y"}
    ]}))
    .await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["name"],
                "strip_prefix": "models/"
            }),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        ids(&models),
        vec!["gemini-x".to_string(), "gemini-y".to_string()]
    );

    Ok(())
}

/// A payload missing the configured array, or an entry missing its identifier, fails the
/// refresh as an upstream error before any storage write.
#[tokio::test]
async fn malformed_page_shapes_fail_the_refresh() -> Result<(), Box<dyn StdError>> {
    let mock = page_mock(json!({"data": "not-an-array"})).await;

    let detail = failure_of(&provider_with(
        discovery_payload(&mock.base(), array_extract(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await;

    assert!(
        detail.contains("lacks the configured array"),
        "got: {detail}"
    );

    let mock = page_mock(json!({"data": [{"name": "m-a"}]})).await;

    let detail = failure_of(&provider_with(
        discovery_payload(&mock.base(), array_extract(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await;

    assert!(detail.contains("lacks its identifier"), "got: {detail}");

    Ok(())
}

/// One unroutable identifier fails the whole refresh — the ingestion boundary no discovered
/// row may bypass.
#[tokio::test]
async fn unroutable_identifiers_fail_the_refresh() -> Result<(), Box<dyn StdError>> {
    for hostile in ["has/slash", "has:colon", "has space"] {
        let mock = page_mock(json!({"data": [{"id": "fine"}, {"id": hostile}]})).await;

        let detail = failure_of(&provider_with(
            discovery_payload(&mock.base(), array_extract(), json!({"type": "none"})),
            &mock.base(),
        ))
        .await;

        assert!(
            detail.contains("unroutable identifier"),
            "`{hostile}` rejected, got: {detail}"
        );
    }

    Ok(())
}

/// Thinking-level extraction: member arrays read their level strings through the closed
/// vocabulary, skipping foreign levels and canonicalizing the declared set.
#[tokio::test]
async fn member_array_levels_filter_and_canonicalize() -> Result<(), Box<dyn StdError>> {
    let mock = page_mock(json!({"data": [{"id": "m-a", "levels": [
        {"effort": "high"},
        {"effort": "none"},
        {"effort": "beyond-the-vocabulary"},
        {"effort": "low"}
    ]}]}))
    .await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"],
                "capabilities": {"thinking_levels": {
                    "shape": "member_array", "array_at": ["levels"], "level_field": ["effort"]
                }}
            }),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        models.first().expect("one model extracted").thinking_levels,
        Some(vec![
            ReasoningLevel::None,
            ReasoningLevel::Low,
            ReasoningLevel::High
        ])
    );

    // An absent level array is the unknown state, never the empty set.
    let mock = page_mock(json!({"data": [{"id": "m-a"}]})).await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"],
                "capabilities": {"thinking_levels": {
                    "shape": "member_array", "array_at": ["levels"], "level_field": ["effort"]
                }}
            }),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        models.first().expect("one model extracted").thinking_levels,
        None
    );

    // A present non-array is the wrong shape.
    let mock = page_mock(json!({"data": [{"id": "m-a", "levels": "high"}]})).await;

    let detail = failure_of(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"],
                "capabilities": {"thinking_levels": {
                    "shape": "member_array", "array_at": ["levels"], "level_field": ["effort"]
                }}
            }),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await;

    assert!(detail.contains("malformed thinking-level array"));

    Ok(())
}

/// Flagged-keys level extraction: a bare boolean stands as its own flag, an object carries its
/// flag at the configured path, a null member reads as absent, and foreign keys are skipped.
#[tokio::test]
async fn flagged_keys_levels_read_both_spellings() -> Result<(), Box<dyn StdError>> {
    let mock = page_mock(json!({"data": [{"id": "m-a", "effort": {
        "low": true,
        "high": {"supported": true},
        "minimal": false,
        "xhigh": null,
        "mega": true
    }}]}))
    .await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            json!({
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"],
                "capabilities": {"thinking_levels": {
                    "shape": "flagged_keys", "object_at": ["effort"], "flag_field": ["supported"]
                }}
            }),
            json!({"type": "none"}),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        models.first().expect("one model extracted").thinking_levels,
        Some(vec![ReasoningLevel::Low, ReasoningLevel::High])
    );

    Ok(())
}

/// Token caps extract positive integers only: a null is unknown, a string or a non-positive
/// integer fails the refresh.
#[tokio::test]
async fn token_caps_stay_positive_integers() -> Result<(), Box<dyn StdError>> {
    let caps_payload =
        |caps: Value| json!({"data": [{"id": "m-a", "input": caps, "output": caps}]});

    let extract = json!({
        "mode": "array",
        "array_at": ["data"],
        "id_field": ["id"],
        "capabilities": {
            "max_input_tokens": ["input"],
            "max_output_tokens": ["output"]
        }
    });

    let mock = page_mock(caps_payload(json!(4096))).await;

    let models = discover(&provider_with(
        discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await?;

    let first = models.first().expect("one model extracted");

    assert_eq!(first.max_input_tokens, Some(4096));

    assert_eq!(first.max_output_tokens, Some(4096));

    let mock = page_mock(caps_payload(json!(null))).await;

    let models = discover(&provider_with(
        discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        models
            .first()
            .expect("one model extracted")
            .max_input_tokens,
        None
    );

    for hostile in [json!("4096"), json!(0), json!(-1)] {
        let mock = page_mock(caps_payload(hostile.clone())).await;

        let detail = failure_of(&provider_with(
            discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
            &mock.base(),
        ))
        .await;

        assert!(
            detail.contains("token cap"),
            "`{hostile}` rejected, got: {detail}"
        );
    }

    Ok(())
}

/// Compaction flags read one boolean per mechanism: true extracts `supported`, false
/// `unsupported`, and absent objects and flags stay unknown; wrong shapes fail the refresh.
#[tokio::test]
async fn compaction_flags_resolve_per_mechanism() -> Result<(), Box<dyn StdError>> {
    let extract = json!({
        "mode": "array",
        "array_at": ["data"],
        "id_field": ["id"],
        "capabilities": {"compaction": {
            "messages_on_demand": {"object_at": ["compaction"], "flag_field": ["supported"]},
            "messages_threshold": {"object_at": ["context_management", "edits"],
                                   "flag_field": ["supported"]}
        }}
    });

    let mock = page_mock(json!({"data": [{"id": "m-a",
        "compaction": {"supported": true},
        "context_management": {"edits": {"supported": false}}
    }]}))
    .await;

    let models = discover(&provider_with(
        discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await?;

    let facts = models
        .first()
        .expect("one model extracted")
        .compaction
        .as_ref()
        .expect("both flags extract");

    assert_eq!(
        facts.get(Mechanism::MessagesOnDemandCompaction),
        Some(MechanismSupport::Supported)
    );

    assert_eq!(
        facts.get(Mechanism::MessagesThresholdCompaction),
        Some(MechanismSupport::Unsupported)
    );

    // Absent objects leave both mechanisms unknown.
    let mock = page_mock(json!({"data": [{"id": "m-a"}]})).await;

    let models = discover(&provider_with(
        discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await?;

    assert_eq!(
        models.first().expect("one model extracted").compaction,
        None
    );

    // A non-object where an object is addressed, and a non-boolean flag, both fail.
    let mock = page_mock(json!({"data": [{"id": "m-a", "compaction": true}]})).await;

    let detail = failure_of(&provider_with(
        discovery_payload(&mock.base(), extract.clone(), json!({"type": "none"})),
        &mock.base(),
    ))
    .await;

    assert!(detail.contains("malformed compaction object"));

    let mock = page_mock(json!({"data": [{"id": "m-a",
        "compaction": {"supported": "yes"}
    }]}))
    .await;

    let detail = failure_of(&provider_with(
        discovery_payload(&mock.base(), extract, json!({"type": "none"})),
        &mock.base(),
    ))
    .await;

    assert!(detail.contains("malformed compaction flag"));

    Ok(())
}

/// Token pagination passes the next-page token back through the named request parameter and
/// ends on a page without one; a budget that ends on an indicated continuation fails instead
/// of truncating.
#[tokio::test]
async fn token_pagination_walks_and_enforces_its_budget() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded_list) = MockUpstream::start_json(vec![
        json!({"data": [{"id": "m-a"}], "next": "t2"}).to_string(),
        json!({"data": [{"id": "m-b"}], "next": null}).to_string(),
    ])
    .await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            array_extract(),
            json!({
                "type": "token",
                "request_param": "page",
                "response_path": ["next"],
                "max_pages": 3
            }),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(ids(&models), vec!["m-a".to_string(), "m-b".to_string()]);

    let requests = recorded(&recorded_list)?;

    assert_eq!(requests.len(), 2);

    assert_eq!(
        requests.first().map(|request| request.target.as_str()),
        Some("/v1/models")
    );

    assert_eq!(
        requests.get(1).map(|request| request.target.as_str()),
        Some("/v1/models?page=t2")
    );

    // A second page that still reports a continuation fails the refresh at the budget.
    let (mock, _) = MockUpstream::start_json(vec![
        json!({"data": [{"id": "m-a"}], "next": "t2"}).to_string(),
        json!({"data": [{"id": "m-b"}], "next": "t3"}).to_string(),
    ])
    .await;

    let detail = failure_of(&provider_with(
        discovery_payload(
            &mock.base(),
            array_extract(),
            json!({
                "type": "token",
                "request_param": "page",
                "response_path": ["next"],
                "max_pages": 2
            }),
        ),
        &mock.base(),
    ))
    .await;

    assert!(
        detail.contains("more pages than the 2-page discovery budget"),
        "got: {detail}"
    );

    // A present non-string, non-null token is a malformed page, never a silent end.
    let (mock, _) =
        MockUpstream::start_json(vec![json!({"data": [], "next": 7}).to_string()]).await;

    let detail = failure_of(&provider_with(
        discovery_payload(
            &mock.base(),
            array_extract(),
            json!({
                "type": "token",
                "request_param": "page",
                "response_path": ["next"],
                "max_pages": 2
            }),
        ),
        &mock.base(),
    ))
    .await;

    assert!(
        detail.contains("malformed next-page token"),
        "got: {detail}"
    );

    Ok(())
}

/// Cursor pagination follows `has_more`/`last_id`; a page claiming more without its last
/// identifier fails.
#[tokio::test]
async fn cursor_pagination_walks_and_needs_its_last_identifier() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded_list) = MockUpstream::start_json(vec![
        json!({"data": [{"id": "m-a"}], "has_more": true, "last_id": "m-a"}).to_string(),
        json!({"data": [{"id": "m-b"}], "has_more": false}).to_string(),
    ])
    .await;

    let models = discover(&provider_with(
        discovery_payload(
            &mock.base(),
            array_extract(),
            json!({
                "type": "cursor",
                "has_more_path": ["has_more"],
                "last_id_path": ["last_id"],
                "request_param": "after",
                "max_pages": 3
            }),
        ),
        &mock.base(),
    ))
    .await?;

    assert_eq!(ids(&models), vec!["m-a".to_string(), "m-b".to_string()]);

    let requests = recorded(&recorded_list)?;

    assert_eq!(
        requests.get(1).map(|request| request.target.as_str()),
        Some("/v1/models?after=m-a")
    );

    // A truthy has-more without a last identifier fails the walk.
    let (mock, _) = MockUpstream::start_json(vec![
        json!({"data": [{"id": "m-a"}], "has_more": true}).to_string(),
    ])
    .await;

    let detail = failure_of(&provider_with(
        discovery_payload(
            &mock.base(),
            array_extract(),
            json!({
                "type": "cursor",
                "has_more_path": ["has_more"],
                "last_id_path": ["last_id"],
                "request_param": "after",
                "max_pages": 3
            }),
        ),
        &mock.base(),
    ))
    .await;

    assert!(
        detail.contains("lacks its last identifier"),
        "got: {detail}"
    );

    Ok(())
}
