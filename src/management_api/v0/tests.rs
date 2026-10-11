//! The version 0 resource tests, driven end to end through the real router against temporary
//! databases: the route registry's declared method/path inventory, the definition versioning
//! flow, the static-key provider lifecycle with its two-state writes, the manual and discovered
//! model rows with their required-nullable overrides, pools, public routes with their automatic
//! protection, serving keys with their secret and policy handling, the OAuth code, device, and
//! reauthorization flows, and the two usage reads.

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking on its own assertions and unreadable fixtures; a \
              # Panics section on every test would restate that"
)]

use super::{
    api_keys::{CreateApiKey, DeleteApiKey, ListApiKeys, UpdateApiKey},
    model_routes::{CreateModelRoute, DeleteModelRoute, ListModelRoutes, UpdateModelRoute},
    models::{CreateModel, DeleteModel, DiscoverModels, ListModels, UpdateModel},
    oauth_flow::{CompleteProviderOauth, ReauthorizeProvider, StartProviderOauth},
    provider_availability::{GetProviderAvailability, ResetProviderAvailability},
    provider_definitions::{
        CreateProviderDefinition, ListProviderDefinitions, UpdateProviderDefinition,
    },
    provider_pools::{
        CreateProviderPool, DeleteProviderPool, ListProviderPools, UpdateProviderPool,
    },
    provider_quota::GetProviderQuota,
    providers::{CreateProvider, DeleteProvider, ListProviders, UpdateProvider},
    usage::{ListUsage, QueryUsage},
};
use crate::{
    accounting::{ApiLogEndpointType, ApiLogOutcome},
    ir::IrUsage,
    management_api::{
        Route as _,
        tests_support::{TestGateway, UPSTREAM_SECRET, VERSION_PREFIX, assert_error_envelope},
    },
    provider::{StoredCredentials, oauth::tests_support as oauth_fixtures},
    storage::api_logs::{self, ApiLogDraft},
    test_support::{
        self,
        mock_upstream::{MockAction, MockUpstream},
    },
};
use http::StatusCode;
use serde_json::{Value, json};
use std::error::Error as StdError;
use time::Date;

/// The static API key of the fixture provider rows; the credential the storage-level credential
/// assertions read back.
const FIXTURE_UPSTREAM_KEY: &str = UPSTREAM_SECRET;

/// The serving secret of the fixture gateway API keys.
const FIXTURE_GATEWAY_KEY: &str = "sk-gateway-fixture";

/// The definition name every fixture definition is created under.
const FIXTURE_DEFINITION: &str = "fixture-definition";

/// The provider name every fixture account is created under.
const FIXTURE_ACCOUNT: &str = "fixture-account";

/// A valid static-key chat-completions definition payload whose inference URL references the
/// base, so every provider bound to it must carry a base URL.
fn static_key_definition() -> Value {
    json!({
        "format": "openai_chat_completion",
        "inference": {
            "url": [{"type": "base"}, {"type": "literal", "value": "/v1/chat/completions"}],
            "upstream_streaming": "match_client",
            "headers": [{"name": "Authorization", "value": [{"type": "secret"}]}]
        },
        "credential": {"type": "api_key"}
    })
}

/// The static-key definition payload extended with the upstream model-list discovery section:
/// one base-addressing `GET` whose array extraction reads `data[].id`.
fn with_member(object: Value, key: &str, value: Value) -> Value {
    let mut object = object;

    object
        .as_object_mut()
        .expect("the fixture is an object")
        .insert(key.to_owned(), value);

    object
}

fn discovery_definition() -> Value {
    let payload = static_key_definition();

    with_member(
        payload,
        "discovery",
        json!({
            "request": {"method": "GET", "url": [
                {"type": "base"},
                {"type": "literal", "value": "/v1/models"},
            ]},
            "extract": {"mode": "array", "array_at": ["data"], "id_field": ["id"]},
            "pagination": {"type": "none"}
        }),
    )
}

/// Creates one definition version through the real route, asserting the 200 envelope and
/// returning the stored version number.
async fn create_definition(
    gateway: &TestGateway,
    name: &str,
    payload: Value,
) -> Result<i64, Box<dyn StdError>> {
    let request = with_member(payload, "name", json!(name));

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/provider-definitions:create"),
            request,
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    Ok(test_support::at(&body, "/provider_definition/version")
        .as_i64()
        .expect("the envelope carries the stored version"))
}

/// Creates one enabled static-key provider through the real route, asserting the 200 envelope
/// and returning the server-assigned row identity.
async fn create_provider(
    gateway: &TestGateway,
    name: &str,
    prefix: &str,
    base_url: &str,
) -> Result<i64, Box<dyn StdError>> {
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/providers:create"),
            json!({
                "model_prefix": prefix,
                "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1,
                "name": name,
                "is_enabled": true,
                "base_url": base_url,
                "api_key": FIXTURE_UPSTREAM_KEY,
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    Ok(test_support::at(&body, "/provider/id")
        .as_i64()
        .expect("the envelope carries the id"))
}

/// Seeds the shared static fixture: one base-addressing static-key definition version 1 and one
/// enabled account named [`FIXTURE_ACCOUNT`] with the `acme.` prefix, returning the provider's
/// row identity.
async fn seed_static_fixture(gateway: &TestGateway) -> Result<i64, Box<dyn StdError>> {
    create_definition(gateway, FIXTURE_DEFINITION, static_key_definition()).await?;

    create_provider(
        gateway,
        FIXTURE_ACCOUNT,
        "acme.",
        "http://127.0.0.1:9/upstream",
    )
    .await
}

/// Records one controlled accounting row into the gateway's api-log store through the real
/// finalization pipeline: the draft is stamped — the shared configuration enables recording —
/// and closed with the given terminal facts.
struct RecordedUsageRow {
    /// Row id of the authenticated gateway API key.
    api_key_id: i64,

    /// Row id of the routed provider.
    provider_id: i64,

    /// Row id of the routed model.
    model_id: i64,

    /// Stable requested public route identity.
    model_route_id: i64,

    /// Recorded outcome of the request.
    outcome: ApiLogOutcome,

    /// HTTP status delivered to the client; `null` when none was produced.
    http_status: Option<i64>,

    /// The row's usage report; `None` leaves the token columns unknown.
    usage: Option<IrUsage>,
}

async fn record_usage_row(gateway: &TestGateway, row: RecordedUsageRow) {
    let mut draft = ApiLogDraft::predispatch(
        row.api_key_id,
        row.provider_id,
        row.model_id,
        row.model_route_id,
        ApiLogEndpointType::OpenaiChatCompletions,
        false,
    );

    draft.stamp();

    api_logs::finalize_api_log(
        gateway.api_log_store(),
        &draft,
        row.outcome,
        row.http_status,
        row.usage,
    )
    .await;

    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
}

/// Lists usage rows with `filter` — omitted when `None` — through the real route.
async fn list_usage(
    gateway: &TestGateway,
    filter: Option<Value>,
) -> Result<Value, Box<dyn StdError>> {
    let request = match filter {
        Some(filter) => json!({"filter": filter}),
        None => json!({}),
    };

    let response = gateway
        .post_management(&format!("{VERSION_PREFIX}/usage:list"), request)
        .await;

    gateway.body_json(response).await
}

/// Aggregates usage buckets over `group_by` with `filter` — omitted when `None` — through the
/// real route.
async fn query_usage(
    gateway: &TestGateway,
    group_by: Value,
    filter: Option<Value>,
) -> Result<Value, Box<dyn StdError>> {
    let request = match filter {
        Some(filter) => json!({"group_by": group_by, "filter": filter}),
        None => json!({"group_by": group_by}),
    };

    let response = gateway
        .post_management(&format!("{VERSION_PREFIX}/usage:query"), request)
        .await;

    gateway.body_json(response).await
}

/// The `YYYY-MM-DD` UTC day of a microsecond stamp, derived through the same Julian-day mapping
/// the api-log partitioning uses.
fn day_key(started_at: i64) -> String {
    const MICROS_PER_DAY: i64 = 86_400_000_000;

    const UNIX_EPOCH_JULIAN_DAY: i32 = 2_440_588;

    let julian_day =
        i32::try_from(started_at.div_euclid(MICROS_PER_DAY) + i64::from(UNIX_EPOCH_JULIAN_DAY))
            .expect("the recorded stamps are inside the supported calendar");

    let date = Date::from_julian_day(julian_day).expect("the recorded stamps are valid dates");

    let (year, month, day) = date.to_calendar_date();

    format!("{year:04}-{:02}-{day:02}", u8::from(month))
}

/// The management headers of an authorized request: the verbatim token and a JSON content type.
fn management_headers() -> Vec<(&'static str, String)> {
    vec![
        (
            "authorization",
            crate::test_support::MANAGEMENT_TOKEN.to_owned(),
        ),
        ("content-type", "application/json".to_owned()),
    ]
}

/// Every route the version declares answers at exactly its declared method and path: the
/// assertion reads each route's method and path from its `route!` declaration — never a
/// hand-copied inventory — and proves the real router serves the declared `POST` there and
/// renders the method-not-allowed 405 for any other method.
#[tokio::test]
async fn every_declared_route_answers_at_its_declared_method_and_path()
-> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    macro_rules! assert_mounted {
        ($($marker:ident),+ $(,)?) => {
            $(
                assert_eq!(
                    $marker::METHOD,
                    http::Method::POST,
                    "every management action is POST"
                );

                let path = format!("{VERSION_PREFIX}{}", $marker::PATH);

                // The declared method at the declared path reaches the handler: an empty body
                // succeeds on the no-argument routes and fails payload validation everywhere
                // else — anything but an unmapped 404.
                let response = gateway.post_management(&path, json!({})).await;

                assert!(
                    response.status() == StatusCode::OK
                        || response.status() == StatusCode::BAD_REQUEST,
                    "{path} must be mounted at its declared path and method"
                );

                // Another method on the registered path is the version's 405, not a 404.
                let response = gateway.request("GET", &path, &management_headers(), None).await;

                let body = assert_error_envelope(
                    &gateway,
                    response,
                    StatusCode::METHOD_NOT_ALLOWED,
                    8,
                )
                .await?;

                assert_eq!(
                    *test_support::at(&body, "/message"),
                    json!(format!("Invalid method (GET {path})")),
                    "{body}"
                );
            )+
        };
    }

    assert_mounted!(
        ListProviderDefinitions,
        CreateProviderDefinition,
        UpdateProviderDefinition,
        ListProviders,
        GetProviderQuota,
        GetProviderAvailability,
        ResetProviderAvailability,
        CreateProvider,
        UpdateProvider,
        DeleteProvider,
        StartProviderOauth,
        CompleteProviderOauth,
        ReauthorizeProvider,
        ListApiKeys,
        CreateApiKey,
        UpdateApiKey,
        DeleteApiKey,
        ListModels,
        DiscoverModels,
        CreateModel,
        UpdateModel,
        DeleteModel,
        ListProviderPools,
        CreateProviderPool,
        UpdateProviderPool,
        DeleteProviderPool,
        ListModelRoutes,
        CreateModelRoute,
        UpdateModelRoute,
        DeleteModelRoute,
        ListUsage,
        QueryUsage,
    );

    Ok(())
}

/// The definition routes store immutable versions: creation answers the version-1 envelope, a
/// repeated name conflicts, an update appends the successor of the exact named base, a stale
/// base conflicts, a non-positive base is the malformed-parameter rejection, and an unknown
/// name renders the not-found pair.
#[tokio::test]
async fn provider_definitions_create_list_and_append_versions() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let path = |action: &str| format!("{VERSION_PREFIX}/provider-definitions:{action}");

    // Creation stores version 1 and answers the identity the caller cannot know.
    let mut create_request = static_key_definition();

    create_request = with_member(create_request, "name", json!(FIXTURE_DEFINITION));

    let response = gateway
        .post_management(&path("create"), create_request)
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/provider_definition/name"),
        FIXTURE_DEFINITION
    );

    assert_eq!(*test_support::at(&body, "/provider_definition/version"), 1);

    assert!(
        test_support::at(&body, "/provider_definition/created_at")
            .as_i64()
            .is_some_and(|at| at > 0),
        "the envelope carries the creation timestamp: {body}"
    );

    assert_eq!(
        *test_support::at(&body, "/provider_definition/format"),
        "openai_chat_completion",
        "the payload is flattened into the row: {body}"
    );

    // The listing carries every stored version, flattened payload included.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/provider_definitions")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(
        *test_support::at(&body, "/provider_definitions/0/credential/type"),
        "api_key",
        "{body}"
    );

    // A repeated name is the definition-name conflict.
    let mut repeated = static_key_definition();

    repeated = with_member(repeated, "name", json!(FIXTURE_DEFINITION));

    let response = gateway.post_management(&path("create"), repeated).await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        format!("Provider definition `{FIXTURE_DEFINITION}` already exists"),
        "{body}"
    );

    // The successor version appends under the exact base.
    let mut successor = static_key_definition();

    successor = with_member(successor, "name", json!(FIXTURE_DEFINITION));

    successor = with_member(successor, "base_version", json!(1));

    let response = gateway.post_management(&path("update"), successor).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/provider_definition/version"),
        2,
        "{body}"
    );

    // A stale base conflicts without writing.
    let mut stale = static_key_definition();

    stale = with_member(stale, "name", json!(FIXTURE_DEFINITION));

    stale = with_member(stale, "base_version", json!(1));

    let response = gateway.post_management(&path("update"), stale).await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("is at version 2")),
        "{body}"
    );

    // A non-positive base version is the shared malformed-parameter rejection.
    let mut non_positive = static_key_definition();

    non_positive = with_member(non_positive, "name", json!(FIXTURE_DEFINITION));

    non_positive = with_member(non_positive, "base_version", json!(0));

    let response = gateway.post_management(&path("update"), non_positive).await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("`base_version` must be a positive")),
        "{body}"
    );

    // An unknown definition name renders the exact referenced pair.
    let mut ghost = static_key_definition();

    ghost = with_member(ghost, "name", json!("ghost"));

    ghost = with_member(ghost, "base_version", json!(1));

    let response = gateway.post_management(&path("update"), ghost).await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Provider definition `ghost` version 1 does not exist",
        "{body}"
    );

    // An unsupported payload variant is rejected before any storage read.
    let response = gateway
        .post_management(
            &path("create"),
            json!({"name": "bad", "format": "not_a_format", "inference": {}, "credential": {}}),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    Ok(())
}

/// The provider routes carry the static-key lifecycle: creation answers the server-assigned row
/// without ever echoing credentials, the definition and boundary validations fire in order, the
/// full-state update keeps the stored key when `api_key` is absent and replaces it when present,
/// an explicit `null` is rejected, a half-named rebind is the pairing rejection, and deletion
/// answers the empty object.
#[tokio::test]
async fn providers_crud_with_two_state_credential_writes() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    create_definition(&gateway, FIXTURE_DEFINITION, static_key_definition()).await?;

    let path = |action: &str| format!("{VERSION_PREFIX}/providers:{action}");

    // Creation answers the wire row: the exact definition pair, the derived format, and no
    // credentials anywhere in the body.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "model_prefix": "acme.",
                "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1,
                "name": FIXTURE_ACCOUNT,
                "is_enabled": true,
                "base_url": "http://127.0.0.1:9/upstream",
                "api_key": FIXTURE_UPSTREAM_KEY,
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/provider/type"),
        "api_key",
        "{body}"
    );

    assert_eq!(*test_support::at(&body, "/provider/name"), FIXTURE_ACCOUNT);

    assert_eq!(
        *test_support::at(&body, "/provider/provider_definition_version"),
        1
    );

    assert_eq!(
        *test_support::at(&body, "/provider/format"),
        "openai_chat_completion"
    );

    let text = body.to_string();

    assert!(
        !text.contains(FIXTURE_UPSTREAM_KEY),
        "credentials are never returned: {text}"
    );

    let provider_id = test_support::at(&body, "/provider/id")
        .as_i64()
        .expect("the row id");

    // The boundary validations fire: an empty name, a secret with whitespace, a base URL the
    // definition requires, an unknown definition version, and a taken name.
    for (request, status, errno) in [
        (
            json!({
                "model_prefix": "", "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1, "name": "", "is_enabled": true,
                "base_url": "http://127.0.0.1:9", "api_key": FIXTURE_UPSTREAM_KEY,
            }),
            StatusCode::BAD_REQUEST,
            1_u16,
        ),
        (
            json!({
                "model_prefix": "", "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1, "name": "spaced", "is_enabled": true,
                "base_url": "http://127.0.0.1:9", "api_key": "has a space",
            }),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({
                "model_prefix": "", "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1, "name": "baseless", "is_enabled": true,
                "base_url": null, "api_key": FIXTURE_UPSTREAM_KEY,
            }),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({
                "model_prefix": "", "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 99, "name": "ghostly", "is_enabled": true,
                "base_url": "http://127.0.0.1:9", "api_key": FIXTURE_UPSTREAM_KEY,
            }),
            StatusCode::NOT_FOUND,
            3,
        ),
    ] {
        let response = gateway.post_management(&path("create"), request).await;

        assert_error_envelope(&gateway, response, status, errno).await?;
    }

    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "model_prefix": "", "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1, "name": FIXTURE_ACCOUNT, "is_enabled": true,
                "base_url": "http://127.0.0.1:9", "api_key": FIXTURE_UPSTREAM_KEY,
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        json!(format!(
            "Provider name `{FIXTURE_ACCOUNT}` is already taken"
        )),
        "{body}"
    );

    // The listing orders rows by name and carries the nullable base URL.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/providers")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(
        *test_support::at(&body, "/providers/0/base_url"),
        "http://127.0.0.1:9/upstream"
    );

    // The full-state update with `api_key` absent keeps the stored credential.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": false,
                "base_url": "http://127.0.0.1:9/upstream",
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let stored = crate::storage::providers::find_provider_by_id(gateway.database(), provider_id)
        .await?
        .expect("the updated row is stored");

    assert_eq!(stored.name, "renamed-account");

    assert!(!stored.is_enabled);

    assert_eq!(
        stored.credentials,
        StoredCredentials::ApiKey {
            api_key: FIXTURE_UPSTREAM_KEY.to_owned(),
        },
        "an absent `api_key` keeps the stored secret"
    );

    // An explicit `null` for the absent-keeps field is rejected.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": false,
                "base_url": "http://127.0.0.1:9/upstream",
                "api_key": null,
            }),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    // A present `api_key` replaces the stored secret.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": true,
                "base_url": "http://127.0.0.1:9/upstream",
                "api_key": "sk-replacement",
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let stored = crate::storage::providers::find_provider_by_id(gateway.database(), provider_id)
        .await?
        .expect("the updated row is stored");

    assert_eq!(
        stored.credentials,
        StoredCredentials::ApiKey {
            api_key: "sk-replacement".to_owned(),
        }
    );

    // A rebind naming only one half of the reference pair is the pairing rejection.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": true,
                "base_url": "http://127.0.0.1:9/upstream",
                "provider_definition_name": FIXTURE_DEFINITION,
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("names both")),
        "{body}"
    );

    // A rebind to an unknown target renders the exact referenced pair.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": true,
                "base_url": "http://127.0.0.1:9/upstream",
                "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 99,
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        format!("Provider definition `{FIXTURE_DEFINITION}` version 99 does not exist"),
        "{body}"
    );

    // A base URL outside the shared contract is the fixed 400.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_id": provider_id,
                "name": "renamed-account",
                "model_prefix": "acme.",
                "is_enabled": true,
                "base_url": "ftp://127.0.0.1:9",
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "`base_url` must be an http:// or https:// URL of printable ASCII without whitespace",
        "{body}"
    );

    // Deletion answers the empty object; an unknown id is the provider not-found.
    let response = gateway
        .post_management(&path("delete"), json!({"provider_id": provider_id}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway
        .post_management(&path("delete"), json!({"provider_id": 424_242}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Provider 424242 does not exist",
        "{body}"
    );

    Ok(())
}

/// The model routes carry the manual-row lifecycle: creation with the complete override state
/// stores the canonicalized overrides, creation with the required-nullable keys omitted — the
/// documented omission-deserializes-as-null tolerance — stores no overrides, natural-key
/// duplicates conflict, every closed vocabulary is enforced, the listing filters by provider
/// with unknown ids answering an empty list rather than not-found, and the row-addressed update
/// and delete round-trip.
#[tokio::test]
async fn models_manual_rows_and_required_nullable_omission() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let provider_id = seed_static_fixture(&gateway).await?;

    let path = |action: &str| format!("{VERSION_PREFIX}/models:{action}");

    // Creation with the complete state answers the applied row: manual state, the canonicalized
    // level set, and the compaction facts.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "provider_id": provider_id,
                "upstream_id": "text-embedded",
                "is_enabled": true,
                "thinking_levels_override": ["high", "low", "high"],
                "max_input_tokens_override": 4096,
                "max_output_tokens_override": null,
                "compaction_support_override": {
                    "responses_standalone_compact": "unknown",
                    "messages_threshold_compaction": "supported"
                },
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/model/discovery_state"),
        "manual",
        "{body}"
    );

    assert_eq!(
        *test_support::at(&body, "/model/thinking_levels_override"),
        json!(["low", "high"]),
        "the override canonicalizes, deduplicated and vocabulary-ordered: {body}"
    );

    assert_eq!(
        *test_support::at(&body, "/model/max_input_tokens_override"),
        4096
    );

    assert_eq!(
        *test_support::at(
            &body,
            "/model/compaction_support_override/messages_threshold_compaction"
        ),
        "supported"
    );

    let model_id = test_support::at(&body, "/model/id")
        .as_i64()
        .expect("the row id");

    // Omission deserializes as null for the required-nullable overrides: the two-state rule's
    // documented tolerance, not a missing-field rejection.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "provider_id": provider_id,
                "upstream_id": "omitted-overrides",
                "is_enabled": false,
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/model/thinking_levels_override"),
        json!(null)
    );

    assert_eq!(
        *test_support::at(&body, "/model/max_input_tokens_override"),
        json!(null)
    );

    assert_eq!(
        *test_support::at(&body, "/model/compaction_support_override"),
        json!(null)
    );

    // A natural-key duplicate is the conflict; the closed vocabularies and the composition
    // contract are the 400s.
    for (request, status, errno, fragment) in [
        (
            json!({
                "provider_id": provider_id, "upstream_id": "text-embedded", "is_enabled": true,
                "thinking_levels_override": null, "max_input_tokens_override": null,
                "max_output_tokens_override": null, "compaction_support_override": null,
            }),
            StatusCode::CONFLICT,
            4_u16,
            "already exists",
        ),
        (
            json!({
                "provider_id": provider_id, "upstream_id": "", "is_enabled": true,
                "thinking_levels_override": null, "max_input_tokens_override": null,
                "max_output_tokens_override": null, "compaction_support_override": null,
            }),
            StatusCode::BAD_REQUEST,
            1,
            "must not be empty",
        ),
        (
            json!({
                "provider_id": provider_id, "upstream_id": "has space", "is_enabled": true,
                "thinking_levels_override": null, "max_input_tokens_override": null,
                "max_output_tokens_override": null, "compaction_support_override": null,
            }),
            StatusCode::BAD_REQUEST,
            1,
            "printable ASCII without whitespace",
        ),
        (
            json!({
                "provider_id": provider_id, "upstream_id": "levels", "is_enabled": true,
                "thinking_levels_override": ["bogus"], "max_input_tokens_override": null,
                "max_output_tokens_override": null, "compaction_support_override": null,
            }),
            StatusCode::BAD_REQUEST,
            1,
            "unknown reasoning effort `bogus`",
        ),
        (
            json!({
                "provider_id": provider_id, "upstream_id": "caps", "is_enabled": true,
                "thinking_levels_override": null, "max_input_tokens_override": 0,
                "max_output_tokens_override": null, "compaction_support_override": null,
            }),
            StatusCode::BAD_REQUEST,
            1,
            "must be a positive integer",
        ),
        (
            json!({
                "provider_id": provider_id, "upstream_id": "facts", "is_enabled": true,
                "thinking_levels_override": null, "max_input_tokens_override": null,
                "max_output_tokens_override": null,
                "compaction_support_override": {"nope": "supported"},
            }),
            StatusCode::BAD_REQUEST,
            1,
            "unknown compaction mechanism or support spelling",
        ),
    ] {
        let response = gateway.post_management(&path("create"), request).await;

        let body = assert_error_envelope(&gateway, response, status, errno).await?;

        assert!(
            test_support::at(&body, "/message")
                .as_str()
                .is_some_and(|message| message.contains(fragment)),
            "{body}"
        );
    }

    // The listing filters by provider; an unknown provider id yields an empty list, and an
    // explicit `null` filter is rejected.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/models").as_array().map(Vec::len),
        Some(2)
    );

    let response = gateway
        .post_management(&path("list"), json!({"provider_id": provider_id}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/models").as_array().map(Vec::len),
        Some(2)
    );

    let response = gateway
        .post_management(&path("list"), json!({"provider_id": 999_999}))
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/models"),
        json!([]),
        "a filter addresses no row: {body}"
    );

    let response = gateway
        .post_management(&path("list"), json!({"provider_id": null}))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    // The row-addressed update replaces the complete writable state: the explicit `null`
    // override clears the stored pin, and the omitted keys are null exactly as on creation.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "model_id": model_id,
                "is_enabled": false,
                "thinking_levels_override": null,
                "max_input_tokens_override": 8192,
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert!(
        !test_support::at(&body, "/model/is_enabled")
            .as_bool()
            .expect("the switch")
    );

    assert_eq!(
        *test_support::at(&body, "/model/thinking_levels_override"),
        json!(null)
    );

    assert_eq!(
        *test_support::at(&body, "/model/max_input_tokens_override"),
        8192
    );

    // The update cannot address an unknown row.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "model_id": 123_456, "is_enabled": true, "thinking_levels_override": null,
                "max_input_tokens_override": null, "max_output_tokens_override": null,
                "compaction_support_override": null,
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Model 123456 does not exist",
        "{body}"
    );

    // Deletion answers the empty object; the deleted row is gone.
    let response = gateway
        .post_management(&path("delete"), json!({"model_id": model_id}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway
        .post_management(&path("delete"), json!({"model_id": model_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        json!(format!("Model {model_id} does not exist")),
        "{body}"
    );

    Ok(())
}

/// The discovery route refreshes the provider's model rows from upstream: a two-model report
/// stores both rows as discovered, a one-model report marks the vanished remainder while the
/// administrator's switch and overrides survive, an empty report is the upstream-error refusal,
/// and an unknown provider is not-found.
#[tokio::test]
async fn models_discovery_refresh_preserves_administrator_state() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) = MockUpstream::start_json(vec![
        json!({"data": [{"id": "m-a"}, {"id": "m-b"}]}).to_string(),
        json!({"data": [{"id": "m-a"}]}).to_string(),
    ])
    .await;

    let gateway = TestGateway::open().await;

    create_definition(&gateway, FIXTURE_DEFINITION, discovery_definition()).await?;

    let provider_id = create_provider(&gateway, FIXTURE_ACCOUNT, "acme.", &mock.base()).await?;

    let path = |action: &str| format!("{VERSION_PREFIX}/models:{action}");

    // The first refresh stores both rows, discovered.
    let response = gateway
        .post_management(&path("discover"), json!({"provider_id": provider_id}))
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/models").as_array().map(Vec::len),
        Some(2),
        "{body}"
    );

    assert!(
        test_support::at(&body, "/models")
            .as_array()
            .expect("the rows")
            .iter()
            .all(|row| test_support::at(row, "/discovery_state") == "discovered")
    );

    // The administrator pins state on one row before the next refresh.
    let model_a = test_support::at(&body, "/models")
        .as_array()
        .expect("the rows")
        .iter()
        .find(|row| *test_support::at(row, "/upstream_id") == "m-a")
        .expect("the m-a row")["id"]
        .as_i64()
        .expect("the row id");

    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "model_id": model_a, "is_enabled": false, "thinking_levels_override": ["low"],
                "max_input_tokens_override": null, "max_output_tokens_override": null,
                "compaction_support_override": null,
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    // The second refresh vanishes the unlisted row and never touches the administrator's state.
    let response = gateway
        .post_management(&path("discover"), json!({"provider_id": provider_id}))
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    let vanished = test_support::at(&body, "/models")
        .as_array()
        .expect("the rows")
        .iter()
        .find(|row| *test_support::at(row, "/upstream_id") == "m-b")
        .expect("the vanished row");

    assert_eq!(
        *test_support::at(vanished, "/discovery_state"),
        "vanished",
        "{body}"
    );

    let kept = test_support::at(&body, "/models")
        .as_array()
        .expect("the rows")
        .iter()
        .find(|row| *test_support::at(row, "/upstream_id") == "m-a")
        .expect("the surviving row");

    assert_eq!(
        *test_support::at(kept, "/is_enabled"),
        false,
        "the switch survives: {body}"
    );

    assert_eq!(
        test_support::at(kept, "/thinking_levels_override"),
        &json!(["low"]),
        "the override survives: {body}"
    );

    // The refresh addressed the definition's discovery request once per call.
    assert_eq!(requests.lock().expect("the recordings").len(), 2);

    // An upstream reporting no models is the upstream-error refusal: the curated rows are never
    // wiped by an empty replacement.
    let (empty_mock, _) = MockUpstream::start_json(vec![json!({"data": []}).to_string()]).await;

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/providers:update"),
            json!({
                "provider_id": provider_id,
                "name": FIXTURE_ACCOUNT,
                "model_prefix": "acme.",
                "is_enabled": true,
                "base_url": empty_mock.base(),
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let response = gateway
        .post_management(&path("discover"), json!({"provider_id": provider_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_GATEWAY, 5).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("no models")),
        "{body}"
    );

    // An unknown provider is the not-found.
    let response = gateway
        .post_management(&path("discover"), json!({"provider_id": 999_999}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Provider 999999 does not exist",
        "{body}"
    );

    Ok(())
}

/// The pool routes carry the complete authored state atomically: creation answers the new
/// identity, the listing orders pools by name with members in priority order, duplicate member
/// providers or priorities are client faults, unknown member providers are not-found, and a
/// pool an authored route references conflicts on deletion.
#[tokio::test]
async fn provider_pools_crud_with_membership_validation() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let first = seed_static_fixture(&gateway).await?;

    let second = create_provider(&gateway, "second-account", "", "http://127.0.0.1:9/b").await?;

    let path = |action: &str| format!("{VERSION_PREFIX}/provider-pools:{action}");

    // Creation answers the server-assigned identity.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "name": "duo",
                "model_prefix": "duo.",
                "is_enabled": true,
                "members": [
                    {"provider_id": first, "priority": 5},
                    {"provider_id": second, "priority": 1},
                ],
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    let pool_id = test_support::at(&body, "/provider_pool_id")
        .as_i64()
        .expect("the pool id");

    // The listing orders by name and members by priority.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/provider_pools")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let members = test_support::at(&body, "/provider_pools/0/members")
        .as_array()
        .expect("the members");

    assert_eq!(
        members
            .iter()
            .map(|member| test_support::at(member, "/provider_id").as_i64())
            .collect::<Vec<_>>(),
        vec![Some(second), Some(first)],
        "members are listed in priority order: {body}"
    );

    // Duplicate providers, duplicate priorities, an empty name, and an unknown member are the
    // documented faults.
    for (request, status, errno) in [
        (
            json!({
                "name": "duplicated", "model_prefix": "", "is_enabled": true,
                "members": [
                    {"provider_id": first, "priority": 1},
                    {"provider_id": first, "priority": 2},
                ],
            }),
            StatusCode::BAD_REQUEST,
            1_u16,
        ),
        (
            json!({
                "name": "colliding", "model_prefix": "", "is_enabled": true,
                "members": [
                    {"provider_id": first, "priority": 1},
                    {"provider_id": second, "priority": 1},
                ],
            }),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({"name": "", "model_prefix": "", "is_enabled": true, "members": []}),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({
                "name": "ghosted", "model_prefix": "", "is_enabled": true,
                "members": [{"provider_id": 999_999, "priority": 1}],
            }),
            StatusCode::NOT_FOUND,
            3,
        ),
    ] {
        let response = gateway.post_management(&path("create"), request).await;

        assert_error_envelope(&gateway, response, status, errno).await?;
    }

    // The update replaces the complete state, identity preserved.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "provider_pool_id": pool_id,
                "name": "trio",
                "model_prefix": "",
                "is_enabled": false,
                "members": [{"provider_id": second, "priority": 9}],
            }),
        )
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(*test_support::at(&body, "/provider_pools/0/name"), "trio");

    assert_eq!(
        test_support::at(&body, "/provider_pools/0/members")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // A pool an authored route references conflicts on deletion; the unreferenced pool deletes.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/model-routes:create"),
            json!({
                "public_id": "pooled-alias",
                "is_enabled": true,
                "target": {"type": "pool", "pool_id": pool_id, "upstream_id": "any-model"},
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let response = gateway
        .post_management(&path("delete"), json!({"provider_pool_id": pool_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("referenced by a model route")),
        "{body}"
    );

    let response = gateway
        .post_management(&path("delete"), json!({"provider_pool_id": 999_999}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Provider pool 999999 does not exist",
        "{body}"
    );

    Ok(())
}

/// The public-route routes carry the alias lifecycle over the automatic namespace: automatic
/// entries are listed beside authored ones, an authored alias is created, retargeted with its
/// identity preserved, and deleted, a public identifier outside the routing-token contract is
/// rejected, and the automatic rows are mutation-protected conflicts.
#[tokio::test]
async fn model_routes_authoring_with_automatic_protection() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let provider_id = seed_static_fixture(&gateway).await?;

    // One manual model row on the prefixed account materializes the automatic direct entry.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/models:create"),
            json!({
                "provider_id": provider_id,
                "upstream_id": "mdl",
                "is_enabled": true,
                "thinking_levels_override": null,
                "max_input_tokens_override": null,
                "max_output_tokens_override": null,
                "compaction_support_override": null,
            }),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let model_id = test_support::at(&body, "/model/id")
        .as_i64()
        .expect("the model id");

    let path = |action: &str| format!("{VERSION_PREFIX}/model-routes:{action}");

    // The listing carries the automatic entry: the composed public id and its direct target.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    let automatic = test_support::at(&body, "/model_routes")
        .as_array()
        .expect("the routes")
        .iter()
        .find(|row| *test_support::at(row, "/is_automatic") == true)
        .expect("the automatic entry")
        .clone();

    assert_eq!(
        *test_support::at(&automatic, "/public_id"),
        "acme.mdl",
        "{body}"
    );

    assert_eq!(
        *test_support::at(&automatic, "/target"),
        json!({"type": "direct", "model_id": model_id})
    );

    let automatic_id = test_support::at(&automatic, "/id")
        .as_i64()
        .expect("the automatic id");

    // An authored alias is created and answered with its server-assigned identity.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "public_id": "alias-one",
                "is_enabled": true,
                "target": {"type": "direct", "model_id": model_id},
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    let alias_id = test_support::at(&body, "/model_route_id")
        .as_i64()
        .expect("the route id");

    // A pool route may name a currently absent upstream id, but its pool must exist.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/provider-pools:create"),
            json!({"name": "empty", "model_prefix": "", "is_enabled": true, "members": []}),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let pool_id = test_support::at(&body, "/provider_pool_id")
        .as_i64()
        .expect("the pool id");

    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "public_id": "pool-alias",
                "is_enabled": true,
                "target": {"type": "pool", "pool_id": pool_id, "upstream_id": "future-model"},
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    // The closed faults: a public id with routing-hostile characters, a public-id collision,
    // and unknown targets.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "public_id": "has:colon", "is_enabled": true,
                "target": {"type": "direct", "model_id": model_id},
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("`public_id`")),
        "{body}"
    );

    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "public_id": "alias-one", "is_enabled": true,
                "target": {"type": "direct", "model_id": model_id},
            }),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "public_id": "ghosted", "is_enabled": true,
                "target": {"type": "direct", "model_id": 999_999},
            }),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    // The retarget preserves the route identity and may keep its public name.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "model_route_id": alias_id,
                "public_id": "alias-one",
                "is_enabled": false,
                "target": {"type": "pool", "pool_id": pool_id, "upstream_id": "retargeted"},
            }),
        )
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    let retargeted = test_support::at(&body, "/model_routes")
        .as_array()
        .expect("the routes")
        .iter()
        .find(|row| *test_support::at(row, "/id") == alias_id)
        .expect("the retargeted row");

    assert_eq!(*test_support::at(retargeted, "/public_id"), "alias-one");

    assert_eq!(
        *test_support::at(retargeted, "/target"),
        json!({"type": "pool", "pool_id": pool_id, "upstream_id": "retargeted"}),
        "{body}"
    );

    // The automatic rows are conflicts for every mutation.
    for action in ["update", "delete"] {
        let mut request = json!({"model_route_id": automatic_id});

        if action == "update" {
            request = with_member(request, "public_id", json!("renamed"));
            request = with_member(request, "is_enabled", json!(true));
            request = with_member(
                request,
                "target",
                json!({"type": "direct", "model_id": model_id}),
            );
        }

        let response = gateway.post_management(&path(action), request).await;

        let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

        assert!(
            test_support::at(&body, "/message")
                .as_str()
                .is_some_and(|message| message.contains("Automatic routes")),
            "{body}"
        );
    }

    // Deletion removes only the authored row.
    let response = gateway
        .post_management(&path("delete"), json!({"model_route_id": alias_id}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway
        .post_management(&path("delete"), json!({"model_route_id": alias_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        json!(format!("Model route {alias_id} does not exist")),
        "{body}"
    );

    Ok(())
}

/// The serving-key routes return plaintext secrets because operators configure clients, enforce
/// the complete access-control write: duplicate typed owners are 400, unknown references 404,
/// a duplicate secret 409 without disclosing it, explicit equal-default pins stay stored, and
/// the full-state update and delete round-trip.
#[tokio::test]
async fn api_keys_secrets_policies_and_duplicates() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    let provider_id = seed_static_fixture(&gateway).await?;

    let path = |action: &str| format!("{VERSION_PREFIX}/api-keys:{action}");

    let policy = |owner_defaults: Value, route_overrides: Value| {
        json!({
            "default_access": "allow",
            "owner_defaults": owner_defaults,
            "route_overrides": route_overrides,
        })
    };

    // Creation answers the row with its plaintext secret and the server-assigned facts.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "name": "ops",
                "key": FIXTURE_GATEWAY_KEY,
                "is_enabled": true,
                "access_control": policy(
                    json!([{"type": "provider", "provider_id": provider_id, "access": "allow"}]),
                    json!([]),
                ),
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/api_key/key"),
        FIXTURE_GATEWAY_KEY,
        "{body}"
    );

    assert!(
        test_support::at(&body, "/api_key/id")
            .as_i64()
            .is_some_and(|id| id > 0)
    );

    let key_id = test_support::at(&body, "/api_key/id")
        .as_i64()
        .expect("the key id");

    // An explicit pin equal to the key default stays stored: inheritance is absence, not a
    // wire decision.
    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/api_keys")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(
        *test_support::at(&body, "/api_keys/0/access_control/owner_defaults"),
        json!([{"type": "provider", "provider_id": provider_id, "access": "allow"}]),
        "{body}"
    );

    // A duplicate secret is the conflict, never disclosing the value.
    let response = gateway
        .post_management(
            &path("create"),
            json!({
                "name": "second",
                "key": FIXTURE_GATEWAY_KEY,
                "is_enabled": true,
                "access_control": policy(json!([]), json!([])),
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "API key is already taken",
        "{body}"
    );

    // The boundary faults: an empty name, a non-token secret, duplicate typed owners, and
    // unknown references.
    for (request, status, errno) in [
        (
            json!({
                "name": "", "key": "sk-other", "is_enabled": true,
                "access_control": policy(json!([]), json!([])),
            }),
            StatusCode::BAD_REQUEST,
            1_u16,
        ),
        (
            json!({
                "name": "spaced", "key": "has space", "is_enabled": true,
                "access_control": policy(json!([]), json!([])),
            }),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({
                "name": "doubled", "key": "sk-other", "is_enabled": true,
                "access_control": policy(
                    json!([
                        {"type": "provider", "provider_id": provider_id, "access": "allow"},
                        {"type": "provider", "provider_id": provider_id, "access": "deny"},
                    ]),
                    json!([]),
                ),
            }),
            StatusCode::BAD_REQUEST,
            1,
        ),
        (
            json!({
                "name": "ghosted", "key": "sk-other", "is_enabled": true,
                "access_control": policy(
                    json!([{"type": "provider", "provider_id": 999_999, "access": "allow"}]),
                    json!([]),
                ),
            }),
            StatusCode::NOT_FOUND,
            3,
        ),
        (
            json!({
                "name": "ghosted-route", "key": "sk-other", "is_enabled": true,
                "access_control": policy(
                    json!([]),
                    json!([{"model_route_id": 999_999, "access": "allow"}]),
                ),
            }),
            StatusCode::NOT_FOUND,
            3,
        ),
    ] {
        let response = gateway.post_management(&path("create"), request).await;

        assert_error_envelope(&gateway, response, status, errno).await?;
    }

    // The full-state update replaces the complete row: the new secret, switch, and policy.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "api_key_id": key_id,
                "name": "ops-2",
                "key": "sk-gateway-fixture-2",
                "is_enabled": false,
                "access_control": {
                    "default_access": "deny",
                    "owner_defaults": [],
                    "route_overrides": [],
                },
            }),
        )
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway.post_management(&path("list"), json!({})).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(*test_support::at(&body, "/api_keys/0/name"), "ops-2");

    assert_eq!(*test_support::at(&body, "/api_keys/0/is_enabled"), false);

    assert_eq!(
        *test_support::at(&body, "/api_keys/0/key"),
        "sk-gateway-fixture-2"
    );

    assert_eq!(
        *test_support::at(&body, "/api_keys/0/access_control/default_access"),
        "deny",
        "{body}"
    );

    // The update cannot address an unknown row.
    let response = gateway
        .post_management(
            &path("update"),
            json!({
                "api_key_id": 999_999, "name": "ghost", "key": "sk-ghost", "is_enabled": true,
                "access_control": {
                    "default_access": "allow",
                    "owner_defaults": [],
                    "route_overrides": [],
                },
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "API key 999999 does not exist",
        "{body}"
    );

    // Deletion revokes the row.
    let response = gateway
        .post_management(&path("delete"), json!({"api_key_id": key_id}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({}));

    let response = gateway
        .post_management(&path("delete"), json!({"api_key_id": key_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        json!(format!("API key {key_id} does not exist")),
        "{body}"
    );

    Ok(())
}

/// The authorization-code OAuth flow runs its full lifecycle through the routes: the begin
/// answers the grant-tagged URL, state, and ten-minute expiry, the completion exchanges the
/// pasted code at the definition's endpoint and stores the provider without echoing secrets, a
/// consumed or unknown state is not-found, a paste mistake is rejected without burning the
/// flow, the grant mismatch is not-found, and a taken creation name is the begin-time conflict.
#[tokio::test]
async fn oauth_authorization_code_flow_lifecycle() -> Result<(), Box<dyn StdError>> {
    let (mock, requests) = MockUpstream::start_json(vec![
        json!({
            "access_token": "at-from-exchange",
            "refresh_token": "rt-from-exchange",
            "expires_in": 3600,
        })
        .to_string(),
    ])
    .await;

    let gateway = TestGateway::open().await;

    create_definition(
        &gateway,
        "oauth-def",
        oauth_fixtures::code_payload(&mock.base()),
    )
    .await?;

    let start = |name: &str| {
        json!({
            "model_prefix": "",
            "provider_definition_name": "oauth-def",
            "provider_definition_version": 1,
            "name": name,
            "base_url": mock.base(),
        })
    };

    // The begin answers the grant-tagged facts; the expiry is the code flow's ten minutes.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            start("oauth-acct"),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/grant"),
        "authorization_code",
        "{body}"
    );

    let authorize_url = test_support::at(&body, "/authorize_url")
        .as_str()
        .expect("the url");

    assert!(
        authorize_url.starts_with("https://up.test/authorize"),
        "{body}"
    );

    assert!(authorize_url.contains("client_id=client-1"), "{body}");

    let state = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    let expires_at = test_support::at(&body, "/expires_at")
        .as_i64()
        .expect("the expiry");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros() as i64;

    assert!(
        expires_at > now && expires_at <= now + 630_000_000,
        "the code flow expires in ten minutes: {expires_at} against {now}"
    );

    // The completion exchanges the pasted code and stores the provider.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state, "code": "the-code"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(*test_support::at(&body, "/status"), "succeeded", "{body}");

    assert_eq!(
        *test_support::at(&body, "/provider/type"),
        "oauth",
        "{body}"
    );

    assert_eq!(
        *test_support::at(&body, "/provider/name"),
        "oauth-acct",
        "{body}"
    );

    assert!(
        test_support::at(&body, "/provider/expires_at").is_i64(),
        "{body}"
    );

    let text = body.to_string();

    assert!(
        !text.contains("at-from-exchange") && !text.contains("rt-from-exchange"),
        "OAuth credentials are never returned: {text}"
    );

    // The exchange hit the definition's token endpoint with the pasted code.
    {
        let recorded = requests.lock().expect("the recordings");

        let exchange = recorded.first().expect("the exchange request");

        assert_eq!(recorded.len(), 1);

        assert_eq!(exchange.target, "/token");

        assert!(exchange.body.contains("code=the-code"), "{}", exchange.body);

        assert!(
            exchange.body.contains("grant_type=authorization_code"),
            "{}",
            exchange.body
        );
    }

    // The created row is listed as an OAuth account.
    let response = gateway
        .post_management(&format!("{VERSION_PREFIX}/providers:list"), json!({}))
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/providers/0/type"),
        "oauth",
        "{body}"
    );

    // The consumed state is spent: exactly one completion observes it.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state, "code": "the-code"}),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "OAuth state is unknown, expired, or already consumed",
        "{body}"
    );

    // An unknown state is the same not-found.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": "ghost", "code": "the-code"}),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    // A paste mistake is rejected without burning the flow.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            start("oauth-acct-2"),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let state_two = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({
                "grant": "authorization_code", "state": state_two,
                "code": "http://callback?code=oops",
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("opaque authorization code")),
        "{body}"
    );

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state_two, "code": " good-code "}),
        )
        .await;

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "the flow survived the paste mistake"
    );

    // A device-form completion of a code-flow state is the grant mismatch, and it does not
    // consume the flow either.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            start("oauth-acct-3"),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let state_three = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "device", "state": state_three}),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("authorization_code")),
        "{body}"
    );

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state_three, "code": "third"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    // A taken creation name is the begin-time conflict, before any upstream round-trip.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            start("oauth-acct"),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::CONFLICT, 4).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        json!("Provider name `oauth-acct` is already taken"),
        "{body}"
    );

    // A flow cannot run against a static-key definition.
    create_definition(&gateway, FIXTURE_DEFINITION, static_key_definition()).await?;

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            json!({
                "model_prefix": "",
                "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1,
                "name": "static-flow",
                "base_url": mock.base(),
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("authorizes no flow")),
        "{body}"
    );

    Ok(())
}

/// The reauthorization route begins a flow against the provider's own definition pair or an
/// explicitly named OAuth target, the completion replaces the credentials while retaining the
/// account identity, the half-named target is the pairing rejection, and a static-key row
/// authorizes no flow.
#[tokio::test]
async fn oauth_reauthorize_retains_identity_and_gates_targets() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_json(vec![
        json!({
            "access_token": "at-refreshed",
            "refresh_token": "rt-refreshed",
            "expires_in": 3600,
        })
        .to_string(),
    ])
    .await;

    let gateway = TestGateway::open().await;

    create_definition(
        &gateway,
        "oauth-def",
        oauth_fixtures::code_payload(&mock.base()),
    )
    .await?;

    create_definition(&gateway, FIXTURE_DEFINITION, static_key_definition()).await?;

    // One OAuth provider through a full flow, and one static-key account.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            json!({
                "model_prefix": "authed.",
                "provider_definition_name": "oauth-def",
                "provider_definition_version": 1,
                "name": "oauth-acct",
                "base_url": mock.base(),
            }),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let state = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state, "code": "first"}),
        )
        .await;

    let body = gateway.body_json(response).await?;

    let oauth_provider_id = test_support::at(&body, "/provider/id")
        .as_i64()
        .expect("the provider id");

    let static_provider_id =
        create_provider(&gateway, FIXTURE_ACCOUNT, "", "http://127.0.0.1:9/upstream").await?;

    let path = format!("{VERSION_PREFIX}/oauth:reauthorize");

    // The default target is the provider's own pair; the begin answers the grant-tagged facts.
    let response = gateway
        .post_management(&path, json!({"provider_id": oauth_provider_id}))
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/grant"),
        "authorization_code",
        "{body}"
    );

    let state = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    // The completion replaces the credentials and retains the account identity.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state, "code": "renewed"}),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(
        *test_support::at(&body, "/provider/id"),
        oauth_provider_id,
        "{body}"
    );

    assert_eq!(*test_support::at(&body, "/provider/name"), "oauth-acct");

    assert_eq!(
        *test_support::at(&body, "/provider/model_prefix"),
        "authed."
    );

    // A half-named explicit target is the pairing rejection.
    let response = gateway
        .post_management(
            &path,
            json!({
                "provider_id": oauth_provider_id,
                "provider_definition_name": "oauth-def",
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("names both")),
        "{body}"
    );

    // An unknown provider is the not-found.
    let response = gateway
        .post_management(&path, json!({"provider_id": 999_999}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "Provider 999999 does not exist",
        "{body}"
    );

    // A static-key provider authorizes no flow on its own pair.
    let response = gateway
        .post_management(&path, json!({"provider_id": static_provider_id}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("authorizes no flow")),
        "{body}"
    );

    // Nor against an explicitly named static-key target.
    let response = gateway
        .post_management(
            &path,
            json!({
                "provider_id": static_provider_id,
                "provider_definition_name": FIXTURE_DEFINITION,
                "provider_definition_version": 1,
            }),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("authorizes no flow")),
        "{body}"
    );

    Ok(())
}

/// The device OAuth flow starts against the upstream device-authorization endpoint, answers the
/// verification facts, and reads its pending outcome without consuming: the mismatched grant
/// form is not-found and leaves the flow readable, while a device-authorization upstream
/// failure is the 502.
#[tokio::test]
async fn oauth_device_flow_starts_and_reads_pending() -> Result<(), Box<dyn StdError>> {
    let device_json = json!({
        "device_code": "dc-1",
        "user_code": "ABCD-EFGH",
        "verification_uri": "https://up.test/activate",
        "expires_in": 600,
        "interval": 3600,
    });

    let device_body = device_json.to_string();

    let pending_body = json!({"error": "authorization_pending"}).to_string();

    let (mock, requests) = MockUpstream::start_raw(vec![
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{device_body}",
            device_body.len()
        ),
        format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n{pending_body}",
            pending_body.len()
        ),
    ])
    .await;

    let gateway = TestGateway::open().await;

    create_definition(
        &gateway,
        "device-def",
        oauth_fixtures::device_payload(&mock.base()),
    )
    .await?;

    // The begin answers the device facts: the user code, the verification URI, and the
    // response-derived expiry.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            json!({
                "model_prefix": "",
                "provider_definition_name": "device-def",
                "provider_definition_version": 1,
                "name": "device-acct",
                "base_url": mock.base(),
            }),
        )
        .await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    assert_eq!(*test_support::at(&body, "/grant"), "device", "{body}");

    assert_eq!(
        *test_support::at(&body, "/user_code"),
        "ABCD-EFGH",
        "{body}"
    );

    assert_eq!(
        *test_support::at(&body, "/verification_uri"),
        "https://up.test/activate"
    );

    assert!(
        body.get("verification_uri_complete").is_none(),
        "absent response facts stay absent: {body}"
    );

    let state = test_support::at(&body, "/state")
        .as_str()
        .expect("the state")
        .to_owned();

    let expires_at = test_support::at(&body, "/expires_at")
        .as_i64()
        .expect("the expiry");

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_micros() as i64;

    assert!(
        expires_at > now && expires_at <= now + 605_000_000,
        "the device expiry follows the response's 600 seconds"
    );

    // The device-authorization request carried the client identity.
    {
        let recorded = requests.lock().expect("the recordings");

        let authorization = recorded.first().expect("the device authorization request");

        assert_eq!(recorded.len(), 1);

        assert_eq!(authorization.target, "/device");
    }

    // The completion reads pending without consuming, repeatedly.
    for _ in 0..2 {
        let response = gateway
            .post_management(
                &format!("{VERSION_PREFIX}/oauth:complete"),
                json!({"grant": "device", "state": state}),
            )
            .await;

        assert_eq!(response.status(), StatusCode::OK);

        let body = gateway.body_json(response).await?;

        assert_eq!(body, json!({"status": "pending"}), "{body}");
    }

    // A code-form completion of a device state is the grant mismatch and does not consume the
    // flow: the pending read survives it.
    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "authorization_code", "state": state, "code": "oops"}),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::NOT_FOUND, 3).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("device")),
        "{body}"
    );

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:complete"),
            json!({"grant": "device", "state": state}),
        )
        .await;

    let body = gateway.body_json(response).await?;

    assert_eq!(body, json!({"status": "pending"}), "{body}");

    Ok(())
}

/// The device-authorization begin maps a failed upstream round-trip onto the upstream-error
/// category: the transport failure is the 502, never an internal or client fault.
#[tokio::test]
async fn oauth_device_start_maps_upstream_failure_to_bad_gateway() -> Result<(), Box<dyn StdError>>
{
    let (mock, _) = MockUpstream::start(vec![MockAction::Close]).await;

    let gateway = TestGateway::open().await;

    create_definition(
        &gateway,
        "device-def",
        oauth_fixtures::device_payload(&mock.base()),
    )
    .await?;

    let response = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/oauth:start"),
            json!({
                "model_prefix": "",
                "provider_definition_name": "device-def",
                "provider_definition_version": 1,
                "name": "device-acct",
                "base_url": mock.base(),
            }),
        )
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_GATEWAY, 5).await?;

    Ok(())
}

/// The usage listing mirrors the recorded rows newest-first with the row's own facts and
/// nothing else, applies the shared filters — within-field OR, across-field AND, unknown ids
/// matching empty selections, null statuses unselectable through the status array — and serves
/// the exclusive cursor pagination with its bounded page size.
#[tokio::test]
async fn usage_list_rows_filters_and_pagination() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    // Three rows with distinct stamps: two completed with usage, one aborted with neither
    // status nor usage.
    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 21,
            provider_id: 11,
            model_id: 31,
            model_route_id: 41,
            outcome: ApiLogOutcome::Completed,
            http_status: Some(200),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 20,
                cached_tokens: Some(5),
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        },
    )
    .await;

    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 22,
            provider_id: 12,
            model_id: 32,
            model_route_id: 42,
            outcome: ApiLogOutcome::Completed,
            http_status: Some(200),
            usage: Some(IrUsage {
                input_tokens: 100,
                output_tokens: 200,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        },
    )
    .await;

    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 23,
            provider_id: 13,
            model_id: 33,
            model_route_id: 43,
            outcome: ApiLogOutcome::ClientAborted,
            http_status: None,
            usage: None,
        },
    )
    .await;

    let path = format!("{VERSION_PREFIX}/usage:list");

    // The unfiltered page lists every row newest-first, each row mirroring its sixteen recorded
    // columns and nothing else.
    let response = gateway.post_management(&path, json!({})).await;

    assert_eq!(response.status(), StatusCode::OK);

    let body = gateway.body_json(response).await?;

    let rows = test_support::at(&body, "/api_logs")
        .as_array()
        .expect("the rows");

    assert_eq!(rows.len(), 3);

    let stamp = |key_id: i64| {
        rows.iter()
            .find(|row| *test_support::at(row, "/api_key_id") == key_id)
            .expect("the recorded row")
            .pointer("/started_at")
            .expect("the stamp")
            .as_i64()
            .expect("the number")
    };

    let stamps: Vec<i64> = rows
        .iter()
        .map(|row| {
            test_support::at(row, "/started_at")
                .as_i64()
                .expect("the stamp")
        })
        .collect();

    assert!(
        stamps
            .iter()
            .zip(stamps.iter().skip(1))
            .all(|(newer, older)| newer > older),
        "rows are newest-first: {body}"
    );

    let first = rows
        .iter()
        .find(|row| *test_support::at(row, "/api_key_id") == 21)
        .expect("row one");

    assert_eq!(first.as_object().map(|fields| fields.len()), Some(16));

    assert_eq!(*test_support::at(first, "/provider_id"), 11);

    assert_eq!(*test_support::at(first, "/model_id"), 31);

    assert_eq!(*test_support::at(first, "/model_route_id"), 41);

    assert_eq!(
        *test_support::at(first, "/endpoint_type"),
        "openai_chat_completions"
    );

    assert_eq!(*test_support::at(first, "/is_stream"), false);

    assert_eq!(*test_support::at(first, "/http_status"), 200);

    assert_eq!(*test_support::at(first, "/outcome"), "completed");

    assert_eq!(*test_support::at(first, "/input_tokens"), 10);

    assert_eq!(*test_support::at(first, "/output_tokens"), 20);

    assert_eq!(*test_support::at(first, "/cached_tokens"), 5);

    assert_eq!(*test_support::at(first, "/cache_write_tokens"), json!(null));

    assert_eq!(*test_support::at(first, "/reasoning_tokens"), json!(null));

    assert_eq!(
        *test_support::at(first, "/upstream_http_status"),
        json!(null)
    );

    let stamp_two = stamp(22);

    // Within-field matches OR; unknown ids match empty selections without lookup errors.
    let body = list_usage(&gateway, Some(json!({"provider_id": [12]}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(*test_support::at(&body, "/api_logs/0/api_key_id"), 22);

    let body = list_usage(&gateway, Some(json!({"provider_id": [11, 13]}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let body = list_usage(&gateway, Some(json!({"provider_id": [11, 999_999]}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // Fields combine AND.
    let body = list_usage(
        &gateway,
        Some(json!({"provider_id": [11], "outcome": ["completed"]})),
    )
    .await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let body = list_usage(&gateway, Some(json!({"outcome": ["upstream_error"]}))).await?;

    assert_eq!(*test_support::at(&body, "/api_logs"), json!([]), "{body}");

    // Null recorded statuses cannot be selected through the status array.
    let body = list_usage(&gateway, Some(json!({"http_status": [200]}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    // The time bounds: `from` is inclusive, `to` exclusive.
    let body = list_usage(&gateway, Some(json!({"from": stamp_two}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(2)
    );

    let body = list_usage(&gateway, Some(json!({"to": stamp_two}))).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(*test_support::at(&body, "/api_logs/0/api_key_id"), 21);

    // The cursor is an exclusive timestamp bound, composable with the page size.
    let mut request = json!({});

    request = with_member(request, "before", json!(stamp_two));

    request = with_member(request, "limit", json!(1));

    let response = gateway.post_management(&path, request).await;

    let body = gateway.body_json(response).await?;

    assert_eq!(
        test_support::at(&body, "/api_logs")
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    assert_eq!(*test_support::at(&body, "/api_logs/0/api_key_id"), 21);

    // The page size is bounded: 0 and 1001 are the fixed 400s.
    for limit in [0, 1001] {
        let mut request = json!({});

        request = with_member(request, "limit", json!(limit));

        let response = gateway.post_management(&path, request).await;

        let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

        assert_eq!(
            *test_support::at(&body, "/message"),
            "`limit` must be between 1 and 1000",
            "{body}"
        );
    }

    // The filter fields are absent-keeps: an explicit `null` is rejected, an empty array is
    // malformed, and a field over its value cap names the field.
    let response = gateway
        .post_management(&path, json!({"filter": {"provider_id": null}}))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    let response = gateway
        .post_management(&path, json!({"filter": {"provider_id": []}}))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    let response = gateway
        .post_management(
            &path,
            json!({"filter": {"provider_id": (1..=101).collect::<Vec<i64>>()}}),
        )
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert!(
        test_support::at(&body, "/message")
            .as_str()
            .is_some_and(|message| message.contains("more than 100 values")),
        "{body}"
    );

    Ok(())
}

/// The usage aggregation serves exactly the requested dimensions — the empty grouping answering
/// one total bucket even with no rows, nulls first in the bucket order, sums ignoring unknown
/// measurements and rendering zero for all-null sums — and rejects a repeated or unknown
/// grouping dimension.
#[tokio::test]
async fn usage_query_groups_orders_and_sums() -> Result<(), Box<dyn StdError>> {
    let gateway = TestGateway::open().await;

    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 21,
            provider_id: 11,
            model_id: 31,
            model_route_id: 41,
            outcome: ApiLogOutcome::Completed,
            http_status: Some(200),
            usage: Some(IrUsage {
                input_tokens: 10,
                output_tokens: 20,
                cached_tokens: Some(5),
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        },
    )
    .await;

    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 22,
            provider_id: 12,
            model_id: 32,
            model_route_id: 42,
            outcome: ApiLogOutcome::Completed,
            http_status: Some(200),
            usage: Some(IrUsage {
                input_tokens: 100,
                output_tokens: 200,
                cached_tokens: None,
                cache_write_tokens: None,
                reasoning_tokens: None,
            }),
        },
    )
    .await;

    record_usage_row(
        &gateway,
        RecordedUsageRow {
            api_key_id: 23,
            provider_id: 13,
            model_id: 33,
            model_route_id: 43,
            outcome: ApiLogOutcome::ClientAborted,
            http_status: None,
            usage: None,
        },
    )
    .await;

    let path = format!("{VERSION_PREFIX}/usage:query");

    // The empty grouping aggregates one total bucket; the sums exclude the unknown measurement
    // and render zero for the all-null sums.
    let body = query_usage(&gateway, json!([]), None).await?;

    assert_eq!(
        test_support::at(&body, "/buckets").as_array().map(Vec::len),
        Some(1)
    );

    assert_eq!(*test_support::at(&body, "/buckets/0/key"), json!({}));

    assert_eq!(*test_support::at(&body, "/buckets/0/requests"), 3);

    assert_eq!(*test_support::at(&body, "/buckets/0/input_tokens"), 110);

    assert_eq!(*test_support::at(&body, "/buckets/0/output_tokens"), 220);

    assert_eq!(*test_support::at(&body, "/buckets/0/cached_tokens"), 5);

    assert_eq!(*test_support::at(&body, "/buckets/0/cache_write_tokens"), 0);

    assert_eq!(*test_support::at(&body, "/buckets/0/reasoning_tokens"), 0);

    // The grouping key carries exactly the requested dimensions.
    let body = query_usage(&gateway, json!(["provider"]), None).await?;

    let buckets = test_support::at(&body, "/buckets")
        .as_array()
        .expect("the buckets");

    assert_eq!(
        buckets
            .iter()
            .map(|bucket| test_support::at(bucket, "/key/provider_id").as_i64())
            .collect::<Vec<_>>(),
        vec![Some(11), Some(12), Some(13)],
        "buckets order along the requested dimension: {body}"
    );

    assert!(buckets.iter().all(|bucket| {
        test_support::at(bucket, "/key")
            .as_object()
            .map(|fields| fields.len())
            == Some(1)
    }));

    // The never-answered rows form the null bucket, ordered first.
    let body = query_usage(&gateway, json!(["http_status"]), None).await?;

    let buckets = test_support::at(&body, "/buckets")
        .as_array()
        .expect("the buckets");

    assert_eq!(buckets.len(), 2, "{body}");

    assert_eq!(
        *test_support::at(&body, "/buckets/0/key"),
        json!({"http_status": null}),
        "{body}"
    );

    assert_eq!(*test_support::at(&body, "/buckets/0/requests"), 1);

    assert_eq!(
        *test_support::at(&body, "/buckets/1/key"),
        json!({"http_status": 200})
    );

    assert_eq!(*test_support::at(&body, "/buckets/1/requests"), 2);

    // The day dimension renders the UTC day of the rows' dispatch stamp.
    let body = query_usage(&gateway, json!(["day"]), None).await?;

    let stamp_one = gateway
        .post_management(
            &format!("{VERSION_PREFIX}/usage:list"),
            json!({"filter": {"api_key_id": [21]}}),
        )
        .await;

    let listed = gateway.body_json(stamp_one).await?;

    let stamp_one = test_support::at(&listed, "/api_logs/0/started_at")
        .as_i64()
        .expect("the stamp");

    assert_eq!(
        *test_support::at(&body, "/buckets/0/key"),
        json!({"day": day_key(stamp_one)}),
        "{body}"
    );

    // The filters apply to the aggregation too.
    let body = query_usage(
        &gateway,
        json!(["outcome"]),
        Some(json!({"provider_id": [11]})),
    )
    .await?;

    assert_eq!(
        test_support::at(&body, "/buckets").as_array().map(Vec::len),
        Some(1)
    );

    assert_eq!(
        *test_support::at(&body, "/buckets/0/key"),
        json!({"outcome": "completed"})
    );

    assert_eq!(*test_support::at(&body, "/buckets/0/requests"), 1);

    // A repeated dimension names itself; an unknown one is the parse rejection.
    let response = gateway
        .post_management(&path, json!({"group_by": ["provider", "provider"]}))
        .await;

    let body = assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    assert_eq!(
        *test_support::at(&body, "/message"),
        "`group_by` repeats `provider`",
        "{body}"
    );

    let response = gateway
        .post_management(&path, json!({"group_by": ["nonsense"]}))
        .await;

    assert_error_envelope(&gateway, response, StatusCode::BAD_REQUEST, 1).await?;

    Ok(())
}
