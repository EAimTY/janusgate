//! Quota extraction tests: strict atomic parsing of foreign quota JSON through authored
//! descriptors — the three source shapes, identity tuples, scopes, routing, measurements,
//! windows, resets, and the atomicity and numeric-domain rules
//! (`docs/design/providers.md`).

#![expect(
    clippy::expect_used,
    clippy::missing_panics_doc,
    reason = "a test fails by panicking: a # Panics section on every test would restate that, \
              and a fixture that cannot parse is that test's failure report"
)]

use super::extract;
use crate::provider::{
    definition::quota::QuotaDescriptor,
    quota::{
        QuotaEntry, SafeDuration,
        types::{
            Amount, PositiveAmount, QuotaMeasurement, QuotaReset, QuotaRouting, QuotaScope,
            QuotaWindow,
        },
    },
};
use serde_json::{Value, json};

/// Parses one quota descriptor fixture.
fn descriptor(fixture: Value) -> QuotaDescriptor {
    serde_json::from_value(fixture).expect("the quota descriptor fixture parses")
}

/// The canonical entry extraction of the fixture descriptor: a counter measured in credits
/// over a fixed window with a future reset and account-wide applicability.
fn counter_entry() -> Value {
    json!({
        "key": "credits",
        "label": "Monthly credits",
        "at": ["credits"],
        "is_required": true,
        "window": {"type": "duration", "seconds": 2_592_000},
        "measurement": {"type": "counter",
                        "value": {"field": {"base": "entry", "path": ["used"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "limit": {"field": {"base": "entry", "path": ["limit"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "direction": "used", "unit": "credits"},
        "reset": {"type": "field",
                  "field": {"base": "entry", "path": ["reset_at"], "is_required": true},
                  "encoding": "rfc3339"},
        "routing": {"type": "all_models"}
    })
}

/// The canonical descriptor: one array source over the fixture payload's organizations, the
/// identity resolved from each item's `id` member.
fn array_descriptor(entries: Value) -> QuotaDescriptor {
    descriptor(json!({
        "request": {"method": "GET",
         "url": [{"type": "literal", "value": "https://up.test/quota"}]},
        "sources": [{
            "type": "array",
            "key": "orgs",
            "at": ["data", "organizations"],
            "is_required": true,
            "identity": [{"type": "field",
                          "field": {"base": "item", "path": ["id"], "is_required": true},
                          "encoding": "string"}],
            "scope": {"type": "field",
                      "field": {"base": "item", "path": ["scope"], "is_required": false},
                      "encoding": "string"},
            "entries": entries
        }]
    }))
}

/// The canonical fixture payload of one upstream quota response: two organizations carrying
/// the counter entry with different magnitudes.
fn fixture_payload() -> Value {
    json!({
        "data": {
            "requested_models": ["m-a", "m-b"],
            "organizations": [
                {
                    "id": "org-1",
                    "scope": "team",
                    "credits": {
                        "used": 120,
                        "limit": 1000,
                        "reset_at": "2030-01-01T00:00:00Z"
                    }
                },
                {
                    "id": "org-2",
                    "scope": "personal",
                    "credits": {
                        "used": 10,
                        "limit": 10,
                        "reset_at": "2030-02-01T00:00:00Z"
                    }
                }
            ]
        }
    })
}

/// The observation summary of one extracted entry as assertion text: identity, measurement
/// tag, routing, scope, window, and reset.
fn summary(entry: &QuotaEntry) -> String {
    let measurement = match &entry.measurement {
        QuotaMeasurement::Unknown => "unknown".to_string(),

        QuotaMeasurement::Unlimited => "unlimited".to_string(),

        QuotaMeasurement::Ratio { used_fraction } => {
            format!("ratio:{}", f64::from(*used_fraction))
        }

        QuotaMeasurement::Counter { used, limit, unit } => {
            format!(
                "counter:{}{unit}/{}{unit}",
                f64::from(*used),
                f64::from(*limit)
            )
        }

        QuotaMeasurement::Remaining { remaining, unit } => {
            format!("remaining:{}{unit}", f64::from(*remaining))
        }
    };

    let routing = match &entry.routing {
        QuotaRouting::MonitoringOnly => "monitoring".to_string(),

        QuotaRouting::AllModels => "all".to_string(),

        QuotaRouting::Models { upstream_ids } => format!("models:{upstream_ids:?}"),
    };

    let window = match &entry.window {
        QuotaWindow::Unknown => "unknown".to_string(),

        QuotaWindow::Duration { seconds } => format!("duration:{}", u64::from(*seconds)),

        QuotaWindow::Calendar { .. } => "calendar".to_string(),
    };

    let reset = match &entry.reset {
        QuotaReset::Unknown => "unknown".to_string(),

        QuotaReset::At { at } => format!("at:{}", i64::from(*at)),

        QuotaReset::Date { date } => format!("date:{date}"),
    };

    let scope = match &entry.scope {
        QuotaScope::Unknown => "unknown".to_string(),

        QuotaScope::Named { name } => format!("scope:{name}"),
    };

    format!(
        "{} | {measurement} | {routing} | {window} | {reset} | {scope} | {}",
        entry.key.join("/"),
        entry.label
    )
}

/// The extraction failure of `payload` under `descriptor` as assertion text.
fn failure(descriptor: &QuotaDescriptor, payload: &Value) -> String {
    extract(payload, descriptor)
        .expect_err("the extraction fails")
        .to_string()
}

/// The observation of a reading known to carry the addressed one; an out-of-range index is the
/// test's failure report.
fn entry(entries: &[QuotaEntry], index: usize) -> &QuotaEntry {
    #[expect(
        clippy::expect_used,
        reason = "a reading missing its expected observation is that test's failure report"
    )]
    entries
        .get(index)
        .expect("the reading carries the observation")
}

/// The canonical array extraction reads both organizations in order: the counter entry with
/// its full identity tuple, named scope, fixed window, instant reset, and account-wide
/// routing — one observation per item per configured entry.
#[test]
fn array_extraction_reads_the_canonical_payload() {
    let fixture = array_descriptor(json!([counter_entry()]));

    let entries = extract(&fixture_payload(), &fixture).expect("the payload extracts");

    assert_eq!(entries.len(), 2, "one observation per organization");

    assert_eq!(
        summary(entry(&entries, 0)),
        "orgs/org-1/credits | counter:120credits/1000credits | all | duration:2592000 | \
         at:1893456000000000 | scope:team | Monthly credits"
    );

    assert_eq!(
        summary(entry(&entries, 1)),
        "orgs/org-2/credits | counter:10credits/10credits | all | duration:2592000 | \
         at:1896134400000000 | scope:personal | Monthly credits"
    );

    // Exhaustion signaling rides the normalized measurement, not the raw magnitudes: a
    // below-limit counter is available, a used==limit counter is exhausted.
    assert_eq!(entry(&entries, 0).measurement.is_exhausted(), Some(false));

    assert_eq!(entry(&entries, 1).measurement.is_exhausted(), Some(true));
}

/// The map source visits its members in lexicographic key order, using each member key as the
/// identity segment and making it available to the scope.
#[test]
fn map_extraction_orders_members_and_names_their_keys() {
    let fixture = descriptor(json!({
        "request": {"method": "GET",
         "url": [{"type": "literal", "value": "https://up.test/quota"}]},
        "sources": [{
            "type": "map",
            "key": "models",
            "at": ["models"],
            "is_required": true,
            "scope": {"type": "map_key"},
            "entries": [{
                "key": "pool",
                "label": "Pool",
                "at": [],
                "is_required": true,
                "window": {"type": "unknown"},
                "measurement": {"type": "remaining",
                                "value": {"field": {"base": "entry", "path": ["left"],
                                                    "is_required": true},
                                          "encoding": "number"},
                                "unit": "tokens"},
                "reset": {"type": "unknown"}
            }]
        }]
    }));

    let payload = json!({
        "models": {
            "zeta": {"left": 5},
            "alpha": {"left": 0}
        }
    });

    let entries = extract(&payload, &fixture).expect("the map extracts");

    let summaries: Vec<String> = entries.iter().map(summary).collect();

    assert_eq!(
        summaries,
        vec![
            "models/alpha/pool | remaining:0tokens | monitoring | unknown | unknown | \
             scope:alpha | Pool",
            "models/zeta/pool | remaining:5tokens | monitoring | unknown | unknown | \
             scope:zeta | Pool",
        ]
    );

    // Exhaustion signaling rides the normalized measurement: a zero remaining is exhausted.
    assert_eq!(entry(&entries, 0).measurement.is_exhausted(), Some(true));

    assert_eq!(entry(&entries, 1).measurement.is_exhausted(), Some(false));
}

/// The object source selects one whole item with no identity segment beyond the source key.
#[test]
fn object_extraction_selects_the_single_item() {
    let fixture = descriptor(json!({
        "request": {"method": "GET",
         "url": [{"type": "literal", "value": "https://up.test/quota"}]},
        "sources": [{
            "type": "object",
            "key": "plan",
            "at": ["plan"],
            "is_required": true,
            "entries": [{
                "key": "requests",
                "label": "Requests",
                "at": [],
                "is_required": true,
                "window": {"type": "calendar", "unit": "month"},
                "measurement": {"type": "ratio",
                                "value": {"field": {"base": "entry", "path": ["ratio"],
                                                    "is_required": true},
                                          "encoding": "number"},
                                "scale": "percent", "direction": "used"},
                "reset": {"type": "field",
                          "field": {"base": "root", "path": ["cycle"], "is_required": true},
                          "encoding": "unix_seconds"}
            }]
        }]
    }));

    let payload = json!({"plan": {"ratio": 75}, "cycle": 1_893_456_000});

    let entries = extract(&payload, &fixture).expect("the object extracts");

    assert_eq!(
        summary(entry(&entries, 0)),
        "plan/requests | ratio:0.75 | monitoring | calendar | at:1893456000000000 | unknown | \
         Requests"
    );
}

/// Ratio normalization: fractions pass through, percentages scale down, remaining directions
/// invert within their scale, used ratios may exceed one, and an out-of-scale remaining fails.
#[test]
fn ratio_measurements_normalize_their_scale_and_direction() {
    let ratio_fixture = |scale: &str, direction: &str| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "requests", "label": "Requests", "at": [], "is_required": true,
                    "window": {"type": "unknown"},
                    "measurement": {"type": "ratio",
                                    "value": {"field": {"base": "entry", "path": ["ratio"],
                                                        "is_required": true},
                                              "encoding": "number"},
                                    "scale": scale, "direction": direction},
                    "reset": {"type": "unknown"}
                }]
            }]
        }))
    };

    let read = |payload: Value, scale: &str, direction: &str| {
        extract(&payload, &ratio_fixture(scale, direction))
            .expect("the ratio extracts")
            .remove(0)
    };

    let used_percent = read(json!({"plan": {"ratio": 75}}), "percent", "used");

    assert_eq!(
        summary(&used_percent).split(" | ").nth(1),
        Some("ratio:0.75")
    );

    let remaining_fraction = read(json!({"plan": {"ratio": 0.4}}), "fraction", "remaining");

    assert_eq!(
        summary(&remaining_fraction).split(" | ").nth(1),
        Some("ratio:0.6")
    );

    // A used ratio beyond one stays visible as exhaustion evidence.
    let overused = read(json!({"plan": {"ratio": 1.5}}), "fraction", "used");

    assert_eq!(overused.measurement.is_exhausted(), Some(true));

    let out_of_scale = failure(
        &ratio_fixture("fraction", "remaining"),
        &json!({"plan": {"ratio": 1.5}}),
    );

    assert!(
        out_of_scale.contains("invalid numeric domain"),
        "got: {out_of_scale}"
    );
}

/// Counter semantics: a used amount may exceed its positive limit; a remaining amount beyond
/// its limit and a zero limit both fail.
#[test]
fn counter_measurements_keep_the_declared_bounds() {
    let counter_fixture = |direction: &str| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "credits", "label": "Credits", "at": [], "is_required": true,
                    "window": {"type": "unknown"},
                    "measurement": {"type": "counter",
                                    "value": {"field": {"base": "entry", "path": ["used"],
                                                        "is_required": true},
                                              "encoding": "number"},
                                    "limit": {"field": {"base": "entry", "path": ["limit"],
                                                        "is_required": true},
                                              "encoding": "number"},
                                    "direction": direction, "unit": "credits"},
                    "reset": {"type": "unknown"}
                }]
            }]
        }))
    };

    let overused = extract(
        &json!({"plan": {"used": 1200, "limit": 1000}}),
        &counter_fixture("used"),
    )
    .expect("a used amount may exceed its limit");

    assert_eq!(
        summary(entry(&overused, 0)),
        "plan/credits | counter:1200credits/1000credits | monitoring | unknown | unknown | \
         unknown | Credits"
    );

    assert_eq!(entry(&overused, 0).measurement.is_exhausted(), Some(true));

    let out_of_bounds = failure(
        &counter_fixture("remaining"),
        &json!({"plan": {"used": 1200, "limit": 1000}}),
    );

    assert!(
        out_of_bounds.contains("invalid numeric domain"),
        "got: {out_of_bounds}"
    );

    let zero_limit = failure(
        &counter_fixture("used"),
        &json!({"plan": {"used": 5, "limit": 0}}),
    );

    assert!(
        zero_limit.contains("invalid numeric domain"),
        "got: {zero_limit}"
    );

    // A counter whose optional numeric fields are absent reads as unknown after the present
    // fields validate; a required absence fails instead (exercised above).
    let optional_counter = descriptor(json!({
        "request": {"method": "GET",
         "url": [{"type": "literal", "value": "https://up.test/quota"}]},
        "sources": [{
            "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
            "entries": [{
                "key": "credits", "label": "Credits", "at": [], "is_required": true,
                "window": {"type": "unknown"},
                "measurement": {"type": "counter",
                                "value": {"field": {"base": "entry", "path": ["used"],
                                                    "is_required": false},
                                          "encoding": "number"},
                                "limit": {"field": {"base": "entry", "path": ["limit"],
                                                    "is_required": false},
                                          "encoding": "number"},
                                "direction": "used", "unit": "credits"},
                "reset": {"type": "unknown"}
            }]
        }]
    }));

    let half_present = extract(&json!({"plan": {"used": 5}}), &optional_counter)
        .expect("a partially absent counter stays unknown");

    assert_eq!(
        entry(&half_present, 0).measurement,
        QuotaMeasurement::Unknown
    );
}

/// Numeric encodings are exact: strict numeric strings reject whitespace and non-numeric
/// text, number-only encodings reject strings, and magnitudes beyond the shared safe-integer
/// domain fail.
#[test]
fn numeric_encodings_are_strict() {
    let fixture_with = |encoding: &str| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "credits", "label": "Credits", "at": [], "is_required": true,
                    "window": {"type": "unknown"},
                    "measurement": {"type": "remaining",
                                    "value": {"field": {"base": "entry", "path": ["left"],
                                                        "is_required": true},
                                              "encoding": encoding},
                                    "unit": "credits"},
                    "reset": {"type": "unknown"}
                }]
            }]
        }))
    };

    let either = fixture_with("number_or_numeric_string");

    for (payload, expected) in [
        (json!({"plan": {"left": 42}}), "remaining:42credits"),
        (json!({"plan": {"left": "42.5"}}), "remaining:42.5credits"),
    ] {
        let entries = extract(&payload, &either).expect("both spellings read");

        assert_eq!(
            summary(entry(&entries, 0)).split(" | ").nth(1),
            Some(expected),
            "the value reads verbatim: {}",
            summary(entry(&entries, 0))
        );
    }

    let string_only = fixture_with("numeric_string");

    let entries = extract(&json!({"plan": {"left": "100"}}), &string_only)
        .expect("a strict numeric string reads");

    assert_eq!(
        summary(entry(&entries, 0)).split(" | ").nth(1),
        Some("remaining:100credits")
    );

    for hostile in [
        json!({"plan": {"left": " 100"}}),
        json!({"plan": {"left": "+100"}}),
        json!({"plan": {"left": "soon"}}),
        json!({"plan": {"left": 100}}),
        json!({"plan": {"left": -1}}),
        json!({"plan": {"left": 9_007_199_254_740_992_i64}}),
        json!({"plan": {"left": true}}),
    ] {
        let failed = failure(&string_only, &hostile);

        assert!(
            failed.contains("invalid numeric domain") || failed.contains("wrong declared shape"),
            "`{hostile}` rejected, got: {failed}"
        );
    }

    let number_only = fixture_with("number");

    assert!(
        extract(&json!({"plan": {"left": 100}}), &number_only).is_ok(),
        "a JSON number reads through the number encoding"
    );

    assert!(
        failure(&number_only, &json!({"plan": {"left": "100"}})).contains("invalid numeric domain"),
        "a numeric string fails the number-only encoding"
    );
}

/// Durations: the configured fixed and calendar periods pass through, and a duration field
/// scales its unit into whole seconds — fractional and zero magnitudes fail.
#[test]
fn windows_stay_in_their_declared_units() {
    let window_fixture = |window: Value| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "credits", "label": "Credits", "at": [], "is_required": true,
                    "window": window,
                    "measurement": {"type": "unknown"},
                    "reset": {"type": "unknown"}
                }]
            }]
        }))
    };

    let read = |payload: Value, window: Value| {
        extract(&payload, &window_fixture(window))
            .expect("the window extracts")
            .remove(0)
            .window
    };

    assert_eq!(
        read(
            json!({"plan": {}}),
            json!({"type": "duration", "seconds": 60})
        ),
        QuotaWindow::Duration {
            seconds: SafeDuration::try_from(60).expect("the fixture duration is positive")
        }
    );

    assert!(matches!(
        read(
            json!({"plan": {}}),
            json!({"type": "calendar", "unit": "month"})
        ),
        QuotaWindow::Calendar { .. }
    ));

    let hours = read(
        json!({"plan": {"period": 2}}),
        json!({"type": "duration_field",
               "value": {"field": {"base": "entry", "path": ["period"], "is_required": true},
                         "encoding": "number"},
               "unit": "hours"}),
    );

    assert!(matches!(hours, QuotaWindow::Duration { seconds: _ }));

    let weeks = read(
        json!({"plan": {"period": 1}}),
        json!({"type": "duration_field",
               "value": {"field": {"base": "entry", "path": ["period"], "is_required": true},
                         "encoding": "number"},
               "unit": "weeks"}),
    );

    assert_eq!(
        weeks,
        QuotaWindow::Duration {
            seconds: SafeDuration::try_from(604_800).expect("the fixture duration is positive")
        }
    );

    for hostile in [
        json!({"plan": {"period": 1.5}}),
        json!({"plan": {"period": 0}}),
    ] {
        let failed = failure(
            &window_fixture(json!({"type": "duration_field",
                                   "value": {"field": {"base": "entry", "path": ["period"],
                                                       "is_required": true},
                                             "encoding": "number"},
                                   "unit": "days"})),
            &hostile,
        );

        assert!(
            failed.contains("invalid numeric domain"),
            "`{hostile}` rejected, got: {failed}"
        );
    }

    // An absent duration field reads as the unknown window.
    let unknown = read(
        json!({"plan": {}}),
        json!({"type": "duration_field",
        "value": {"field": {"base": "entry", "path": ["period"], "is_required": false},
                  "encoding": "number"},
        "unit": "days"}),
    );

    assert_eq!(unknown, QuotaWindow::Unknown);
}

/// Reset encodings: Unix seconds and milliseconds scale exactly, RFC 3339 accepts
/// microsecond-precision instants and rejects sub-microsecond digits, and dates keep their
/// spelling without inventing a timezone.
#[test]
fn reset_encodings_stay_exact() {
    let reset_fixture = |encoding: &str| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "object", "key": "plan", "at": ["plan"], "is_required": true,
                "entries": [{
                    "key": "credits", "label": "Credits", "at": [], "is_required": true,
                    "window": {"type": "unknown"},
                    "measurement": {"type": "unknown"},
                    "reset": {"type": "field",
                              "field": {"base": "entry", "path": ["reset_at"], "is_required": true},
                              "encoding": encoding}
                }]
            }]
        }))
    };

    let read = |payload: Value, encoding: &str| {
        extract(&payload, &reset_fixture(encoding))
            .expect("the reset extracts")
            .remove(0)
            .reset
    };

    let seconds = read(json!({"plan": {"reset_at": 1_893_456_000}}), "unix_seconds");

    assert!(matches!(seconds, QuotaReset::At { .. }));

    let millis = read(
        json!({"plan": {"reset_at": 1_893_456_000_999_i64}}),
        "unix_milliseconds",
    );

    assert!(matches!(millis, QuotaReset::At { .. }));

    let instant = read(
        json!({"plan": {"reset_at": "2030-01-01T00:00:00.123456Z"}}),
        "rfc3339",
    );

    assert!(matches!(instant, QuotaReset::At { .. }));

    for hostile in [
        json!({"plan": {"reset_at": "2030-01-01T00:00:00.1234567Z"}}),
        json!({"plan": {"reset_at": "2030-01-01T00:00:00.1234561+00:00"}}),
        json!({"plan": {"reset_at": "2030-01-01"}}),
        json!({"plan": {"reset_at": 1_893_456_000}}),
    ] {
        let failed = failure(&reset_fixture("rfc3339"), &hostile);

        assert!(
            failed.contains("invalid reset date, timestamp, or precision"),
            "`{hostile}` rejected, got: {failed}"
        );
    }

    let date = read(json!({"plan": {"reset_at": "2030-06-15"}}), "date");

    assert_eq!(
        date,
        QuotaReset::Date {
            date: "2030-06-15".to_string()
        }
    );

    // A past reset is a valid observation; only scheduling treats the future specially.
    let past = read(json!({"plan": {"reset_at": 1_000}}), "unix_seconds");

    assert!(matches!(past, QuotaReset::At { .. }));

    for hostile in [
        json!({"plan": {"reset_at": "2030-13-01"}}),
        json!({"plan": {"reset_at": "2030-6-15"}}),
        json!({"plan": {"reset_at": "2030-06-15T00:00:00Z"}}),
    ] {
        let failed = failure(&reset_fixture("date"), &hostile);

        assert!(
            failed.contains("invalid reset date, timestamp, or precision"),
            "`{hostile}` rejected, got: {failed}"
        );
    }
}

/// Routing resolution: model sets read from their configured contexts, sorting and
/// deduplicating; an unresolved required selector invalidates the complete observation.
#[test]
fn routing_selectors_resolve_and_canonicalize() {
    let fixture = array_descriptor(json!([{
        "key": "models",
        "label": "Model set",
        "at": ["models"],
        "is_required": true,
        "window": {"type": "unknown"},
        "measurement": {"type": "unknown"},
        "reset": {"type": "unknown"},
        "routing": {"type": "models", "upstream_ids": [
            {"type": "literal", "value": "m-b"},
            {"type": "field",
             "field": {"base": "item", "path": ["model"], "is_required": true},
             "encoding": "string"},
            {"type": "literal", "value": "m-a"}
        ]}
    }]));

    let payload = json!({
        "data": {"organizations": [{"id": "org-1", "model": "m-a", "models": {}}]}
    });

    let entries = extract(&payload, &fixture).expect("the routing resolves");

    assert_eq!(
        summary(entry(&entries, 0)).split(" | ").nth(2),
        Some("models:[\"m-a\", \"m-b\"]"),
        "{}",
        summary(entry(&entries, 0))
    );

    let unresolved = json!({
        "data": {"organizations": [{"id": "org-1", "models": {}}]}
    });

    let failed = failure(&fixture, &unresolved);

    assert!(failed.contains("required value is absent"), "got: {failed}");
}

/// Identity tuples are structural: two observations colliding on the same tuple fail the
/// complete reading, and every segment carries its declared source.
#[test]
fn duplicate_identity_tuples_fail_the_reading() {
    let fixture = array_descriptor(json!([counter_entry()]));

    let colliding = json!({
        "data": {"organizations": [
            {"id": "org-1", "scope": "team", "credits": {"used": 1, "limit": 2,
                                                         "reset_at": "2030-01-01T00:00:00Z"}},
            {"id": "org-1", "scope": "team", "credits": {"used": 3, "limit": 4,
                                                         "reset_at": "2030-01-01T00:00:00Z"}}
        ]}
    });

    let failed = failure(&fixture, &colliding);

    assert!(
        failed.contains("duplicate quota identity tuple"),
        "got: {failed}"
    );
}

/// Atomicity: an optional absent or null source is omitted, a required one fails, and a
/// present malformed value anywhere fails the complete reading — no partial list escapes.
#[test]
fn extraction_is_atomic_over_requiredness_and_shape() {
    let fixture = array_descriptor(json!([counter_entry()]));

    // An empty array is a well-formed empty reading.
    let empty = extract(&json!({"data": {"organizations": []}}), &fixture)
        .expect("an empty collection extracts no observations");

    assert!(empty.is_empty());

    // A required absent source fails.
    let failed = failure(&fixture, &json!({"data": {}}));

    assert!(failed.contains("required value is absent"), "got: {failed}");

    // A null source reads as absent, so the same failure fires.
    let failed = failure(&fixture, &json!({"data": {"organizations": null}}));

    assert!(failed.contains("required value is absent"), "got: {failed}");

    // A present malformed active field fails the whole reading.
    let malformed = json!({
        "data": {"organizations": [
            {"id": "org-1", "scope": "team", "credits": {"used": "high", "limit": 2,
                                                         "reset_at": "2030-01-01T00:00:00Z"}}
        ]}
    });

    let failed = failure(&fixture, &malformed);

    assert!(failed.contains("invalid numeric domain"), "got: {failed}");

    // A non-object item fails the reading.
    let failed = failure(&fixture, &json!({"data": {"organizations": ["org-1"]}}));

    assert!(failed.contains("wrong declared shape"), "got: {failed}");

    // A non-object root fails the reading.
    let failed = failure(&fixture, &json!([1]));

    assert!(failed.contains("wrong declared shape"), "got: {failed}");

    // An absent optional entry is omitted; an absent required one fails naming the entry.
    let optional_entry = json!([{
        "key": "credits",
        "label": "Monthly credits",
        "at": ["absent"],
        "is_required": false,
        "window": {"type": "unknown"},
        "measurement": {"type": "unknown"},
        "reset": {"type": "unknown"}
    }]);

    let omitted = extract(
        &json!({"data": {"organizations": [{"id": "org-1"}]}}),
        &array_descriptor(optional_entry.clone()),
    )
    .expect("an absent optional entry is omitted");

    assert!(omitted.is_empty());

    let required_entry = json!([{
        "key": "credits",
        "label": "Monthly credits",
        "at": ["absent"],
        "is_required": true,
        "window": {"type": "unknown"},
        "measurement": {"type": "unknown"},
        "reset": {"type": "unknown"}
    }]);

    let failed = failure(
        &array_descriptor(required_entry),
        &json!({"data": {"organizations": [{"id": "org-1"}]}}),
    );

    assert!(
        failed.contains("source Some(\"orgs\")") && failed.contains("entry Some(\"credits\")"),
        "the failure keeps the configured context: {failed}"
    );
}

/// The text encodings read verbatim or by exact integer spelling, and empty text fails.
#[test]
fn text_encodings_read_without_lossy_conversion() {
    let identity_fixture = |encoding: &str| {
        descriptor(json!({
            "request": {"method": "GET",
             "url": [{"type": "literal", "value": "https://up.test/quota"}]},
            "sources": [{
                "type": "array",
                "key": "orgs",
                "at": ["data", "organizations"],
                "is_required": true,
                "identity": [{"type": "field",
                              "field": {"base": "item", "path": ["id"], "is_required": true},
                              "encoding": encoding}],
                "entries": [{
                    "key": "credits", "label": "Credits", "at": [], "is_required": false,
                    "window": {"type": "unknown"},
                    "measurement": {"type": "unknown"},
                    "reset": {"type": "unknown"}
                }]
            }]
        }))
    };

    let integer_identity = extract(
        &json!({"data": {"organizations": [{"id": 42}]}}),
        &identity_fixture("integer"),
    )
    .expect("an integer identity reads by its exact spelling");

    assert_eq!(integer_identity.len(), 1);

    assert_eq!(
        entry(&integer_identity, 0).key,
        vec!["orgs".to_string(), "42".to_string(), "credits".to_string()]
    );

    let float_identity = failure(
        &identity_fixture("integer"),
        &json!({"data": {"organizations": [{"id": 4.5}]}}),
    );

    assert!(
        float_identity.contains("invalid nonempty text encoding"),
        "got: {float_identity}"
    );

    let empty_string = failure(
        &identity_fixture("string"),
        &json!({"data": {"organizations": [{"id": ""}]}}),
    );

    assert!(
        empty_string.contains("invalid nonempty text encoding"),
        "got: {empty_string}"
    );
}

/// The normalized amount domain keeps negative and non-finite magnitudes out.
#[test]
fn amount_domains_stay_finite_and_nonnegative() {
    assert!(Amount::try_from(-0.5).is_err());

    assert!(Amount::try_from(f64::NAN).is_err());

    assert!(Amount::try_from(f64::INFINITY).is_err());

    assert_eq!(
        f64::from(Amount::try_from(0.0).expect("zero is an amount")),
        0.0
    );

    assert!(PositiveAmount::try_from(0.0).is_err());

    assert!(
        PositiveAmount::try_from(1.5).is_ok(),
        "a fractional limit stays positive"
    );
}

/// A true unlimited flag bypasses only the numeric measurement fields — never the scope,
/// window, or reset validation of the same entry.
#[test]
fn unlimited_flags_bypass_measurement_fields_only() {
    let fixture = array_descriptor(json!([{
        "key": "credits",
        "label": "Monthly credits",
        "at": ["credits"],
        "is_required": true,
        "window": {"type": "duration_field",
                   "value": {"field": {"base": "entry", "path": ["period"], "is_required": true},
                             "encoding": "number"},
                   "unit": "days"},
        "measurement": {"type": "counter",
                        "value": {"field": {"base": "entry", "path": ["used"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "limit": {"field": {"base": "entry", "path": ["limit"],
                                            "is_required": true},
                                  "encoding": "number"},
                        "direction": "used", "unit": "credits"},
        "reset": {"type": "field",
                  "field": {"base": "entry", "path": ["reset_at"], "is_required": true},
                  "encoding": "unix_seconds"},
        "is_unlimited": {"base": "entry", "path": ["unlimited"], "is_required": false}
    }]));

    // The unlimited entry carries no numeric measurement fields, but its window and reset
    // facts are present and validated.
    let entries = extract(
        &json!({"data": {"organizations": [
            {"id": "org-1",
             "credits": {"unlimited": true, "period": 30, "reset_at": 1_893_456_000}}
        ]}}),
        &fixture,
    )
    .expect("the unlimited entry extracts");

    assert_eq!(entry(&entries, 0).measurement, QuotaMeasurement::Unlimited);

    assert!(matches!(
        entry(&entries, 0).window,
        QuotaWindow::Duration { .. }
    ));

    assert!(matches!(entry(&entries, 0).reset, QuotaReset::At { .. }));

    // A non-boolean unlimited flag is the wrong declared shape.
    let failed = failure(
        &fixture,
        &json!({"data": {"organizations": [
            {"id": "org-1",
             "credits": {"unlimited": "yes", "period": 30, "reset_at": 1_893_456_000}}
        ]}}),
    );

    assert!(failed.contains("wrong declared shape"), "got: {failed}");

    // The unlimited flag never rescues a malformed window fact.
    let failed = failure(
        &fixture,
        &json!({"data": {"organizations": [
            {"id": "org-1",
             "credits": {"unlimited": true, "period": 1.5, "reset_at": 1_893_456_000}}
        ]}}),
    );

    assert!(failed.contains("invalid numeric domain"), "got: {failed}");
}
