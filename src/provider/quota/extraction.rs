//! Strict atomic extraction of foreign quota JSON into gateway-owned facts.

use super::types::*;
use crate::provider::definition::{quota::*, template::ResponsePath};
use serde_json::Value as Json;
use std::collections::BTreeSet;
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

/// A safe structured extraction failure; no foreign values or response bodies are retained.
#[derive(Debug, Clone, thiserror::Error)]
#[error("quota source {source_key:?}, entry {entry_key:?}: {reason}")]
pub(crate) struct ExtractionError {
    /// Configured source key, absent before source selection.
    pub(crate) source_key: Option<String>,
    /// Configured entry key, absent before entry selection.
    pub(crate) entry_key: Option<String>,
    /// Safe contract failure category.
    pub(crate) reason: ExtractionReason,
}

/// Closed categories of malformed observations.
#[derive(Debug, Clone, Copy, thiserror::Error)]
pub(crate) enum ExtractionReason {
    /// A required source, entry, or fact was absent.
    #[error("required value is absent")]
    Missing,
    /// A present value violated its declared JSON shape.
    #[error("value has the wrong declared shape")]
    Shape,
    /// A numeric fact violated grammar, range, or exactness.
    #[error("invalid numeric domain or encoding")]
    Number,
    /// A reset violated its declared time encoding or precision.
    #[error("invalid reset date, timestamp, or precision")]
    Time,
    /// Two observations resolved to the same identity tuple.
    #[error("duplicate quota identity tuple")]
    Duplicate,
    /// A text fact was empty or had the wrong shape.
    #[error("invalid nonempty text encoding")]
    Text,
}

/// Borrowed contexts for one entry; source metadata is resolved before entry selection.
struct Context<'a> {
    /// The complete response.
    root: &'a Json,
    /// The source item.
    item: &'a Json,
    /// The selected entry object.
    entry: &'a Json,
    /// A map source's member name.
    map_key: Option<&'a str>,
}

/// Extracts every observation or one safe error; no partially successful list escapes.
pub(crate) fn extract(
    root: &Json,
    descriptor: &QuotaDescriptor,
) -> Result<Vec<QuotaEntry>, ExtractionError> {
    let mut result = Vec::new();
    let mut identities = BTreeSet::new();
    if !root.is_object() {
        return Err(error(None, None, ExtractionReason::Shape));
    }
    for source in &descriptor.sources {
        let (key, at, is_required, scope, entries) = match source {
            QuotaSource::Object {
                key,
                at,
                is_required,
                scope,
                entries,
            }
            | QuotaSource::Array {
                key,
                at,
                is_required,
                scope,
                entries,
                ..
            }
            | QuotaSource::Map {
                key,
                at,
                is_required,
                scope,
                entries,
            } => (key, at, *is_required, scope, entries),
        };
        let mut active_entry = None;
        let mut run = || -> Result<Vec<QuotaEntry>, ExtractionReason> {
            let Some(selected) = required(resolve(root, at)?, is_required)? else {
                return Ok(Vec::new());
            };
            let items: Vec<(Option<&str>, &Json)> = match source {
                QuotaSource::Object { .. } if selected.is_object() => vec![(None, selected)],
                QuotaSource::Array { .. } => selected
                    .as_array()
                    .ok_or(ExtractionReason::Shape)?
                    .iter()
                    .map(|item| (None, item))
                    .collect(),
                QuotaSource::Map { .. } => {
                    let map = selected.as_object().ok_or(ExtractionReason::Shape)?;
                    let mut items: Vec<_> = map
                        .iter()
                        .map(|(key, item)| (Some(key.as_str()), item))
                        .collect();
                    items.sort_by_key(|(key, _)| *key);
                    items
                }
                _ => return Err(ExtractionReason::Shape),
            };
            let mut observations = Vec::new();
            for (map_key, item) in items {
                active_entry = None;
                if !item.is_object() {
                    return Err(ExtractionReason::Shape);
                }
                let context = Context {
                    root,
                    item,
                    entry: item,
                    map_key,
                };
                let mut identity = vec![key.clone()];
                match source {
                    QuotaSource::Array {
                        identity: parts, ..
                    } => {
                        for part in parts {
                            identity.push(text(part, &context)?.ok_or(ExtractionReason::Missing)?);
                        }
                    }
                    QuotaSource::Map { .. } => {
                        identity.push(nonempty(map_key.ok_or(ExtractionReason::Text)?)?)
                    }
                    QuotaSource::Object { .. } => {}
                }
                let scope = scope_value(scope.as_ref(), &context)?;
                for configured in entries {
                    active_entry = Some(configured.key.as_str());
                    if let Some(observation) =
                        extract_entry(configured, &context, &identity, &scope)?
                    {
                        observations.push(observation);
                    }
                }
            }
            Ok(observations)
        };
        // Keep configured entry context on failures; no upstream values are retained.
        // Duplicate identities are compared as tuples, never joined strings.
        let observations = run().map_err(|reason| error(Some(key), active_entry, reason))?;
        for observation in observations {
            if !identities.insert(observation.key.clone()) {
                return Err(error(
                    Some(key),
                    observation.key.last().map(String::as_str),
                    ExtractionReason::Duplicate,
                ));
            }
            result.push(observation);
        }
    }
    Ok(result)
}

/// Builds an extraction failure from configured identifiers only.
fn error(source: Option<&str>, entry: Option<&str>, reason: ExtractionReason) -> ExtractionError {
    ExtractionError {
        source_key: source.map(str::to_owned),
        entry_key: entry.map(str::to_owned),
        reason,
    }
}

/// Resolves object-member paths, distinguishing absence from malformed intermediate values.
fn resolve<'a>(root: &'a Json, path: &ResponsePath) -> Result<Option<&'a Json>, ExtractionReason> {
    let mut current = root;
    for member in &path.0 {
        if current.is_null() {
            return Ok(None);
        }
        let object = current.as_object().ok_or(ExtractionReason::Shape)?;
        let Some(value) = object.get(member) else {
            return Ok(None);
        };
        current = value;
    }
    Ok((!current.is_null()).then_some(current))
}

/// Applies requiredness to an already strictly resolved value.
fn required<T>(value: Option<T>, is_required: bool) -> Result<Option<T>, ExtractionReason> {
    if value.is_none() && is_required {
        return Err(ExtractionReason::Missing);
    }
    Ok(value)
}

/// Resolves a scalar from its explicitly declared context.
fn field<'a>(
    selection: &QuotaField,
    context: &Context<'a>,
) -> Result<Option<&'a Json>, ExtractionReason> {
    let base = match selection.base {
        QuotaFieldBase::Root => context.root,
        QuotaFieldBase::Item => context.item,
        QuotaFieldBase::Entry => context.entry,
    };
    required(resolve(base, &selection.path)?, selection.is_required)
}

/// Retains nonempty text verbatim.
fn nonempty(value: &str) -> Result<String, ExtractionReason> {
    if value.is_empty() {
        Err(ExtractionReason::Text)
    } else {
        Ok(value.to_owned())
    }
}

/// Decodes text without lossy numeric conversion.
fn field_text(
    selection: &QuotaField,
    encoding: QuotaTextEncoding,
    context: &Context<'_>,
) -> Result<Option<String>, ExtractionReason> {
    let Some(value) = field(selection, context)? else {
        return Ok(None);
    };
    match (encoding, value) {
        (QuotaTextEncoding::String, Json::String(value)) => nonempty(value).map(Some),
        (QuotaTextEncoding::Integer, Json::Number(value)) if value.is_i64() || value.is_u64() => {
            Ok(Some(value.to_string()))
        }
        _ => Err(ExtractionReason::Text),
    }
}

/// Resolves one array identity segment.
fn text(
    source: &QuotaTextSource,
    context: &Context<'_>,
) -> Result<Option<String>, ExtractionReason> {
    match source {
        QuotaTextSource::Literal { value } => Ok(Some(value.clone())),
        QuotaTextSource::Field { field, encoding } => field_text(field, *encoding, context),
        QuotaTextSource::MapKey => context.map_key.map(nonempty).transpose(),
    }
}

/// Resolves an authored exact model set without widening unresolved selectors.
fn routing_value(
    source: &QuotaRoutingSource,
    context: &Context<'_>,
) -> Result<QuotaRouting, ExtractionReason> {
    match source {
        QuotaRoutingSource::MonitoringOnly => Ok(QuotaRouting::MonitoringOnly),
        QuotaRoutingSource::AllModels => Ok(QuotaRouting::AllModels),
        QuotaRoutingSource::Models { upstream_ids } => {
            let mut models = BTreeSet::new();
            for source in upstream_ids {
                let model = text(source, context)?.ok_or(ExtractionReason::Missing)?;
                if !crate::routing_text::is_routing_safe_token(&model) {
                    return Err(ExtractionReason::Text);
                }
                models.insert(model);
            }
            Ok(QuotaRouting::Models {
                upstream_ids: models.into_iter().collect(),
            })
        }
    }
}

/// Resolves scope independently of an entry's restriction label; an absent scope is the
/// unknown scope.
fn scope_value(
    source: Option<&QuotaTextSource>,
    context: &Context<'_>,
) -> Result<QuotaScope, ExtractionReason> {
    let name = match source {
        Some(source) => text(source, context)?,
        None => None,
    };
    Ok(name.map_or(QuotaScope::Unknown, |name| QuotaScope::Named { name }))
}

/// Decodes exactly the configured numeric union and checks integer precision before conversion.
fn number(
    selection: &QuotaNumberField,
    context: &Context<'_>,
) -> Result<Option<f64>, ExtractionReason> {
    let Some(value) = field(&selection.field, context)? else {
        return Ok(None);
    };
    let decoded;
    let number = match (selection.encoding, value) {
        (
            QuotaNumberEncoding::Number | QuotaNumberEncoding::NumberOrNumericString,
            Json::Number(value),
        ) => value,
        (
            QuotaNumberEncoding::NumericString | QuotaNumberEncoding::NumberOrNumericString,
            Json::String(value),
        ) => {
            if value.trim() != value {
                return Err(ExtractionReason::Number);
            }
            decoded = serde_json::from_str::<serde_json::Number>(value)
                .map_err(|_| ExtractionReason::Number)?;
            &decoded
        }
        _ => return Err(ExtractionReason::Number),
    };
    if number
        .as_u64()
        .is_some_and(|value| value > MAX_SAFE_INTEGER)
        || number
            .as_i64()
            .is_some_and(|value| value.unsigned_abs() > MAX_SAFE_INTEGER)
    {
        return Err(ExtractionReason::Number);
    }
    let value = number.as_f64().ok_or(ExtractionReason::Number)?;
    if !value.is_finite()
        || value < 0.0
        || (value.fract() == 0.0 && value > MAX_SAFE_INTEGER as f64)
    {
        return Err(ExtractionReason::Number);
    }
    Ok(Some(if value == 0.0 { 0.0 } else { value }))
}

/// Converts a normalized nonnegative magnitude into its validated result domain.
fn amount(value: f64) -> Result<Amount, ExtractionReason> {
    Amount::try_from(value).map_err(|_| ExtractionReason::Number)
}

/// Evaluates all active counter fields before deciding whether the measurement is unknown.
fn measurement(
    source: &QuotaMeasurementSource,
    context: &Context<'_>,
) -> Result<QuotaMeasurement, ExtractionReason> {
    Ok(match source {
        QuotaMeasurementSource::Unknown => QuotaMeasurement::Unknown,
        QuotaMeasurementSource::Ratio {
            value,
            scale,
            direction,
        } => match number(value, context)? {
            None => QuotaMeasurement::Unknown,
            Some(value) => {
                let fraction = match scale {
                    QuotaRatioScale::Fraction => value,
                    QuotaRatioScale::Percent => value / 100.0,
                };
                let used = match direction {
                    QuotaDirection::Used => fraction,
                    QuotaDirection::Remaining if fraction <= 1.0 => 1.0 - fraction,
                    QuotaDirection::Remaining => return Err(ExtractionReason::Number),
                };
                QuotaMeasurement::Ratio {
                    used_fraction: amount(used)?,
                }
            }
        },
        QuotaMeasurementSource::Counter {
            value,
            limit,
            direction,
            unit,
        } => {
            let value = number(value, context)?;
            let limit = number(limit, context)?
                .map(PositiveAmount::try_from)
                .transpose()
                .map_err(|_| ExtractionReason::Number)?;
            match (value, limit) {
                (Some(value), Some(limit)) => {
                    let used = match direction {
                        QuotaDirection::Used => value,
                        QuotaDirection::Remaining if value <= f64::from(limit) => {
                            f64::from(limit) - value
                        }
                        QuotaDirection::Remaining => return Err(ExtractionReason::Number),
                    };
                    QuotaMeasurement::Counter {
                        used: amount(used)?,
                        limit,
                        unit: unit.clone(),
                    }
                }
                _ => QuotaMeasurement::Unknown,
            }
        }
        QuotaMeasurementSource::Remaining { value, unit } => match number(value, context)? {
            Some(value) => QuotaMeasurement::Remaining {
                remaining: amount(value)?,
                unit: unit.clone(),
            },
            None => QuotaMeasurement::Unknown,
        },
    })
}

/// Extracts a fixed or calendar period without inventing alignment.
fn window(
    source: &QuotaWindowSource,
    context: &Context<'_>,
) -> Result<QuotaWindow, ExtractionReason> {
    Ok(match source {
        QuotaWindowSource::Unknown => QuotaWindow::Unknown,
        QuotaWindowSource::Duration { seconds } => QuotaWindow::Duration { seconds: *seconds },
        QuotaWindowSource::Calendar { unit } => QuotaWindow::Calendar { unit: *unit },
        QuotaWindowSource::DurationField { value, unit } => match number(value, context)? {
            None => QuotaWindow::Unknown,
            Some(value) => {
                if value.fract() != 0.0 || value == 0.0 {
                    return Err(ExtractionReason::Number);
                }
                let factor = match unit {
                    QuotaDurationUnit::Seconds => 1,
                    QuotaDurationUnit::Minutes => 60,
                    QuotaDurationUnit::Hours => 3600,
                    QuotaDurationUnit::Days => 86400,
                    QuotaDurationUnit::Weeks => 604800,
                };
                let seconds = (value as u64)
                    .checked_mul(factor)
                    .and_then(|value| SafeDuration::try_from(value).ok())
                    .ok_or(ExtractionReason::Number)?;
                QuotaWindow::Duration { seconds }
            }
        },
    })
}

/// Validates an exact calendar date while keeping its timezone unspecified.
fn is_valid_date(value: &str) -> bool {
    value.len() == 10
        && value.bytes().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
        && Date::parse(
            value,
            &time::macros::format_description!("[year]-[month]-[day]"),
        )
        .is_ok()
}

/// Extracts reset metadata, rejecting unsafe or sub-microsecond instants.
fn reset(source: &QuotaResetSource, context: &Context<'_>) -> Result<QuotaReset, ExtractionReason> {
    let QuotaResetSource::Field {
        field: selection,
        encoding,
    } = source
    else {
        return Ok(QuotaReset::Unknown);
    };
    let Some(value) = field(selection, context)? else {
        return Ok(QuotaReset::Unknown);
    };
    let micros = match encoding {
        QuotaResetEncoding::Date => {
            let date = value
                .as_str()
                .filter(|value| is_valid_date(value))
                .ok_or(ExtractionReason::Time)?;
            return Ok(QuotaReset::Date {
                date: date.to_owned(),
            });
        }
        QuotaResetEncoding::Rfc3339 => {
            let date = value.as_str().ok_or(ExtractionReason::Time)?;
            // The time parser discards digits beyond nanoseconds. Inspect the original
            // fraction first so a discarded nonzero digit cannot hide inexact microseconds.
            if date.split_once('.').is_some_and(|(_, fraction)| {
                fraction
                    .bytes()
                    .take_while(u8::is_ascii_digit)
                    .skip(6)
                    .any(|digit| digit != b'0')
            }) {
                return Err(ExtractionReason::Time);
            }
            let instant =
                OffsetDateTime::parse(date, &Rfc3339).map_err(|_| ExtractionReason::Time)?;
            if instant.nanosecond() % 1000 != 0 {
                return Err(ExtractionReason::Time);
            }
            i64::try_from(instant.unix_timestamp_nanos() / 1000)
                .map_err(|_| ExtractionReason::Time)?
        }
        QuotaResetEncoding::UnixSeconds | QuotaResetEncoding::UnixMilliseconds => {
            let value = value.as_i64().ok_or(ExtractionReason::Time)?;
            let factor = match encoding {
                QuotaResetEncoding::UnixSeconds => 1_000_000,
                _ => 1000,
            };
            value.checked_mul(factor).ok_or(ExtractionReason::Time)?
        }
    };
    Ok(QuotaReset::At {
        at: SafeTimestamp::try_from(micros).map_err(|_| ExtractionReason::Time)?,
    })
}

/// Extracts one object entry after its source identity and scope have resolved.
fn extract_entry(
    configured: &QuotaEntryExtraction,
    context: &Context<'_>,
    identity: &[String],
    scope: &QuotaScope,
) -> Result<Option<QuotaEntry>, ExtractionReason> {
    let Some(entry) = required(
        resolve(context.item, &configured.at)?,
        configured.is_required,
    )?
    else {
        return Ok(None);
    };
    if !entry.is_object() {
        return Err(ExtractionReason::Shape);
    }
    let context = Context { entry, ..*context };
    let is_unlimited = match &configured.is_unlimited {
        None => false,
        Some(selection) => match field(selection, &context)? {
            None => false,
            Some(value) => value.as_bool().ok_or(ExtractionReason::Shape)?,
        },
    };
    let measurement = if is_unlimited {
        QuotaMeasurement::Unlimited
    } else {
        measurement(&configured.measurement, &context)?
    };
    let window = window(&configured.window, &context)?;
    let reset = reset(&configured.reset, &context)?;
    let mut key = identity.to_vec();
    key.push(configured.key.clone());
    Ok(Some(QuotaEntry {
        key,
        label: configured.label.clone(),
        scope: scope.clone(),
        routing: routing_value(&configured.routing, &context)?,
        window,
        measurement,
        reset,
    }))
}
