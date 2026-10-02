//! Pure quota normalization contracts, independent of upstream products.
#![expect(
    clippy::unwrap_used,
    reason = "test fixtures and their expected outcomes are deliberate"
)]

use super::extract;
use crate::provider::definition::quota::QuotaDescriptor;
use serde_json::{Value, json};

/// A required entry with optional numeric facts and reset metadata.
fn entry() -> Value {
    json!({
        "key": "primary",
        "label": "Primary",
        "at": [],
        "is_required": true,
        "window": {
            "type": "duration_field",
            "value": {
                "field": {
                    "base": "entry",
                    "path": [
                        "seconds"
                    ],
                    "is_required": false
                },
                "encoding": "number"
            },
            "unit": "seconds"
        },
        "measurement": {
            "type": "counter",
            "value": {
                "field": {
                    "base": "entry",
                    "path": [
                        "used"
                    ],
                    "is_required": false
                },
                "encoding": "number_or_numeric_string"
            },
            "limit": {
                "field": {
                    "base": "entry",
                    "path": [
                        "limit"
                    ],
                    "is_required": false
                },
                "encoding": "number"
            },
            "direction": "used",
            "unit": "requests"
        },
        "reset": {
            "type": "field",
            "field": {
                "base": "root",
                "path": [
                    "reset"
                ],
                "is_required": false
            },
            "encoding": "rfc3339"
        },
        "is_unlimited": {
            "base": "entry",
            "path": [
                "unlimited"
            ],
            "is_required": false
        }
    })
}

/// A map source with stable pool identities and root metadata.
///
/// # Panics
///
/// Panics if fixture parsing or a quota contract assertion fails.
fn descriptor() -> QuotaDescriptor {
    serde_json::from_value(json!({
        "request": {
            "method": "GET",
            "url": [
                {
                    "type": "literal",
                    "value": "https://example.test/usage"
                }
            ]
        },
        "sources": [
            {
                "type": "map",
                "key": "pools",
                "at": [
                    "pools"
                ],
                "is_required": true,
                "scope": {
                    "type": "map_key"
                },
                "entries": [
                    entry()
                ]
            }
        ]
    }))
    .unwrap()
}

/// Preserves stable order, over-limit usage, unknown facts, and unlimited precedence.
///
/// # Panics
///
/// Panics if fixture parsing or a quota contract assertion fails.
#[test]
fn map_pools_and_root_metadata_are_atomic() {
    let result = extract(
        &json!({"reset":"2030-01-01T01:00:00.123456+01:00","pools":{
        "z":{"unlimited":true,"limit":0,"used":"garbage"},
        "a":{"used":"125.5","limit":100,"seconds":18000},
        "b":{}}}),
        &descriptor(),
    )
    .unwrap();
    let [a, b, z]: [_; 3] = result.try_into().unwrap();
    assert_eq!(a.key, ["pools", "a", "primary"]);
    assert_eq!(
        serde_json::to_value(a.measurement).unwrap(),
        json!({"type":"counter","used":125.5,"limit":100.0,"unit":"requests"})
    );
    assert_eq!(
        serde_json::to_value(a.reset).unwrap(),
        json!({"type":"at","at":1893456000123456_i64})
    );
    assert_eq!(
        serde_json::to_value(b.measurement).unwrap(),
        json!({"type":"unknown"})
    );
    assert_eq!(
        serde_json::to_value(z.measurement).unwrap(),
        json!({"type":"unlimited"})
    );
    for item in [
        json!({"used":"bad"}),
        json!({"used":1,"limit":0}),
        json!({"seconds":1.5}),
        json!({"unlimited":1}),
        json!({"used":9007199254740992_u64}),
    ] {
        assert!(
            extract(&json!({"pools":{"a":item}}), &descriptor()).is_err(),
            "{item}"
        );
    }
}

/// Optional absence never hides malformed present paths or facts.
///
/// # Panics
///
/// Panics if fixture parsing or a quota contract assertion fails.
#[test]
fn absence_and_shape_are_distinct() {
    let mut descriptor = descriptor();
    if let crate::provider::definition::quota::QuotaSource::Map { is_required, .. } =
        descriptor.sources.first_mut().unwrap()
    {
        *is_required = false;
    }
    assert!(extract(&json!({}), &descriptor).unwrap().is_empty());
    assert!(
        extract(&json!({"pools":null}), &descriptor)
            .unwrap()
            .is_empty()
    );
    assert!(
        extract(&json!({"pools":{}}), &descriptor)
            .unwrap()
            .is_empty()
    );
    for body in [
        json!([]),
        json!({"pools":[]}),
        json!({"pools":{"a":null}}),
        json!({"pools":{"a":{"used":false}}}),
    ] {
        assert!(extract(&body, &descriptor).is_err(), "{body}");
    }
    if let crate::provider::definition::quota::QuotaSource::Map { at, .. } =
        descriptor.sources.first_mut().unwrap()
    {
        at.0 = vec!["nested".into(), "pools".into()];
    }
    assert!(extract(&json!({"nested":42}), &descriptor).is_err());
}

/// Timestamp precision, calendar validity, and safe integers survive normalization exactly.
///
/// # Panics
///
/// Panics if fixture parsing or a quota contract assertion fails.
#[test]
fn reset_precision_and_numeric_grammar_are_strict() {
    for reset in [
        "2030-01-01T00:00:00.1234567Z",
        "2030-01-01T00:00:00.0000000001Z",
        "2030-01-01T00:00:00.1234560001Z",
        "2030-02-30T00:00:00Z",
        "not-a-date",
    ] {
        assert!(extract(&json!({"reset":reset,"pools":{"a":{}}}), &descriptor()).is_err());
    }
    assert!(
        extract(
            &json!({"reset":"2030-01-01T00:00:00.1234560000Z","pools":{"a":{}}}),
            &descriptor()
        )
        .is_ok()
    );
    for used in [" 1", "1 ", "01", "NaN", "10%", "-1", "9007199254740993"] {
        assert!(
            extract(
                &json!({"pools":{"a":{"used":used,"limit":10}}}),
                &descriptor()
            )
            .is_err(),
            "{used}"
        );
    }
    assert!(
        extract(
            &json!({"pools":{"a":{"used":"1e1","limit":10}}}),
            &descriptor()
        )
        .is_ok()
    );
}

/// Builds a descriptor with a caller-selected measurement, reset, and period.
///
/// # Panics
///
/// Panics if the deliberately typed fixture fails to parse.
fn customized(measurement: Value, reset: Value, window: Value) -> QuotaDescriptor {
    let mut entry = entry();
    let object = entry.as_object_mut().unwrap();
    object.insert("measurement".into(), measurement);
    object.insert("reset".into(), reset);
    object.insert("window".into(), window);
    serde_json::from_value(json!({
        "request":{"method":"GET","url":[{"type":"literal","value":"https://example.test"}]},
        "sources":[{"type":"object","key":"account","at":[],"is_required":true,
            "scope":{"type":"unknown"},"entries":[entry]}]
    }))
    .unwrap()
}

/// Scalar fields used by the normalization tables.
fn numeric(path: &str, encoding: &str) -> Value {
    json!({"field":{"base":"entry","path":[path],"is_required":false},"encoding":encoding})
}

/// Protects orientation, explicit encodings, decimal amounts, and positive limits.
///
/// # Panics
///
/// Panics if normalization diverges from the declared numeric domain.
#[test]
fn measurement_normalization_and_domains() {
    for (scale, direction, value, expected) in [
        ("percent", "used", 125.5, 1.255),
        ("percent", "remaining", 25.0, 0.75),
        ("fraction", "used", 0.0, 0.0),
        ("fraction", "remaining", 0.0, 1.0),
        ("fraction", "used", 1.25, 1.25),
        ("fraction", "used", -0.0, 0.0),
    ] {
        let descriptor = customized(
            json!({"type":"ratio",
                "value":numeric("value",
                "number"),
                "scale":scale,
                "direction":direction}),
            json!({"type":"unknown"}),
            json!({"type":"unknown"}),
        );
        let observation = extract(&json!({"value":value}), &descriptor)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            serde_json::to_value(observation.measurement).unwrap(),
            json!({"type":"ratio","used_fraction":expected})
        );
        assert!(extract(&json!({"value":"0.5"}), &descriptor).is_err());
    }
    for (scale, value) in [("fraction", 1.01), ("percent", 100.1)] {
        let descriptor = customized(
            json!({"type":"ratio",
                "value":numeric("value",
                "number"),
                "scale":scale,
                "direction":"remaining"}),
            json!({"type":"unknown"}),
            json!({"type":"unknown"}),
        );
        assert!(extract(&json!({"value":value}), &descriptor).is_err());
    }
    let descriptor = customized(
        json!({"type":"counter",
            "value":numeric("value",
            "number"),
            "limit":numeric("limit",
            "number"),
            "direction":"remaining",
            "unit":"requests"}),
        json!({"type":"unknown"}),
        json!({"type":"unknown"}),
    );
    let observation = extract(&json!({"value":2.5,"limit":10}), &descriptor)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        serde_json::to_value(observation.measurement).unwrap(),
        json!({"type":"counter","used":7.5,"limit":10.0,"unit":"requests"})
    );
    for body in [
        json!({"value":11,"limit":10}),
        json!({"limit":0}),
        json!({"limit":"bad"}),
        json!({"value":false}),
    ] {
        assert!(extract(&body, &descriptor).is_err());
    }
    let descriptor = customized(
        json!({"type":"remaining","value":numeric("value","numeric_string"),"unit":"tokens"}),
        json!({"type":"unknown"}),
        json!({"type":"calendar","unit":"month"}),
    );
    assert!(extract(&json!({"value":1}), &descriptor).is_err());
    let observation = extract(&json!({"value":"1.25"}), &descriptor)
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        serde_json::to_value(observation.measurement).unwrap(),
        json!({"type":"remaining","remaining":1.25,"unit":"tokens"})
    );
}

/// Preserves tuple segments, repeated periods, deterministic arrays, and duplicate detection.
///
/// # Panics
///
/// Panics if stable identity, ordering, or requiredness diverges.
#[test]
fn arrays_preserve_identity_and_independent_windows() {
    let mut primary = entry();
    primary
        .as_object_mut()
        .unwrap()
        .insert("at".into(), json!(["primary"]));
    let mut secondary = primary.clone();
    secondary
        .as_object_mut()
        .unwrap()
        .insert("key".into(), json!("secondary"));
    secondary
        .as_object_mut()
        .unwrap()
        .insert("at".into(), json!(["secondary"]));
    secondary
        .as_object_mut()
        .unwrap()
        .insert("is_required".into(), json!(false));
    let descriptor: QuotaDescriptor = serde_json::from_value(json!({
        "request":{"method":"GET","url":[{"type":"literal","value":"https://example.test"}]},
        "sources":[{"type":"array","key":"additional","at":["additional"],"is_required":true,
            "scope":{"type":"field",
                "field":{"base":"item",
                "path":["scope"],
                "is_required":false},
                "encoding":"string"},

            "identity":[
                {"type":"field",
                    "field":{"base":"item",
                    "path":["id"],
                    "is_required":true},
                    "encoding":"string"},

                {"type":"field",
                    "field":{"base":"item",
                    "path":["part"],
                    "is_required":true},
                    "encoding":"string"}],

            "entries":[primary,secondary]}]
    }))
    .unwrap();
    let body = json!({"additional":[{"id":"a/b",
        "part":"c",
        "primary":{},
        "secondary":{}},
        {"id":"a",
        "part":"b/c",
        "primary":{}}]});
    let observations = extract(&body, &descriptor).unwrap();
    let keys: Vec<_> = observations.iter().map(|entry| entry.key.clone()).collect();
    assert_eq!(
        keys,
        vec![
            vec!["additional", "a/b", "c", "primary"],
            vec!["additional", "a/b", "c", "secondary"],
            vec!["additional", "a", "b/c", "primary"]
        ]
    );
    for additional in [
        json!([{"id":"a","part":"b","primary":{}},{"id":"a","part":"b","primary":{}}]),
        json!([null]),
        json!([{"id":"a","part":"b"}]),
        json!([{"id":null,"part":"b","primary":{}}]),
        json!([{"id":"a","part":"b","primary":{},"secondary":false}]),
    ] {
        assert!(extract(&json!({"additional":additional}), &descriptor).is_err());
    }
}

/// Protects exact Unix units, date-only semantics, safe overflow, and past reset acceptance.
///
/// # Panics
///
/// Panics if a reset or fixed period crosses the wrong domain boundary.
#[test]
fn time_units_and_calendar_dates() {
    for (encoding, value, expected) in [
        ("unix_seconds", json!(1), json!({"type":"at","at":1000000})),
        (
            "unix_milliseconds",
            json!(-1),
            json!({"type":"at","at":-1000}),
        ),
        (
            "date",
            json!("2024-02-29"),
            json!({"type":"date","date":"2024-02-29"}),
        ),
    ] {
        let descriptor = customized(
            json!({"type":"unknown"}),
            json!({"type":"field",
                "field":{"base":"entry",
                "path":["reset"],
                "is_required":true},
                "encoding":encoding}),
            json!({"type":"duration_field","value":numeric("duration","number"),"unit":"hours"}),
        );
        let observation = extract(&json!({"reset":value,"duration":5}), &descriptor)
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(serde_json::to_value(observation.reset).unwrap(), expected);
        assert_eq!(
            serde_json::to_value(observation.window).unwrap(),
            json!({"type":"duration","seconds":18000})
        );
        assert!(
            extract(
                &json!({"reset":value,"duration":9007199254740991_u64}),
                &descriptor
            )
            .is_err()
        );
    }
    for (encoding, value) in [
        ("unix_seconds", json!(9007199255_i64)),
        ("unix_seconds", json!(1.5)),
        ("unix_milliseconds", json!(9223372036854775807_i64)),
        ("date", json!("2023-02-29")),
        ("date", json!("2024-2-09")),
        ("date", json!("2024-01-01T00:00:00Z")),
    ] {
        let descriptor = customized(
            json!({"type":"unknown"}),
            json!({"type":"field",
                "field":{"base":"entry",
                "path":["reset"],
                "is_required":true},
                "encoding":encoding}),
            json!({"type":"unknown"}),
        );
        assert!(extract(&json!({"reset":value}), &descriptor).is_err());
    }
}

/// The definition boundary rejects unavailable expressions and invalid extraction contexts,
/// and quota-only base references participate in the provider base-URL requirement.
///
/// # Panics
///
/// Panics if a fixture parses or validates on the wrong side of the contract.
#[test]
fn definition_validation_covers_contexts_and_all_request_leaves() {
    use crate::{format::UpstreamFormat, provider::definition::template::ValueExpr};
    let mut payload =
        crate::test_support::static_key_definition_payload(UpstreamFormat::OpenAiResponse);
    payload.inference.url =
        crate::provider::definition::template::StringExpr::literal("https://example.test");
    payload.discovery = None;
    payload.quota = Some(descriptor());
    payload.validate().unwrap();
    assert!(!payload.references_base());
    for request in [
        json!({"method":"GET","url":[{"type":"base"}]}),
        json!({"method":"GET",
            "url":[{"type":"literal",
            "value":"https://example.test"}],
            "headers":[{"name":"X-Base",
            "value":[{"type":"base"}]}]}),
        json!({"method":"GET",
            "url":[{"type":"literal",
            "value":"https://example.test"}],
            "query":[{"name":"base",
            "value":[{"type":"coalesce",
            "parts":[{"type":"base"}]}]}]}),
        json!({"method":"POST",
            "url":[{"type":"literal",
            "value":"https://example.test"}],
            "body":{"kind":"form",
            "fields":[{"name":"base",
            "value":[{"type":"base"}]}]}}),
        json!({"method":"POST",
            "url":[{"type":"literal",
            "value":"https://example.test"}],
            "body":{"kind":"json",
            "node":{"type":"string",
            "expr":[{"type":"base"}]}}}),
    ] {
        payload.quota.as_mut().unwrap().request = serde_json::from_value(request).unwrap();
        payload.validate().unwrap();
        assert!(payload.references_base());
    }
    for expression in [
        ValueExpr::Model,
        ValueExpr::RequestUuid,
        ValueExpr::ClientId,
        ValueExpr::ClientSecret,
        ValueExpr::RefreshToken,
        ValueExpr::Slot {
            name: "unknown".into(),
        },
    ] {
        payload.quota.as_mut().unwrap().request.url.0 = vec![expression];
        assert!(payload.validate().is_err());
    }
    // Array identity must be stable item data; neither constants nor other contexts suffice.
    for identity in [
        json!([]),
        json!([{"type":"literal","value":"constant"}]),
        json!([{"type":"map_key"}]),
        json!([{"type":"field","field":{"base":"root","path":["id"],"is_required":true},"encoding":"string"}]),
        json!([{"type":"field","field":{"base":"item","path":["id"],"is_required":false},"encoding":"string"}]),
        json!([{"type":"field","field":{"base":"entry","path":["id"],"is_required":true},"encoding":"string"}]),
    ] {
        payload.quota = Some(serde_json::from_value(json!({
            "request":{"method":"GET","url":[{"type":"literal","value":"https://example.test"}]},
            "sources":[{"type":"array","key":"array","at":[],"is_required":true,
                "identity":identity,"scope":{"type":"unknown"},"entries":[entry()]}]
        })).unwrap());
        assert!(payload.validate().is_err());
    }
    let mut duplicate = descriptor();
    duplicate
        .sources
        .push(duplicate.sources.first().unwrap().clone());
    payload.quota = Some(duplicate);
    assert!(payload.validate().is_err());
    let mut duplicate = descriptor();
    if let crate::provider::definition::quota::QuotaSource::Map { entries, .. } =
        duplicate.sources.first_mut().unwrap()
    {
        entries.push(entries.first().unwrap().clone());
    }
    payload.quota = Some(duplicate);
    assert!(payload.validate().is_err());
    let original = serde_json::to_value(descriptor()).unwrap();
    for path in [
        vec!["sources"],
        vec!["sources", "0", "entries"],
        vec!["sources", "0", "key"],
    ] {
        // Walk fixture objects/arrays explicitly; production uses only concrete descriptor types.
        let mut value = original.clone();
        let mut current = &mut value;
        for part in path.iter().take(path.len() - 1) {
            current = if let Ok(index) = part.parse::<usize>() {
                current.get_mut(index).unwrap()
            } else {
                current.get_mut(*part).unwrap()
            };
        }
        let leaf = path.last().unwrap();
        let replacement = if *leaf == "key" { json!("") } else { json!([]) };
        current
            .as_object_mut()
            .unwrap()
            .insert((*leaf).into(), replacement);
        payload.quota = Some(serde_json::from_value(value).unwrap());
        assert!(payload.validate().is_err());
    }
    let mut value = original.clone();
    value
        .as_object_mut()
        .unwrap()
        .insert("unknown".into(), json!(true));
    assert!(serde_json::from_value::<QuotaDescriptor>(value).is_err());
    let mut value = original;
    let source = value
        .get_mut("sources")
        .unwrap()
        .get_mut(0)
        .unwrap()
        .as_object_mut()
        .unwrap();
    source.insert(
        "scope".into(),
        json!({"type":"field",
        "field":{"base":"entry",
        "path":[],
        "is_required":false},
        "encoding":"string"}),
    );
    payload.quota = Some(serde_json::from_value(value).unwrap());
    assert!(payload.validate().is_err());
}

/// Reviewed upstream-shaped fixture with its actual catalog descriptor and normalized result.
#[derive(serde::Deserialize)]
struct PresetFixture {
    /// Recipe emitted by the frontend catalog.
    descriptor: QuotaDescriptor,
    /// Foreign JSON shape from the pinned reader/types, with synthetic values.
    response: Value,
    /// Complete expected list, including independent repeated restrictions.
    expected: Vec<super::types::QuotaEntry>,
}

/// All researched catalog mappings normalize representative shapes atomically.
///
/// # Panics
///
/// Panics if a reviewed fixture or expected normalization diverges.
#[test]
fn researched_preset_shapes_normalize_without_product_branches() {
    for (name, fixture) in [
        (
            "codex",
            include_str!("../../../web/src/quota-fixtures/codex.json"),
        ),
        (
            "claude-code",
            include_str!("../../../web/src/quota-fixtures/claude-code.json"),
        ),
        (
            "kimi-code",
            include_str!("../../../web/src/quota-fixtures/kimi-code.json"),
        ),
        (
            "github-copilot",
            include_str!("../../../web/src/quota-fixtures/github-copilot.json"),
        ),
        (
            "antigravity",
            include_str!("../../../web/src/quota-fixtures/antigravity.json"),
        ),
    ] {
        let fixture: PresetFixture = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            extract(&fixture.response, &fixture.descriptor).unwrap(),
            fixture.expected,
            "{name}"
        );
    }
}
