//! Management-boundary structural validation of quota extraction descriptors.
//!
//! The nonempty-collection contracts — the descriptor's sources, an array source's identity, a
//! source's entries, and a routing selector's model set — belong to the carrying `Vec1` types:
//! their empty lists are parse rejections, not validation rejections.

use super::{ValidationError, quota::*, template::ResponsePath};
use std::collections::BTreeSet;

/// Validates contexts, uniqueness, nonempty configuration, and stable array identity.
pub(super) fn validate(descriptor: &QuotaDescriptor) -> Result<(), ValidationError> {
    let mut sources = BTreeSet::new();
    for source in &descriptor.sources {
        let (key, at, scope, entries, is_map) = match source {
            QuotaSource::Object {
                key,
                at,
                scope,
                entries,
                ..
            }
            | QuotaSource::Array {
                key,
                at,
                scope,
                entries,
                ..
            } => (key, at, scope, entries, false),
            QuotaSource::Map {
                key,
                at,
                scope,
                entries,
                ..
            } => (key, at, scope, entries, true),
        };
        nonempty(key)?;
        path(at)?;
        if !sources.insert(key) {
            return Err(invalid("quota source keys must be unique"));
        }
        if let Some(scope) = scope {
            match scope {
                QuotaTextSource::Literal { value } => nonempty(value)?,
                QuotaTextSource::Field {
                    field: selection, ..
                } => field(selection, false)?,
                QuotaTextSource::MapKey if is_map => {}
                QuotaTextSource::MapKey => {
                    return Err(invalid("quota map_key requires a map source"));
                }
            }
        }
        if let QuotaSource::Array { identity, .. } = source {
            let mut has_item_field = false;
            for part in identity {
                match part {
                    QuotaTextSource::Literal { value } => nonempty(value)?,
                    QuotaTextSource::MapKey => {
                        return Err(invalid("quota array identity cannot use map_key"));
                    }
                    QuotaTextSource::Field {
                        field: selection, ..
                    } => {
                        field(selection, false)?;
                        if !selection.is_required {
                            return Err(invalid("quota array identity fields must be required"));
                        }
                        has_item_field |= selection.base == QuotaFieldBase::Item;
                    }
                }
            }
            if !has_item_field {
                return Err(invalid("quota array identity requires an item field"));
            }
        }
        let mut keys = BTreeSet::new();
        for entry in entries {
            nonempty(&entry.key)?;
            nonempty(&entry.label)?;
            path(&entry.at)?;
            if !keys.insert(&entry.key) {
                return Err(invalid("quota entry keys must be unique within a source"));
            }
            if let QuotaRoutingSource::Models { upstream_ids } = &entry.routing {
                for source in upstream_ids {
                    match source {
                        QuotaTextSource::Literal { value } => {
                            if !crate::routing_text::is_routing_safe_token(value) {
                                return Err(invalid(
                                    "quota routing model IDs must be exact routing tokens",
                                ));
                            }
                        }
                        QuotaTextSource::Field {
                            field: selection, ..
                        } => {
                            field(selection, true)?;
                            if !selection.is_required {
                                return Err(invalid("quota routing model fields must be required"));
                            }
                        }
                        QuotaTextSource::MapKey if is_map => {}
                        QuotaTextSource::MapKey => {
                            return Err(invalid("quota routing map_key requires a map source"));
                        }
                    }
                }
            }
            if let Some(selection) = &entry.is_unlimited {
                field(selection, true)?;
            }
            if let QuotaWindowSource::DurationField { value, .. } = &entry.window {
                field(&value.field, true)?;
            }
            match &entry.measurement {
                QuotaMeasurementSource::Unknown => {}
                QuotaMeasurementSource::Ratio { value, .. } => field(&value.field, true)?,
                QuotaMeasurementSource::Counter {
                    value, limit, unit, ..
                } => {
                    field(&value.field, true)?;
                    field(&limit.field, true)?;
                    nonempty(unit)?;
                }
                QuotaMeasurementSource::Remaining { value, unit } => {
                    field(&value.field, true)?;
                    nonempty(unit)?;
                }
            }
            if let QuotaResetSource::Field {
                field: selection, ..
            } = &entry.reset
            {
                field(selection, true)?;
            }
        }
    }
    Ok(())
}

/// Rejects an empty configured text value without rewriting it.
fn nonempty(value: &str) -> Result<(), ValidationError> {
    if value.is_empty() {
        Err(invalid("quota configured text must not be empty"))
    } else {
        Ok(())
    }
}

/// Validates every object-member name while permitting the current-object path.
fn path(value: &ResponsePath) -> Result<(), ValidationError> {
    for part in &value.0 {
        nonempty(part)?;
    }
    Ok(())
}

/// Checks whether the scalar's context is available at its extraction stage.
fn field(value: &QuotaField, can_use_entry: bool) -> Result<(), ValidationError> {
    if !can_use_entry && value.base == QuotaFieldBase::Entry {
        return Err(invalid(
            "quota source identity/scope cannot reference entry",
        ));
    }
    path(&value.path)
}

/// Builds a safe write-boundary rejection.
fn invalid(message: &str) -> ValidationError {
    ValidationError::new(message)
}
