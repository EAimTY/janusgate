//! String-expression interpreter tests: the one evaluator every templated descriptor string
//! runs through — concatenated parts, slot fallback, `coalesce` selection, per-stage value
//! availability, and JSON template rendering (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot build is that test's failure report"
)]

use super::{EvalError, ValueEnv};
use crate::provider::definition::template::{StringExpr, TemplateNode, ValueExpr};
use serde_json::json;
use std::collections::BTreeMap;

/// The one-part literal expression of `text`.
fn literal(text: &str) -> StringExpr {
    StringExpr(vec![ValueExpr::Literal {
        value: text.to_string(),
    }])
}

/// The one-part expression carrying `part`.
fn single(part: ValueExpr) -> StringExpr {
    StringExpr(vec![part])
}

/// The working slots carrying the `region` slot and nothing else.
fn slots() -> BTreeMap<String, String> {
    BTreeMap::from([("region".to_string(), "eu-central".to_string())])
}

/// The inference-stage environment of one fixture dispatch.
fn inference_env<'a>(slots: &'a BTreeMap<String, String>) -> ValueEnv<'a> {
    ValueEnv::for_inference(slots, Some("https://up.test"), "key-1", "uuid-1", "model-1")
}

/// Literal parts concatenate verbatim, and the empty expression is the empty string.
#[test]
fn literals_concatenate_verbatim() {
    let slots = slots();

    let env = inference_env(&slots);

    let rendered = env
        .evaluate(&literal("https://up.test/v1"), "test")
        .expect("literal parts always evaluate");

    assert_eq!(rendered, "https://up.test/v1");

    let empty = env
        .evaluate(&StringExpr(vec![]), "test")
        .expect("the empty expression is the empty string");

    assert_eq!(empty, "");
}

/// A stage value renders where the stage carries it: the secret, base URL, model, and request
/// UUID of one inference dispatch — with one UUID shared by every reference of the dispatch.
#[test]
fn stage_values_render_where_the_stage_carries_them() {
    let slots = slots();

    let env = inference_env(&slots);

    let url = StringExpr(vec![
        ValueExpr::Base,
        ValueExpr::Literal {
            value: "/v1/".to_string(),
        },
        ValueExpr::Model,
        ValueExpr::Literal {
            value: "?d=".to_string(),
        },
        ValueExpr::RequestUuid,
        ValueExpr::Literal {
            value: "&again=".to_string(),
        },
        ValueExpr::RequestUuid,
    ]);

    let rendered = env.evaluate(&url, "test").expect("the stage carries all");

    assert_eq!(rendered, "https://up.test/v1/model-1?d=uuid-1&again=uuid-1");
}

/// A present slot renders its stored value and a missing slot evaluates to the empty string —
/// the fallback that lets a coalesce pick the first slot a flow actually placed.
#[test]
fn missing_slots_evaluate_empty() {
    let slots = slots();

    let env = inference_env(&slots);

    let present = env
        .evaluate(
            &single(ValueExpr::Slot {
                name: "region".to_string(),
            }),
            "test",
        )
        .expect("a present slot renders");

    assert_eq!(present, "eu-central");

    let absent = env
        .evaluate(
            &single(ValueExpr::Slot {
                name: "never-placed".to_string(),
            }),
            "test",
        )
        .expect("a missing slot is empty, never an error");

    assert_eq!(absent, "");
}

/// `coalesce` renders the first nonempty candidate; every candidate empty renders empty; a
/// missing slot inside a coalesce falls through to the next candidate.
#[test]
fn coalesce_selects_the_first_nonempty_candidate() {
    let slots = slots();

    let env = inference_env(&slots);

    let pick_second = StringExpr(vec![ValueExpr::Coalesce {
        parts: vec![
            ValueExpr::Slot {
                name: "never-placed".to_string(),
            },
            ValueExpr::Slot {
                name: "region".to_string(),
            },
            ValueExpr::Literal {
                value: "fallback".to_string(),
            },
        ],
    }]);

    let rendered = env
        .evaluate(&pick_second, "test")
        .expect("the second candidate is nonempty");

    assert_eq!(rendered, "eu-central");

    let pick_literal = StringExpr(vec![ValueExpr::Coalesce {
        parts: vec![
            ValueExpr::Literal {
                value: String::new(),
            },
            ValueExpr::Literal {
                value: "fallback".to_string(),
            },
        ],
    }]);

    let rendered = env
        .evaluate(&pick_literal, "test")
        .expect("the empty literal falls through");

    assert_eq!(rendered, "fallback");

    let all_empty = StringExpr(vec![ValueExpr::Coalesce {
        parts: vec![
            ValueExpr::Slot {
                name: "never-placed".to_string(),
            },
            ValueExpr::Literal {
                value: String::new(),
            },
        ],
    }]);

    let rendered = env
        .evaluate(&all_empty, "test")
        .expect("every candidate empty renders empty");

    assert_eq!(rendered, "");
}

/// A stage that does not carry a referenced value fails the evaluation, naming the kind and
/// the expression — the stored-drift shape the write boundary rejects.
#[test]
fn stage_missing_values_fail_the_evaluation() {
    let slots = slots();

    let stored = ValueEnv::for_stored_credential(&slots, Some("https://up.test"), "key-1");

    let error = stored
        .evaluate(&single(ValueExpr::Model), "discovery URL")
        .expect_err("the stored-credential stage carries no model");

    assert_eq!(
        error.to_string(),
        "discovery URL references the model, which this stage does not carry"
    );

    let authorize = ValueEnv::for_authorize_url(&slots, "client-1", None, "state-1");

    let error = authorize
        .evaluate(&single(ValueExpr::Base), "authorize URL")
        .expect_err("the authorize-URL stage carries no base");

    assert!(
        error
            .to_string()
            .contains("authorize URL references the base URL")
    );

    let error = authorize
        .evaluate(&single(ValueExpr::Secret), "authorize URL")
        .expect_err("the authorize-URL stage carries no secret");

    assert!(error.to_string().contains("references the secret"));
}

/// Each stage constructor carries exactly its documented values: the exchange environment has
/// the grant's code, verifier, and redirect URI, and the refresh environment the stored
/// refresh token — each absent from the other stages.
#[test]
fn stage_constructors_carry_their_documented_values() {
    let slots = slots();

    let exchange = ValueEnv::for_exchange(
        &slots,
        Some("https://up.test"),
        "client-1",
        Some("secret-1"),
        "code-1",
        Some("verifier-1"),
        "http://localhost:1455/callback",
    );

    let rendered = |env: &ValueEnv, part: &ValueExpr| env.evaluate(&single(part.clone()), "test");

    assert_eq!(
        rendered(&exchange, &ValueExpr::AuthorizationCode).expect("exchange carries the code"),
        "code-1"
    );

    assert_eq!(
        rendered(&exchange, &ValueExpr::CodeVerifier).expect("exchange carries the verifier"),
        "verifier-1"
    );

    assert_eq!(
        rendered(&exchange, &ValueExpr::RedirectUri).expect("exchange carries the redirect URI"),
        "http://localhost:1455/callback"
    );

    assert!(rendered(&exchange, &ValueExpr::DeviceCode).is_err());

    let refresh = ValueEnv::for_refresh_grant(&slots, None, "client-1", None, Some("refresh-1"));

    assert_eq!(
        rendered(&refresh, &ValueExpr::RefreshToken).expect("refresh carries the token"),
        "refresh-1"
    );

    assert!(rendered(&refresh, &ValueExpr::AuthorizationCode).is_err());

    let device_poll = ValueEnv::for_device_poll(&slots, None, "client-1", None, Some("dev-1"));

    assert_eq!(
        rendered(&device_poll, &ValueExpr::DeviceCode).expect("the poll carries the device code"),
        "dev-1"
    );

    assert!(rendered(&device_poll, &ValueExpr::ClientSecret).is_err());

    let provisioning = ValueEnv::for_provisioning(&slots, None, Some("tok"), "client-1", None);

    assert_eq!(
        rendered(&provisioning, &ValueExpr::Secret).expect("provisioning carries the secret"),
        "tok"
    );

    // A provisioning stage without a credential secret renders the reference as drift.
    let secretless = ValueEnv::for_provisioning(&slots, None, None, "client-1", None);

    assert!(rendered(&secretless, &ValueExpr::Secret).is_err());
}

/// The JSON template renderer keeps literal leaves verbatim, evaluates string expressions,
/// passes the codec body through at its placeholder, and renders object members in the map's
/// canonical order with arrays in the carried order.
#[test]
fn template_rendering_evaluates_leaves_and_keeps_literals() {
    let slots = slots();

    let env = inference_env(&slots);

    let template = TemplateNode::Object {
        fields: BTreeMap::from([
            (
                "model".to_string(),
                TemplateNode::String {
                    expr: single(ValueExpr::Model),
                },
            ),
            (
                "metadata".to_string(),
                TemplateNode::Object {
                    fields: BTreeMap::from([
                        (
                            "region".to_string(),
                            TemplateNode::Json {
                                value: json!("eu-central"),
                            },
                        ),
                        (
                            "dispatch".to_string(),
                            TemplateNode::String {
                                expr: single(ValueExpr::RequestUuid),
                            },
                        ),
                    ]),
                },
            ),
            (
                "items".to_string(),
                TemplateNode::Array {
                    items: vec![
                        TemplateNode::Json { value: json!(1) },
                        TemplateNode::String {
                            expr: literal("two"),
                        },
                    ],
                },
            ),
        ]),
    };

    let rendered = env
        .render_json(&template, &(), "envelope")
        .expect("every expression evaluates");

    let rendered = serde_json::to_value(&rendered).expect("the rendering serializes");

    assert_eq!(
        rendered,
        json!({
            "model": "model-1",
            "metadata": {"region": "eu-central", "dispatch": "uuid-1"},
            "items": [1, "two"]
        })
    );

    // The codec body rides the placeholder as its concrete borrow.
    let with_body = TemplateNode::Array {
        items: vec![
            TemplateNode::Body,
            TemplateNode::String {
                expr: literal("tail"),
            },
        ],
    };

    let rendered = env
        .render_json(&with_body, &7_i64, "envelope")
        .expect("the body renders at its placeholder");

    let rendered = serde_json::to_value(&rendered).expect("the rendering serializes");

    assert_eq!(rendered, json!([7, "tail"]));
}

/// A template string leaf referencing an unavailable stage value fails the whole rendering.
#[test]
fn template_rendering_propagates_evaluation_failures() {
    let slots = BTreeMap::new();

    let stored = ValueEnv::for_stored_credential(&slots, None, "key-1");

    let template = TemplateNode::Object {
        fields: BTreeMap::from([
            ("ok".to_string(), TemplateNode::Json { value: json!(true) }),
            (
                "model".to_string(),
                TemplateNode::String {
                    expr: single(ValueExpr::Model),
                },
            ),
        ]),
    };

    let error: EvalError = stored
        .render_json(&template, &(), "envelope")
        .err()
        .expect("the model part is absent from the stored-credential stage");

    assert!(error.to_string().contains("envelope references the model"));
}

/// The response path resolver: the empty chain addresses the root, a member chain walks
/// objects only, and a missing member or a non-object intermediate yields `None`.
#[test]
fn response_paths_resolve_object_members_only() {
    use crate::provider::definition::template::ResponsePath;

    let document = json!({
        "a": {"b": {"c": "found"}},
        "list": [1, 2]
    });

    let root = ResponsePath(vec![]);

    assert_eq!(root.resolve(&document), Some(&document));

    let deep = ResponsePath(vec!["a".to_string(), "b".to_string(), "c".to_string()]);

    assert_eq!(deep.resolve(&document), Some(&json!("found")));

    let missing = ResponsePath(vec!["a".to_string(), "x".to_string()]);

    assert_eq!(missing.resolve(&document), None);

    let into_array = ResponsePath(vec!["list".to_string(), "0".to_string()]);

    assert_eq!(into_array.resolve(&document), None);

    let into_scalar = ResponsePath(vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ]);

    assert_eq!(into_scalar.resolve(&document), None);
}
