//! Provider-subsystem tests outside one engine: the shared control-request executor's parsing,
//! encoding, and extraction contracts, the quota-rejection classifier's positive discrimination,
//! and the stored model/credential accessors (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    clippy::panic,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse, build, or serve is that test's failure report"
)]

use super::{
    CredentialKind, DiscoveryState, Model, StoredCredentials,
    control::{
        self, ControlFailure, DecodedBody, append_query_row, is_truthy_scalar, parse_form,
        percent_encode_component,
    },
    definition::{
        compaction::CompactionSection,
        eval::ValueEnv,
        template::{
            ControlBody, ControlMethod, ControlRequest, ExtractionShape, ParamRow,
            ResponseEncoding, ResponseField, ResponsePath, ResponseTarget, StringExpr,
            TemplateNode, ValueExpr,
        },
    },
    quota::QuotaApplicability,
    quota_rejection,
};
use crate::{
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    http_body,
    provider::definition::quota_exhaustion::QuotaExhaustion,
    test_support::mock_upstream::MockUpstream,
    vocabulary::ReasoningLevel,
};
use serde_json::{Value as Json, json};
use std::{collections::BTreeMap, error::Error as StdError};

/// Parses one compaction section fixture over the `mechanisms` JSON object.
fn compaction_section(mechanisms: Json) -> CompactionSection {
    #[expect(
        clippy::expect_used,
        reason = "a fixture that cannot parse is that test's failure report"
    )]
    serde_json::from_value(json!({"mechanisms": mechanisms})).expect("the section fixture parses")
}

/// A GET control request over the literal `url` with no headers, query, or body.
fn get_request(url: &str) -> ControlRequest {
    ControlRequest {
        method: ControlMethod::Get,
        url: StringExpr::literal(url),
        headers: Vec::new(),
        query: Vec::new(),
        body: None,
    }
}

/// The stored-credential environment over `slots` carrying `secret` and no base URL.
fn stored_env<'a>(slots: &'a BTreeMap<String, String>, secret: &'a str) -> ValueEnv<'a> {
    ValueEnv::for_stored_credential(slots, None, secret)
}

/// The bounded error body of one raw mock rejection with `status` and `reason` over
/// `payload`, fetched the way a production classification receives it.
async fn rejection_body(status: u16, reason: &str, payload: &str) -> http_body::ErrorBody {
    let (mock, _) = MockUpstream::start_raw(vec![raw_json_response(status, reason, payload)]).await;

    error_body_of(&reqwest::Client::new(), &mock.base()).await
}

/// The `application/x-www-form-urlencoded` body of `fields`.
fn form_body(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// A raw HTTP response with `status` and `reason` over a JSON `body`.
fn raw_json_response(status: u16, reason: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
}

/// A raw 429 response whose `Content-Length` declares far more than the body carries: the
/// truncated-body shape that must never classify.
fn truncated_response(body: &str) -> String {
    format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: \
         4096\r\nConnection: close\r\n\r\n{body}"
    )
}

/// The two-rule quota-exhaustion fixture: the first rule classifies the account-wide
/// `quota_exceeded` code, the second the requested-model `tier_exhausted` code.
fn two_rule_descriptor() -> QuotaExhaustion {
    serde_json::from_value(json!({
        "rules": [
            {
                "name": "monthly-plan",
                "statuses": [429, 403],
                "code": {"at": ["error"], "value": "quota_exceeded"},
                "restriction_key": [{"type": "literal", "value": "fixture-plan"}],
                "applies_to": {"type": "all_models"}
            },
            {
                "name": "model-tier",
                "statuses": [429],
                "code": {"at": ["error", "type"], "value": "tier_exhausted"},
                "restriction_key": [
                    {"type": "literal", "value": "tier"},
                    {"type": "requested_model"}
                ],
                "applies_to": {"type": "requested_model"}
            }
        ]
    }))
    .expect("the exhaustion fixture parses")
}

/// Fetches `target` and reads its bounded error body, the only path a production
/// classification ever sees one through.
async fn error_body_of(client: &reqwest::Client, target: &str) -> http_body::ErrorBody {
    let response = client.get(target).send().await.expect("the mock answers");

    http_body::read_bounded_error_body(response).await
}

/// The identity tuple and applicability summary of a classified rejection, unwrapped for
/// assertions.
fn classified(
    rejection: Option<quota_rejection::ClassifiedQuotaRejection>,
) -> (Vec<String>, String) {
    let rejection = rejection.expect("the rejection classifies");

    let applicability = match rejection.applies_to {
        QuotaApplicability::AllModels => "all_models".to_string(),
        QuotaApplicability::Models { upstream_ids } => format!("models:{upstream_ids:?}"),
    };

    (rejection.key, applicability)
}

/// `parse_form` decodes `+` as a space, `%XX` escapes in both alphabets, keeps an invalid
/// escape's `%` verbatim, folds a repeated field to its last value, reads a pair without `=`
/// as an empty value, and skips empty segments.
#[test]
fn parse_form_decodes_urlencoded_components() {
    let fields = parse_form("a=one+two&b=%2B1%2c&bad=%zz&c&d=first&d=last&e=no%2&f=");

    let expected = BTreeMap::from([
        ("a".to_string(), "one two".to_string()),
        ("b".to_string(), "+1,".to_string()),
        ("bad".to_string(), "%zz".to_string()),
        ("c".to_string(), String::new()),
        ("d".to_string(), "last".to_string()),
        ("e".to_string(), "no%2".to_string()),
        ("f".to_string(), String::new()),
    ]);

    assert_eq!(fields, expected);
}

/// `percent_encode_component` keeps the RFC 3986 unreserved set verbatim and escapes every
/// other byte — a space as `%20`, a literal plus as `%2B`, non-ASCII bytes as their UTF-8
/// octets — never the form-urlencoded `+`-for-space rule.
#[test]
fn percent_encoding_uses_the_rfc3986_unreserved_set() {
    assert_eq!(percent_encode_component("A-z09-._~"), "A-z09-._~");

    assert_eq!(percent_encode_component("a b+c/d"), "a%20b%2Bc%2Fd");

    assert_eq!(percent_encode_component("ä"), "%C3%A4");

    assert_eq!(percent_encode_component("#fragment"), "%23fragment");
}

/// Query rows join with `?` first and `&` after, and a URL fragment stays behind every row —
/// the join never folds a pagination or state parameter into the fragment.
#[test]
fn query_rows_join_ahead_of_a_url_fragment() {
    let mut plain = "https://up.test/token".to_string();

    append_query_row(&mut plain, "first", "1");

    append_query_row(&mut plain, "second", "2");

    assert_eq!(plain, "https://up.test/token?first=1&second=2");

    let mut fragment = "https://up.test/token#end".to_string();

    append_query_row(&mut fragment, "state", "abc");

    assert_eq!(fragment, "https://up.test/token?state=abc#end");
}

/// The truthy-scalar contract of poll done paths and has-more chains: boolean `true`, the
/// number `1`, and the string `"true"` read as true; every other shape — including `0`,
/// `"1"`, `"yes"`, null, and arrays — reads as false.
#[test]
fn truthy_scalars_are_the_shared_boolean_contract() {
    assert!(is_truthy_scalar(&json!(true)));

    assert!(is_truthy_scalar(&json!(1)));

    assert!(is_truthy_scalar(&json!("true")));

    assert!(!is_truthy_scalar(&json!(false)));

    assert!(!is_truthy_scalar(&json!(0)));

    assert!(!is_truthy_scalar(&json!(2)));

    assert!(!is_truthy_scalar(&json!("1")));

    assert!(!is_truthy_scalar(&json!("yes")));

    assert!(!is_truthy_scalar(&json!(null)));

    assert!(!is_truthy_scalar(&json!([])));
}

/// The text extraction shape reads JSON strings and renders JSON numbers as their decimal
/// text; the integer shape demands an exact integer; the string-or-field shape accepts a bare
/// string or an object carrying the named string member; a missing path is `None`; a present
/// value of the wrong shape always fails.
#[test]
fn json_extraction_shapes_follow_the_declared_shape() {
    let decoded = DecodedBody::Json(json!({
        "text": "tok",
        "number": 1800,
        "integer": 7,
        "fractional": 7.5,
        "plain": "bare",
        "wrapped": {"value": "in-object"},
        "wrong": {"value": 12},
        "nested": {"deep": "found"}
    }));

    let read = |members: &[&str], shape: &ExtractionShape| {
        control::extract_field(
            &decoded,
            &ResponsePath(members.iter().map(|member| (*member).to_string()).collect()),
            shape,
            "test",
        )
    };

    let text = |members: &[&str]| read(members, &ExtractionShape::String);

    let extracted = text(&["text"]).expect("the text shape reads a string");

    assert_eq!(
        extracted.map(|value| value.as_text()),
        Some("tok".to_string())
    );

    let numeric = text(&["number"]).expect("the text shape renders a number as decimal text");

    assert_eq!(
        numeric.map(|value| value.as_text()),
        Some("1800".to_string())
    );

    let integer =
        read(&["integer"], &ExtractionShape::Integer).expect("the integer shape reads an integer");

    assert_eq!(integer.and_then(|value| value.as_integer()), Some(7));

    let fractional = read(&["fractional"], &ExtractionShape::Integer);

    assert!(fractional.is_err(), "a fractional number is not an integer");

    let string_or_field = ExtractionShape::StringOrField {
        field: "value".to_string(),
    };

    let bare = read(&["plain"], &string_or_field).expect("a bare string stands as its own value");

    assert_eq!(bare.map(|value| value.as_text()), Some("bare".to_string()));

    let wrapped = read(&["wrapped"], &string_or_field).expect("the object carries the member");

    assert_eq!(
        wrapped.map(|value| value.as_text()),
        Some("in-object".to_string())
    );

    let wrong_member = read(&["wrong"], &string_or_field);

    assert!(
        wrong_member.is_err(),
        "a non-string member is the wrong shape"
    );

    let deep = text(&["nested", "deep"]).expect("a member chain resolves");

    assert_eq!(deep.map(|value| value.as_text()), Some("found".to_string()));

    assert!(
        text(&["absent"])
            .expect("an absent path is not an error")
            .is_none()
    );

    assert!(
        text(&["fractional", "member"])
            .expect("a non-object intermediate is a missing path")
            .is_none()
    );
}

/// The flat form encoding reads its single-member paths verbatim, parses integer shapes from
/// their decimal text, and rejects a multi-member path as a malformed definition.
#[test]
fn form_extraction_reads_flat_fields() {
    let decoded = DecodedBody::Form(BTreeMap::from([
        ("access_token".to_string(), "abc".to_string()),
        ("expires_in".to_string(), "3600".to_string()),
        ("not_a_number".to_string(), "soon".to_string()),
    ]));

    let single = control::extract_field(
        &decoded,
        &ResponsePath(vec!["access_token".to_string()]),
        &ExtractionShape::String,
        "test",
    )
    .expect("a flat form field reads verbatim");

    assert_eq!(single.map(|value| value.as_text()), Some("abc".to_string()));

    let integer = control::extract_field(
        &decoded,
        &ResponsePath(vec!["expires_in".to_string()]),
        &ExtractionShape::Integer,
        "test",
    )
    .expect("the decimal text parses as an integer");

    assert_eq!(integer.and_then(|value| value.as_integer()), Some(3600));

    let malformed = control::extract_field(
        &decoded,
        &ResponsePath(vec!["not_a_number".to_string()]),
        &ExtractionShape::Integer,
        "test",
    );

    assert!(malformed.is_err(), "non-numeric text is the wrong shape");

    let absent = control::extract_field(
        &decoded,
        &ResponsePath(vec!["missing".to_string()]),
        &ExtractionShape::String,
        "test",
    );

    assert!(absent.expect("an absent member is not an error").is_none());

    let multi = control::extract_field(
        &decoded,
        &ResponsePath(vec!["a".to_string(), "b".to_string()]),
        &ExtractionShape::String,
        "test",
    );

    assert!(multi.is_err(), "form paths address flat fields only");
}

/// The response fold extracts every declared field: a missing required path fails the stage
/// naming the path, a missing optional path leaves its target unchanged, and an apply failure
/// propagates unchanged.
#[test]
fn response_folding_applies_required_optional_and_apply_failures() {
    let decoded = DecodedBody::Json(json!({"present": "value"}));

    let field = |path: &str, required: bool| ResponseField {
        path: ResponsePath(vec![path.to_string()]),
        target: ResponseTarget::Slot {
            name: "sink".to_string(),
        },
        required,
    };

    let mut applied: Vec<String> = Vec::new();

    let fields = [field("present", true), field("absent", false)];

    control::map_response_fields(&decoded, &fields, "test", |_, extracted| {
        applied.push(extracted.as_text());

        Ok::<(), ControlFailure>(())
    })
    .expect("an optional absence never fails the fold");

    assert_eq!(applied, vec!["value".to_string()]);

    let required_missing = [field("absent", true)];

    let failure = control::map_response_fields(&decoded, &required_missing, "test", |_, _| {
        Ok::<(), ControlFailure>(())
    });

    let detail = failure
        .expect_err("a missing required path fails")
        .to_string();

    assert!(
        detail.contains("absent"),
        "the failure names the path: {detail}"
    );

    let propagated =
        control::map_response_fields(&decoded, &[field("present", true)], "test", |_, _| {
            Err(ControlFailure::Unavailable {
                detail: "apply failed".to_string(),
            })
        });

    match propagated {
        Err(ControlFailure::Unavailable { detail }) => assert_eq!(detail, "apply failed"),

        other => panic!("an apply failure propagates unchanged, got {other:?}"),
    }
}

/// A request expression referencing a value its stage does not carry fails the request build
/// as unavailable — the stored-drift shape the write boundary rejects.
#[test]
fn request_building_fails_on_a_stage_missing_value() {
    let request = ControlRequest {
        method: ControlMethod::Get,
        url: StringExpr(vec![ValueExpr::Model]),
        headers: Vec::new(),
        query: Vec::new(),
        body: None,
    };

    let client = reqwest::Client::new();

    let failure = control::build_request(
        &client,
        &request,
        &stored_env(&BTreeMap::new(), "key"),
        "test",
    );

    match failure {
        Err(ControlFailure::Unavailable { detail }) => {
            assert!(
                detail.contains("model"),
                "the failure names the kind: {detail}"
            );
        }

        other => panic!("a stage-missing value fails the build, got {other:?}"),
    }
}

/// The executor decodes a 2xx JSON response through the declared JSON encoding against the
/// mock upstream, and a form-encoded body through the flat form encoding.
#[tokio::test]
async fn execute_decodes_both_declared_encodings() -> Result<(), Box<dyn StdError>> {
    let (json_mock, _) = MockUpstream::start_json(vec![json!({"ok": true}).to_string()]).await;

    let client = reqwest::Client::new();

    let decoded = control::execute(
        &client,
        &get_request(&json_mock.base()),
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Json,
        "test",
    )
    .await?;

    match decoded {
        DecodedBody::Json(json) => assert_eq!(json, json!({"ok": true})),

        DecodedBody::Form(_) => panic!("the JSON encoding decoded as form fields"),
    }

    let body = form_body(&[("error", "slow_down")]);

    let (form_mock, _) = MockUpstream::start_raw(vec![format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-www-form-urlencoded\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )])
    .await;

    let decoded = control::execute(
        &client,
        &get_request(&form_mock.base()),
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Form,
        "test",
    )
    .await?;

    match decoded {
        DecodedBody::Form(fields) => {
            assert_eq!(fields.get("error").map(String::as_str), Some("slow_down"));
        }

        DecodedBody::Json(_) => panic!("the form encoding decoded as JSON"),
    }

    Ok(())
}

/// A non-2xx answer is the `Rejected` failure carrying the upstream status and the full body,
/// the shape the token-grant classification inspects error codes through.
#[tokio::test]
async fn execute_carries_rejected_statuses_with_their_body() -> Result<(), Box<dyn StdError>> {
    let body = r#"{"error": "server_error"}"#;

    let (mock, _) =
        MockUpstream::start_raw(vec![raw_json_response(503, "Service Unavailable", body)]).await;

    let client = reqwest::Client::new();

    let failure = control::execute(
        &client,
        &get_request(&mock.base()),
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Json,
        "test",
    )
    .await;

    match failure {
        Err(ControlFailure::Rejected {
            status,
            body: carried,
        }) => {
            assert_eq!(status, 503);

            assert_eq!(carried, body);
        }

        other => panic!("a non-2xx answer classifies as Rejected, got {other:?}"),
    }

    Ok(())
}

/// An unreachable endpoint is the `Unavailable` failure: the transport-failure shape every
/// upstream-facing stage maps onto its own vocabulary.
#[tokio::test]
async fn execute_maps_a_transport_failure_to_unavailable() {
    let client = reqwest::Client::new();

    let failure = control::execute(
        &client,
        &get_request("http://127.0.0.1:1/unreachable"),
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Json,
        "test",
    )
    .await;

    assert!(
        matches!(failure, Err(ControlFailure::Unavailable { .. })),
        "connection refused is Unavailable, got {failure:?}"
    );
}

/// The executor supplies the `Content-Type` from the body kind and renders both body shapes:
/// the form encoding percent-encodes its components, the JSON tree renders with literal leaves
/// and evaluated string leaves, an explicit header row rides beside the supplied one, and the
/// query rows join the target ahead of any fragment.
#[tokio::test]
async fn execute_renders_the_post_wire_from_the_body_kind() -> Result<(), Box<dyn StdError>> {
    let (mock, recorded) =
        MockUpstream::start_json(vec![json!({}).to_string(), json!({}).to_string()]).await;

    let client = reqwest::Client::new();

    let base = get_request(&mock.base());

    let form_request = ControlRequest {
        method: ControlMethod::Post,
        url: base.url.clone(),
        headers: vec![ParamRow {
            name: "X-Trace".to_string(),
            value: StringExpr::literal("t-1"),
        }],
        query: vec![ParamRow {
            name: "page".to_string(),
            value: StringExpr::literal("1 2"),
        }],
        body: Some(ControlBody::Form {
            fields: vec![
                ParamRow {
                    name: "grant".to_string(),
                    value: StringExpr::literal("authorization_code"),
                },
                ParamRow {
                    name: "scope".to_string(),
                    value: StringExpr::literal("read write"),
                },
            ],
        }),
    };

    control::execute(
        &client,
        &form_request,
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Json,
        "test",
    )
    .await?;

    let json_request = ControlRequest {
        method: ControlMethod::Post,
        url: base.url.clone(),
        headers: Vec::new(),
        query: Vec::new(),
        body: Some(ControlBody::Json {
            node: TemplateNode::Object {
                fields: BTreeMap::from([
                    (
                        "model".to_string(),
                        TemplateNode::String {
                            expr: StringExpr::literal("m-1"),
                        },
                    ),
                    ("static".to_string(), TemplateNode::Json { value: json!(7) }),
                ]),
            },
        }),
    };

    control::execute(
        &client,
        &json_request,
        &stored_env(&BTreeMap::new(), "key"),
        ResponseEncoding::Json,
        "test",
    )
    .await?;

    let requests = recorded
        .lock()
        .map_err(|error| format!("the recorded requests lock: {error}"))?;

    let form = requests.first().expect("the form request");

    assert_eq!(form.method, "POST");

    assert_eq!(
        form.header("content-type"),
        Some("application/x-www-form-urlencoded")
    );

    assert_eq!(form.header("x-trace"), Some("t-1"));

    assert_eq!(form.target, "/?page=1%202");

    assert!(form.body.contains("grant=authorization_code"));

    assert!(
        form.body.contains("scope=read+write"),
        "the form encoding folds a space into `+`: {}",
        form.body
    );

    let tree = requests.get(1).expect("the JSON request");

    assert_eq!(tree.header("content-type"), Some("application/json"));

    assert_eq!(
        crate::test_support::at(&serde_json::from_str::<Json>(&tree.body)?, "/model"),
        &json!("m-1")
    );

    assert_eq!(
        crate::test_support::at(&serde_json::from_str::<Json>(&tree.body)?, "/static"),
        &json!(7)
    );

    Ok(())
}

/// The rejection classifier matches the first rule whose status set and exact case-sensitive
/// code both hold, resolving literal and requested-model identity segments and both
/// applicability forms.
#[tokio::test]
async fn rejection_classification_matches_rules_in_order() -> Result<(), Box<dyn StdError>> {
    let (mock, _) = MockUpstream::start_raw(vec![raw_json_response(
        429,
        "Too Many Requests",
        r#"{"error": "quota_exceeded"}"#,
    )])
    .await;

    let client = reqwest::Client::new();

    let body = error_body_of(&client, &mock.base()).await;

    let (key, applicability) = classified(quota_rejection::classify_rejection(
        429,
        &body,
        Some(&two_rule_descriptor()),
        "upstream-model",
    ));

    assert_eq!(key, vec!["fixture-plan".to_string()]);

    assert_eq!(applicability, "all_models");

    let (mock, _) = MockUpstream::start_raw(vec![raw_json_response(
        429,
        "Too Many Requests",
        r#"{"error": {"type": "tier_exhausted"}}"#,
    )])
    .await;

    let body = error_body_of(&client, &mock.base()).await;

    let (key, applicability) = classified(quota_rejection::classify_rejection(
        429,
        &body,
        Some(&two_rule_descriptor()),
        "upstream-model",
    ));

    assert_eq!(key, vec!["tier".to_string(), "upstream-model".to_string()]);

    assert_eq!(applicability, "models:[\"upstream-model\"]");

    Ok(())
}

/// Classification needs all three facts together: a rule-listed status with a different code,
/// a matching code at an unlisted status, a foreign code spelling, and no descriptor at all
/// never classify.
#[tokio::test]
async fn rejection_classification_is_exact_in_status_code_and_presence()
-> Result<(), Box<dyn StdError>> {
    let descriptor = two_rule_descriptor();

    let wrong_code = rejection_body(429, "Too Many Requests", r#"{"error": "other"}"#).await;

    assert!(
        quota_rejection::classify_rejection(429, &wrong_code, Some(&descriptor), "m").is_none(),
        "a foreign code never classifies"
    );

    let wrong_status = rejection_body(
        500,
        "Internal Server Error",
        r#"{"error": "quota_exceeded"}"#,
    )
    .await;

    assert!(
        quota_rejection::classify_rejection(500, &wrong_status, Some(&descriptor), "m").is_none(),
        "a code at an unlisted status never classifies"
    );

    let code_case =
        rejection_body(429, "Too Many Requests", r#"{"error": "Quota_Exceeded"}"#).await;

    assert!(
        quota_rejection::classify_rejection(429, &code_case, Some(&descriptor), "m").is_none(),
        "the code comparison is case-sensitive"
    );

    let wrong_path =
        rejection_body(429, "Too Many Requests", r#"{"detail": "quota_exceeded"}"#).await;

    assert!(
        quota_rejection::classify_rejection(429, &wrong_path, Some(&descriptor), "m").is_none(),
        "the code must sit at the configured member path"
    );

    assert!(
        quota_rejection::classify_rejection(429, &wrong_code, None, "m").is_none(),
        "a definition without a classifier never classifies"
    );

    Ok(())
}

/// Only a complete valid JSON body can classify: a truncated body and a body that is not JSON
/// at all are never exhaustion evidence.
#[tokio::test]
async fn rejection_classification_requires_complete_json() -> Result<(), Box<dyn StdError>> {
    let (mock, _) =
        MockUpstream::start_raw(vec![truncated_response(r#"{"error": "quota_exce"#)]).await;

    let body = error_body_of(&reqwest::Client::new(), &mock.base()).await;

    assert!(
        quota_rejection::classify_rejection(429, &body, Some(&two_rule_descriptor()), "m")
            .is_none(),
        "a truncated body never classifies"
    );

    let (mock, _) = MockUpstream::start_raw(vec![raw_json_response(
        429,
        "Too Many Requests",
        "plain prose, no JSON",
    )])
    .await;

    let body = error_body_of(&reqwest::Client::new(), &mock.base()).await;

    assert!(
        quota_rejection::classify_rejection(429, &body, Some(&two_rule_descriptor()), "m")
            .is_none(),
        "body prose never classifies"
    );

    Ok(())
}

/// The stored credentials render their secret, kind, and slot map per variant; an API key
/// carries an empty slot map.
#[test]
fn stored_credentials_expose_secret_kind_and_slots() {
    let api_key = StoredCredentials::ApiKey {
        api_key: "key-1".to_string(),
    };

    assert_eq!(api_key.secret(), "key-1");

    assert_eq!(api_key.kind(), CredentialKind::ApiKey);

    assert_eq!(api_key.kind().as_str(), "api_key");

    assert!(api_key.slots().is_empty());

    let oauth = StoredCredentials::OAuth {
        access_token: "tok".to_string(),
        refresh_token: Some("r".to_string()),
        slots: BTreeMap::from([("team".to_string(), "t".to_string())]),
        expires_at: None,
        refresh_started_at: None,
    };

    assert_eq!(oauth.secret(), "tok");

    assert_eq!(oauth.kind(), CredentialKind::OAuth);

    assert_eq!(oauth.kind().as_str(), "oauth");

    assert_eq!(oauth.slots().get("team").map(String::as_str), Some("t"));
}

/// The discovery-state vocabulary parses its three canonical names and nothing else, and
/// renders them back verbatim.
#[test]
fn discovery_states_parse_the_canonical_vocabulary() {
    for (spelling, state) in [
        ("discovered", DiscoveryState::Discovered),
        ("vanished", DiscoveryState::Vanished),
        ("manual", DiscoveryState::Manual),
    ] {
        assert_eq!(DiscoveryState::parse(spelling), Some(state));

        assert_eq!(state.as_str(), spelling);
    }

    assert_eq!(DiscoveryState::parse("unknown"), None);

    assert_eq!(DiscoveryState::parse(""), None);

    assert_eq!(DiscoveryState::parse("Discovered"), None);
}

/// The model row's effective declarations prefer the administrator override over the
/// discovered fact, and a vanished row never serves regardless of either switch.
#[test]
fn model_effective_declarations_prefer_the_override() {
    let model = Model {
        id: 1,
        upstream_id: "m".to_string(),
        is_enabled: true,
        discovery_state: DiscoveryState::Discovered,
        thinking_levels: Some(vec![ReasoningLevel::Low, ReasoningLevel::Medium]),
        max_input_tokens: Some(1000),
        max_output_tokens: Some(500),
        thinking_levels_override: Some(vec![ReasoningLevel::High]),
        max_input_tokens_override: Some(2000),
        max_output_tokens_override: None,
        compaction_support: None,
        compaction_support_override: None,
    };

    assert_eq!(
        model.effective_thinking_levels(),
        Some(&[ReasoningLevel::High][..])
    );

    assert_eq!(model.effective_max_input_tokens(), Some(2000));

    assert_eq!(model.effective_max_output_tokens(), Some(500));

    let undiscovered = Model {
        thinking_levels: None,
        thinking_levels_override: None,
        max_input_tokens: None,
        max_input_tokens_override: None,
        max_output_tokens: None,
        max_output_tokens_override: None,
        discovery_state: DiscoveryState::Vanished,
        ..model.clone()
    };

    assert_eq!(undiscovered.effective_thinking_levels(), None);

    assert_eq!(undiscovered.effective_max_input_tokens(), None);

    assert!(!undiscovered.is_effectively_enabled(true));

    assert!(!model.is_effectively_enabled(false));

    let disabled_model = Model {
        is_enabled: false,
        ..model.clone()
    };

    assert!(!disabled_model.is_effectively_enabled(true));

    assert!(model.is_effectively_enabled(true));
}

/// The effective mechanism support resolves the administrator override over the discovery fact
/// over the section's declared default, omits every unknown mechanism, and an explicit
/// `unknown` override suppresses a lower supported fact.
#[test]
fn model_effective_mechanism_support_resolves_the_layers() {
    let facts = CompactionSupportFacts::from_pairs([(
        Mechanism::MessagesOnDemandCompaction,
        MechanismSupport::Supported,
    )])
    .expect("the pair set is nonempty");

    let suppressed = CompactionSupportFacts::from_pairs([
        (
            Mechanism::MessagesOnDemandCompaction,
            MechanismSupport::Unknown,
        ),
        (
            Mechanism::MessagesThresholdCompaction,
            MechanismSupport::Unsupported,
        ),
    ])
    .expect("the pair set is nonempty");

    let model = Model {
        id: 1,
        upstream_id: "m".to_string(),
        is_enabled: true,
        discovery_state: DiscoveryState::Discovered,
        thinking_levels: None,
        max_input_tokens: None,
        max_output_tokens: None,
        thinking_levels_override: None,
        max_input_tokens_override: None,
        max_output_tokens_override: None,
        compaction_support: Some(facts),
        compaction_support_override: Some(suppressed),
    };

    let section = compaction_section(json!({
        "messages_on_demand_compaction": {"default": "unsupported"}
    }));

    let resolved: Vec<(Mechanism, MechanismSupport)> =
        model.effective_mechanism_support(Some(&section)).collect();

    // The override suppresses the on-demand discovery fact into unknown, and the threshold
    // override stands; the declared default never overrules a known fact.
    assert_eq!(
        resolved,
        vec![(
            Mechanism::MessagesThresholdCompaction,
            MechanismSupport::Unsupported
        )]
    );

    let unoverridden = Model {
        compaction_support_override: None,
        ..model
    };

    let resolved: Vec<(Mechanism, MechanismSupport)> = unoverridden
        .effective_mechanism_support(Some(&section))
        .collect();

    // The discovery fact beats the section's declared unsupported default.
    assert_eq!(
        resolved,
        vec![(
            Mechanism::MessagesOnDemandCompaction,
            MechanismSupport::Supported
        )]
    );
}
