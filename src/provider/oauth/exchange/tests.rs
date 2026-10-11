//! Token-mechanics tests: the exchange execution and classification against the mock upstream,
//! the storage-boundary value validators, and the token-response expiry resolution
//! (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::{ExchangeOutcome, ExchangeProduct, execute_exchange, resolve_expiry};
use crate::{
    provider::{
        Error as ProviderError,
        definition::{credential::Exchange, eval::ValueEnv},
        oauth::tests_support,
        tests_support::failure_detail,
    },
    test_support::mock_upstream::MockUpstream,
    unix_time,
};
use base64::{Engine as _, engine::general_purpose};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error as StdError, num::NonZeroU64};

/// The exchange fixture parsed from one recipe payload.
fn exchange_from(payload: crate::provider::definition::DefinitionPayload) -> Exchange {
    let definition =
        crate::provider::tests_support::definition_named("fixture-definition", payload);

    definition
        .oauth_recipe()
        .expect("the fixture recipe is OAuth")
        .exchange
        .clone()
}

/// The exchange fixture of the canonical code recipe, re-pointed at `url`.
fn exchange_at(url: &str) -> Exchange {
    exchange_from(tests_support::parse(tests_support::code_payload(url)))
}

/// The exchange environment of one fixture completion: the code, verifier-free PKCE, and the
/// redirect URI the recipe repeats.
fn exchange_env<'a>(slots: &'a BTreeMap<String, String>) -> ValueEnv<'a> {
    ValueEnv::for_exchange(
        slots,
        None,
        "client-1",
        Some("secret-1"),
        "code-1",
        None,
        "http://localhost:1455/callback",
    )
}

/// Serves one JSON body with status 200 to every request.
async fn json_mock(body: Value) -> MockUpstream {
    let (mock, _) = MockUpstream::start_json(vec![body.to_string()]).await;

    mock
}

/// Serves one complete raw response to every request.
async fn raw_mock(response: String) -> MockUpstream {
    let (mock, _) = MockUpstream::start_raw(vec![response]).await;

    mock
}

/// A raw JSON response with `status` over `body`.
fn raw_response(status: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// The classification of one executed exchange as assertion text: the outcome tag plus any
/// carried code.
fn classified(product: ExchangeProduct) -> String {
    match product {
        ExchangeProduct::Completed(_) => "completed".to_string(),

        ExchangeProduct::Pending => "pending".to_string(),

        ExchangeProduct::SlowDown => "slow_down".to_string(),

        ExchangeProduct::Terminal(code) => format!("terminal:{code}"),

        ExchangeProduct::Other(code) => format!("other:{code}"),
    }
}

/// The successful token response of the fixture upstream.
fn token_response() -> Value {
    json!({
        "access_token": "at-1",
        "refresh_token": "rt-1",
        "expires_in": 3600,
        "base_url": "https://derived.test",
        "project": "p-1"
    })
}

/// The exchange request renders the declared wire: the POST over the form body carrying the
/// grant type, the pasted code, the redirect URI, and the client identity — exactly the parts
/// the recipe's grant rules demand (`docs/design/providers.md`).
#[tokio::test]
async fn exchange_requests_render_the_declared_wire() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) = MockUpstream::start_json(vec![token_response().to_string()]).await;

    let exchange = exchange_at(&mock.base());

    let slots = BTreeMap::new();

    execute_exchange(&reqwest::Client::new(), &exchange, &exchange_env(&slots)).await?;

    let guard = recorded
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    let request = guard.first().expect("the exchange posts one request");

    assert_eq!(request.method, "POST");

    assert_eq!(request.target, "/token");

    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );

    for field in [
        "grant_type=authorization_code",
        "code=code-1",
        "redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fcallback",
        "client_id=client-1",
        "client_secret=secret-1",
    ] {
        assert!(
            request.body.contains(field),
            "the body carries `{field}`: {}",
            request.body
        );
    }

    Ok(())
}

/// Patches one pointer of a payload JSON value.
trait TapPointer {
    /// Replaces the value at `pointer` with `value`.
    fn tap_pointer(self, pointer: &str, value: Value) -> Self;
}

impl TapPointer for Value {
    fn tap_pointer(mut self, pointer: &str, value: Value) -> Self {
        *self
            .pointer_mut(pointer)
            .expect("the payload pointer resolves") = value;

        self
    }
}

/// The successful exchange maps every declared target: the tokens, the lifetime, the derived
/// base URL, and the named slot — the slot visible through the shared fold.
#[tokio::test]
async fn exchange_maps_the_token_response() -> Result<(), Box<dyn StdError>> {
    let mock = json_mock(token_response()).await;

    let exchange = exchange_at(&format!("{}/token", mock.base()));

    let slots = BTreeMap::new();

    let product =
        execute_exchange(&reqwest::Client::new(), &exchange, &exchange_env(&slots)).await?;

    let ExchangeProduct::Completed(outcome) = product else {
        panic!("a 2xx token response without an error code completes");
    };

    assert_eq!(outcome.access_token.as_deref(), Some("at-1"));

    assert_eq!(outcome.refresh_token.as_deref(), Some("rt-1"));

    assert_eq!(outcome.expires_in_seconds, Some(3600));

    assert_eq!(outcome.base_url.as_deref(), Some("https://derived.test"));

    let absorbed = super::absorb_exchange_slots(&slots, &outcome, &[]).expect("the fold holds");

    assert_eq!(absorbed.get("project").map(String::as_str), Some("p-1"));

    Ok(())
}

/// A form-encoded token response maps through the flat encoding; the exchange request URL and
/// response encoding both ride the fixture recipe's patchable surface.
#[tokio::test]
async fn exchange_maps_form_token_responses() -> Result<(), Box<dyn StdError>> {
    let body = "access_token=at-2&refresh_token=rt-2&expires_in=60";

    let mock = raw_mock(format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
    .await;

    let exchange = exchange_from(tests_support::parse(
        tests_support::code_payload(&mock.base())
            .tap_pointer("/credential/exchange/response/encoding", json!("form")),
    ));

    let slots = BTreeMap::new();

    let product =
        execute_exchange(&reqwest::Client::new(), &exchange, &exchange_env(&slots)).await?;

    let ExchangeProduct::Completed(outcome) = product else {
        panic!("a 2xx form token response completes");
    };

    assert_eq!(outcome.access_token.as_deref(), Some("at-2"));

    assert_eq!(outcome.refresh_token.as_deref(), Some("rt-2"));

    assert_eq!(outcome.expires_in_seconds, Some(60));

    Ok(())
}

/// The RFC 8628 polling codes classify from a 2xx body carrying the error name — the GitHub
/// device shape — matched case-insensitively.
#[tokio::test]
async fn polling_codes_classify_from_success_bodies() -> Result<(), Box<dyn StdError>> {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    for (code, expected) in [
        ("authorization_pending", "pending"),
        ("slow_down", "slow_down"),
        ("Slow_Down", "slow_down"),
        ("AUTHORIZATION_PENDING", "pending"),
    ] {
        let mock = json_mock(json!({"error": code})).await;

        let product = execute_exchange(
            &reqwest::Client::new(),
            &exchange_at(&format!("{}/token", mock.base())),
            &env,
        )
        .await?;

        assert_eq!(classified(product), expected, "for upstream code `{code}`");
    }

    Ok(())
}

/// A failure code in the configured terminal list ends the flow — matched case-insensitively,
/// carrying the code's original spelling — and a code outside the list classifies as `Other`.
#[tokio::test]
async fn terminal_codes_end_the_flow_case_insensitively() -> Result<(), Box<dyn StdError>> {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    let mock = json_mock(json!({"error": "INVALID_GRANT"})).await;

    let product = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at(&format!("{}/token", mock.base())),
        &env,
    )
    .await?;

    assert_eq!(classified(product), "terminal:INVALID_GRANT");

    let mock = json_mock(json!({"error": "server_busy"})).await;

    let product = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at(&format!("{}/token", mock.base())),
        &env,
    )
    .await?;

    assert_eq!(classified(product), "other:server_busy");

    Ok(())
}

/// A non-2xx rejection carrying a terminal code classifies as terminal; every other non-2xx
/// answer is the generic rejected failure carrying the upstream status.
#[tokio::test]
async fn rejections_classify_by_their_error_body() -> Result<(), Box<dyn StdError>> {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    let terminal = raw_mock(raw_response(400, r#"{"error": "expired_token"}"#)).await;

    let product = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at(&format!("{}/token", terminal.base())),
        &env,
    )
    .await?;

    assert_eq!(classified(product), "terminal:expired_token");

    let codeless = raw_mock(raw_response(503, "out of capacity")).await;

    let failure = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at(&format!("{}/token", codeless.base())),
        &env,
    )
    .await;

    let Err(ProviderError::Rejected { status, detail }) = failure else {
        panic!("a codeless rejection carries the Rejected variant");
    };

    assert_eq!(status, 503);

    assert!(
        detail.contains("out of capacity"),
        "the excerpt rides along"
    );

    Ok(())
}

/// The form `error=` field (RFC 6749 §5.2) carries the machine code a rejection classifies by.
#[tokio::test]
async fn form_rejections_carry_the_error_code() -> Result<(), Box<dyn StdError>> {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    let body = "error=expired_token&error_description=old";

    let mock = raw_mock(format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: application/x-www-form-urlencoded\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    ))
    .await;

    let product = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at(&format!("{}/token", mock.base())),
        &env,
    )
    .await?;

    assert_eq!(classified(product), "terminal:expired_token");

    Ok(())
}

/// An unreachable token endpoint is the unavailable failure.
#[tokio::test]
async fn transport_failures_are_unavailable() {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    let failure = execute_exchange(
        &reqwest::Client::new(),
        &exchange_at("http://127.0.0.1:1/token"),
        &env,
    )
    .await;

    assert!(
        matches!(failure, Err(ProviderError::Unavailable { .. })),
        "connection refused is Unavailable"
    );
}

/// A missing required mapped field fails the exchange naming the path; a non-integer
/// `expires_in` is a malformed token response, never a silent fall-through.
#[tokio::test]
async fn unusable_token_responses_fail_the_fold() -> Result<(), Box<dyn StdError>> {
    let slots = BTreeMap::new();

    let env = exchange_env(&slots);

    let mock = json_mock(json!({"refresh_token": "rt-1"})).await;

    let exchange = exchange_at(&format!("{}/token", mock.base()));

    let detail = failure_detail(execute_exchange(&reqwest::Client::new(), &exchange, &env).await);

    assert!(detail.contains("required path"), "got: {detail}");

    let mock =
        json_mock(json!({"access_token": "at-1", "refresh_token": "rt-1", "expires_in": "soon"}))
            .await;

    let exchange = exchange_at(&format!("{}/token", mock.base()));

    let detail = failure_detail(execute_exchange(&reqwest::Client::new(), &exchange, &env).await);

    assert!(detail.contains("non-integer `expires_in`"), "got: {detail}");

    Ok(())
}

/// Hostile token values fail the storage boundary before anything persists: tokens carry
/// header-value bytes, slots routing-safe text, and base URLs the http(s) scheme contract.
#[test]
fn storage_boundaries_validate_machine_consumed_values() {
    assert!(super::validate_token_value("access_token", "tok-1").is_ok());

    assert!(
        failure_detail(super::validate_token_value("access_token", "tok\n"))
            .contains("header-value bytes"),
        "a control byte is not a header value"
    );

    assert!(super::validate_slot_value("project", "p-1").is_ok());

    assert!(
        failure_detail(super::validate_slot_value("project", "has space"))
            .contains("printable ASCII"),
        "whitespace is not routing-safe"
    );

    assert!(super::validate_base_url_value("https://derived.test").is_ok());

    for hostile in [
        "ftp://derived.test",
        "http://",
        "derived.test",
        "https://a b",
    ] {
        assert!(
            failure_detail(super::validate_base_url_value(hostile)).contains("base_url"),
            "`{hostile}` fails the base-URL contract"
        );
    }
}

/// The expiry resolution precedence: a positive reported `expires_in` wins; a non-positive one
/// is malformed; the future `exp` claim follows (fractional claims ceil); then the configured
/// lifetime; nothing means no scheduled refresh.
#[test]
fn expiry_resolution_follows_the_documented_precedence() {
    let now = unix_time::unix_now_micros();

    let reported =
        resolve_expiry(Some(30), Some("at"), None).expect("a positive lifetime resolves");

    assert!(reported.expect("the lifetime schedules") - now > 29 * unix_time::MICROS_PER_SEC);

    let non_positive = resolve_expiry(Some(0), None, None);

    assert!(
        failure_detail(non_positive).contains("must be positive"),
        "a non-positive lifetime is malformed"
    );

    let future = unix_time::unix_now() + 3600;

    let claimed = resolve_expiry(None, Some(&fixture_jwt(&json!({"exp": future}))), None)
        .expect("the exp claim resolves");

    assert_eq!(
        claimed.expect("the claim schedules"),
        future * unix_time::MICROS_PER_SEC
    );

    let past = unix_time::unix_now() - 3600;

    let failure = resolve_expiry(None, Some(&fixture_jwt(&json!({"exp": past}))), None);

    assert!(
        failure_detail(failure).contains("must be in the future"),
        "a past claim is malformed"
    );

    let fractional_base = unix_time::unix_now() + 3600;

    let fractional = resolve_expiry(
        None,
        Some(&fixture_jwt(&json!({"exp": fractional_base as f64 + 0.5}))),
        None,
    );

    assert_eq!(
        fractional
            .expect("the fractional claim ceils")
            .expect("schedules"),
        (fractional_base + 1) * unix_time::MICROS_PER_SEC
    );

    let configured =
        resolve_expiry(None, Some("not-a-jwt"), NonZeroU64::new(120)).expect("resolves");

    assert!(configured.expect("the lifetime schedules") - now > 119 * unix_time::MICROS_PER_SEC);

    assert_eq!(
        resolve_expiry(None, Some("not-a-jwt"), None).expect("resolves"),
        None
    );

    let out_of_range = resolve_expiry(None, None, NonZeroU64::new(u64::MAX));

    assert!(
        failure_detail(out_of_range).contains("out of range"),
        "a lifetime beyond the clock is drift"
    );
}

/// Builds one unsigned JWT whose payload carries `claims` (RFC 7519 §2 spelling).
fn fixture_jwt(claims: &Value) -> String {
    let segment = |part: &Value| {
        general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(part).expect("claims render"))
    };

    format!(
        "{}.{}.sig",
        segment(&json!({"alg": "none"})),
        segment(claims)
    )
}

/// The exchange outcome serializes into the device poller's checkpoint shape and back — the
/// persistence contract a restart recovery reads.
#[test]
fn exchange_outcomes_round_trip_through_their_checkpoint() {
    let outcome = ExchangeOutcome {
        access_token: Some("at-1".to_string()),
        refresh_token: Some("rt-1".to_string()),
        expires_in_seconds: Some(60),
        base_url: None,
        ..ExchangeOutcome::default()
    };

    let checkpoint = serde_json::to_value(&outcome).expect("the checkpoint serializes");

    let recovered: ExchangeOutcome =
        serde_json::from_value(checkpoint).expect("the checkpoint deserializes");

    assert_eq!(recovered.access_token.as_deref(), Some("at-1"));

    assert_eq!(recovered.refresh_token.as_deref(), Some("rt-1"));

    assert_eq!(recovered.expires_in_seconds, Some(60));
}
