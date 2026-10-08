//! Upstream model discovery, driven entirely by the provider definition's discovery descriptor
//! (README §5.5).
//!
//! Discovery configures a request, a response extraction, and a pagination loop; it discovers
//! upstream model identifiers and never assigns protocol capabilities. A discovered model that
//! does not support its definition's format produces the upstream error when called. The
//! existing `discovered`/`vanished`/`manual` state machine is untouched: an empty or failed
//! upstream list never mass-vanishes existing rows, because a failed refresh is rejected before
//! the storage layer is reached. Failures speak the provider control engine's error vocabulary
//! ([`Error`]) — the two engines reject on the same upstream surface and fail through the
//! same token refresh.

use super::{Error, Provider, control, oauth};
use crate::{
    compaction_model::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    provider::{
        DiscoveredModel,
        definition::{
            discovery as descriptor,
            discovery::{
                CapabilityExtraction, CompactionCapabilityExtraction, Discovery, DiscoveryExtract,
                Pagination, ThinkingLevelsExtraction,
            },
            eval::ValueEnv,
            template::{ControlRequest, ResponseEncoding, ResponsePath},
        },
    },
    vocabulary::ReasoningLevel,
};
use reqwest::Client;
use serde_json::Value as Json;
use std::num::NonZeroU64;

/// Fetches the upstream models — identifiers with their extracted capability facts — of a
/// provider through its definition's discovery descriptor, refreshing the access token first
/// when a known expiry requires it.
///
/// Failures share the provider control engine's vocabulary ([`Error`]): discovery and the token
/// stages reject on the same upstream surface, and the vanish race inside the token refresh —
/// the provider row deleted mid-refresh — classifies as a 404 `not_found`, consistent with the
/// reauthorize surface's vanished-provider precedent.
///
/// # Errors
///
/// Returns [`Error`] when the token refresh fails, the upstream call fails, the payload
/// does not carry the configured shape, or a discovered identifier is not routing-safe.
pub(crate) async fn discover_upstream_models(
    database: &crate::database::Database,
    http: &Client,
    provider: &Provider,
) -> Result<Vec<DiscoveredModel>, Error> {
    let provider = oauth::ensure_fresh_token(database, http, provider).await?;

    let Some(discovery) = &provider.definition.payload.discovery else {
        return Err(Error::Internal {
            message: format!(
                "provider `{}` carries no discovery configuration",
                provider.name
            ),
        });
    };

    let env = discovery_env(&provider);

    run_discovery(http, discovery, &env).await
}

/// Runs one discovery descriptor to completion against its environment: the request, the
/// extraction, and the pagination loop.
///
/// # Errors
///
/// Returns [`Error`] per the discovery failure contract: upstream rejections and
/// unrecognizable payloads are upstream failures, and only a violated gateway invariant is
/// internal.
async fn run_discovery(
    http: &Client,
    discovery: &Discovery,
    env: &ValueEnv<'_>,
) -> Result<Vec<DiscoveredModel>, Error> {
    let mut request = discovery.request.clone();

    let models = match &discovery.pagination {
        None | Some(Pagination::None) => {
            // Discovery responses are JSON documents in every researched shape; the request
            // carries no response declaration of its own, so the encoding is fixed here.
            let body =
                control::execute(http, &request, env, ResponseEncoding::Json, "upstream").await?;

            extract_models(&body, &discovery.extract)?
        }

        Some(Pagination::Token {
            request_param,
            response_path,
            max_pages,
        }) => {
            walk_pages(
                http,
                &mut request,
                env,
                discovery,
                request_param,
                *max_pages,
                |body| {
                    let token_value = value_at(body, response_path);

                    // A present, non-null token that is not a usable string is a malformed
                    // page, not the end of the walk: reading it as absent would pass the
                    // budget check and deliver a truncated list. The failure mirrors the
                    // cursor mode's missing-last-identifier guard.
                    if token_value
                        .is_some_and(|value| !matches!(value, Json::String(_) | Json::Null))
                    {
                        return Err(Error::Unavailable {
                            detail: "model discovery page carries a malformed next-page token"
                                .to_string(),
                        });
                    }

                    Ok(token_value
                        .and_then(Json::as_str)
                        .filter(|token| !token.is_empty())
                        .map(str::to_string))
                },
            )
            .await?
        }

        Some(Pagination::Cursor {
            has_more_path,
            last_id_path,
            request_param,
            max_pages,
        }) => {
            walk_pages(
                http,
                &mut request,
                env,
                discovery,
                request_param,
                *max_pages,
                |body| {
                    let has_more =
                        value_at(body, has_more_path).is_some_and(control::is_truthy_scalar);

                    if !has_more {
                        return Ok(None);
                    }

                    let Some(last_id) = value_at(body, last_id_path)
                        .and_then(Json::as_str)
                        .map(str::to_string)
                    else {
                        return Err(Error::Unavailable {
                            detail: "model discovery page lacks its last identifier".to_string(),
                        });
                    };

                    Ok(Some(last_id))
                },
            )
            .await?
        }
    };

    // Every discovered identifier is validated before any row is written (README §7): one
    // unroutable identifier fails the whole refresh. The routing-token composition — routing-safe
    // text without `/` or `:` — is the ingestion boundary of the shared text contract.
    if let Some(invalid) = models
        .iter()
        .find(|model| !crate::routing_text::is_routing_safe_token(&model.upstream_id))
    {
        return Err(Error::Unavailable {
            detail: format!(
                "model discovery reported the unroutable identifier `{}`",
                invalid.upstream_id
            ),
        });
    }

    Ok(models)
}

/// Walks one paginated discovery against its page budget: each iteration fetches one page
/// through the control engine, extracts its identifiers, and asks the mode's continuation
/// reader for the next request-param value — `None` ends the walk. A budget that ends while
/// the last fetched page still reports a continuation fails the refresh instead of delivering
/// a truncated list (README §5.5).
///
/// # Errors
///
/// Returns [`Error`] when a page fetch or extraction fails, when the continuation reader
/// rejects a malformed page, or when the budget ends on an indicated continuation.
async fn walk_pages(
    http: &Client,
    request: &mut ControlRequest,
    env: &ValueEnv<'_>,
    discovery: &Discovery,
    request_param: &str,
    max_pages: NonZeroU64,
    mut continuation_of: impl FnMut(&control::DecodedBody) -> Result<Option<String>, Error>,
) -> Result<Vec<DiscoveredModel>, Error> {
    let mut models = Vec::new();

    // `Some` while the last fetched page still reports a continuation: a budget that ends on
    // `Some` fails below instead of delivering a truncated list.
    let mut continuation = None;

    for _ in 0..max_pages.get() {
        // Discovery responses are JSON documents in every researched shape; the request
        // carries no response declaration of its own, so the encoding is fixed here.
        let body = control::execute(http, request, env, ResponseEncoding::Json, "upstream").await?;

        models.extend(extract_models(&body, &discovery.extract)?);

        continuation = continuation_of(&body)?;

        let Some(param_value) = &continuation else {
            break;
        };

        set_query_param(request, request_param, param_value);
    }

    if continuation.is_some() {
        return Err(page_budget_exhausted(max_pages));
    }

    Ok(models)
}

/// The environment one discovery request carries: the current secret, the stored slots, and the
/// stored base URL.
fn discovery_env(provider: &Provider) -> ValueEnv<'_> {
    ValueEnv::for_stored_credential(
        provider.credentials.slots(),
        provider.base_url.as_deref(),
        provider.credentials.secret(),
    )
}

/// The failure for a page budget that ended while the upstream still reported more data: a
/// truncated identifier list would reach the apply step, whose blanket mark suspends every row
/// the truncated list fails to re-state, so the refresh fails here — before any storage write —
/// like every other upstream anomaly (README §7.1).
fn page_budget_exhausted(max_pages: NonZeroU64) -> Error {
    Error::Unavailable {
        detail: format!(
            "upstream reported more pages than the {max_pages}-page discovery budget fetches; \
             refusing to truncate the provider's model rows"
        ),
    }
}

/// Reads the JSON value one member chain addresses in a decoded body, or `None` when the body is
/// not JSON or any member is missing.
fn value_at<'a>(
    body: &'a control::DecodedBody,
    path: &crate::provider::definition::template::ResponsePath,
) -> Option<&'a Json> {
    let control::DecodedBody::Json(json) = body else {
        return None;
    };

    path.resolve(json)
}

/// Appends or replaces one query parameter of the paginated request.
fn set_query_param(request: &mut ControlRequest, name: &str, value: &str) {
    if let Some(existing) = request.query.iter_mut().find(|row| row.name == name) {
        existing.value = crate::provider::definition::template::StringExpr::literal(value);
    } else {
        request
            .query
            .push(crate::provider::definition::template::ParamRow {
                name: name.to_string(),
                value: crate::provider::definition::template::StringExpr::literal(value),
            });
    }
}

/// Extracts one page's models per the discovery extraction shape: each entry's identifier with
/// its per-entry capability facts when the descriptor declares an extraction (README §5.5).
///
/// # Errors
///
/// Returns [`Error`] when the payload lacks the configured shape or an entry carries a present
/// capability value of the wrong shape.
fn extract_models(
    body: &control::DecodedBody,
    extract: &descriptor::DiscoveryExtract,
) -> Result<Vec<DiscoveredModel>, Error> {
    let control::DecodedBody::Json(json) = body else {
        return Err(Error::Unavailable {
            detail: "model discovery response must be a JSON document".to_string(),
        });
    };

    match extract {
        DiscoveryExtract::Array {
            array_at,
            id_field,
            strip_prefix,
            capabilities,
        } => {
            let Some(array) = array_at.resolve(json).and_then(Json::as_array) else {
                return Err(Error::Unavailable {
                    detail: "model discovery payload lacks the configured array".to_string(),
                });
            };

            array
                .iter()
                .map(|entry| {
                    let id = id_field
                        .resolve(entry)
                        .and_then(Json::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| Error::Unavailable {
                            detail: "model discovery entry lacks its identifier".to_string(),
                        })?;

                    let upstream_id = match strip_prefix {
                        Some(prefix) => id.strip_prefix(prefix).map(str::to_string).unwrap_or(id),
                        None => id,
                    };

                    let capabilities = extract_capabilities(entry, capabilities.as_ref())?;

                    Ok(DiscoveredModel {
                        upstream_id,
                        thinking_levels: capabilities.thinking_levels,
                        max_input_tokens: capabilities.max_input_tokens,
                        max_output_tokens: capabilities.max_output_tokens,
                        compaction: capabilities.compaction,
                    })
                })
                .collect()
        }

        DiscoveryExtract::MapKeys { at } => {
            let Some(object) = at.resolve(json).and_then(Json::as_object) else {
                return Err(Error::Unavailable {
                    detail: "model discovery payload lacks the configured object".to_string(),
                });
            };

            // The map-keys shape carries no per-entry object: every capability stays unknown.
            Ok(object
                .keys()
                .map(|upstream_id| DiscoveredModel {
                    upstream_id: upstream_id.clone(),
                    thinking_levels: None,
                    max_input_tokens: None,
                    max_output_tokens: None,
                    compaction: None,
                })
                .collect())
        }
    }
}

/// Extracts one entry's capability facts (README §5.5). A descriptor without a capability
/// extraction — or an entry whose configured path is absent or `null` — yields the unknown
/// state (`None`); a present value of the wrong shape fails the refresh; a level string outside
/// the closed vocabulary is silently skipped (the declared set is the vocabulary-restricted
/// support set, so a value the gateway would itself reject never becomes a declaration); every
/// extracted set canonicalizes — deduplicated, vocabulary-ordered.
///
/// # Errors
///
/// Returns [`Error`] as the upstream-error category when a present value has the wrong shape, a
/// compaction flag is not a boolean, or a token cap is not a positive integer.
fn extract_capabilities(
    entry: &Json,
    capabilities: Option<&CapabilityExtraction>,
) -> Result<EntryCapabilities, Error> {
    let Some(capabilities) = capabilities else {
        return Ok(EntryCapabilities::unknown());
    };

    let thinking_levels = match &capabilities.thinking_levels {
        Some(ThinkingLevelsExtraction::MemberArray {
            array_at,
            level_field,
        }) => match array_at.resolve(entry) {
            // An absent path or an explicit null at the addressed member is the unknown state:
            // the entry carries no level array, not an empty one (README §5.5).
            None | Some(Json::Null) => None,

            Some(value) => {
                let Some(elements) = value.as_array() else {
                    return Err(Error::Unavailable {
                        detail: "model discovery entry carries a malformed thinking-level array"
                            .to_string(),
                    });
                };

                let mut levels = Vec::with_capacity(elements.len());

                for element in elements {
                    let Some(level) = level_field.resolve(element).and_then(Json::as_str) else {
                        return Err(Error::Unavailable {
                            detail: "model discovery entry carries a thinking-level element \
                             without its level member"
                                .to_string(),
                        });
                    };

                    // A foreign level is silently skipped: the declaration covers the closed
                    // vocabulary alone (README §5.5).
                    if let Some(level) = ReasoningLevel::from_vocabulary_str(level) {
                        levels.push(level);
                    }
                }

                Some(ReasoningLevel::canonicalize_levels(levels))
            }
        },

        Some(ThinkingLevelsExtraction::FlaggedKeys {
            object_at,
            flag_field,
        }) => match object_at.resolve(entry) {
            // An absent path or an explicit null at the addressed member is the unknown state,
            // like the member-array arm (README §5.5).
            None | Some(Json::Null) => None,

            Some(value) => {
                let Some(members) = value.as_object() else {
                    return Err(Error::Unavailable {
                        detail: "model discovery entry carries a malformed thinking-level object"
                            .to_string(),
                    });
                };

                let mut levels = Vec::new();

                for (name, member) in members {
                    // The member's flag: a bare boolean stands as its own flag — the aggregate
                    // `supported` member of the Anthropic shape, a key no vocabulary filter
                    // admits — an object carries it at `flag_field`, and a `null` member (the
                    // Anthropic nullable `xhigh`) reads as absent.
                    let is_supported = match member {
                        Json::Bool(flag) => *flag,

                        Json::Object(_) => match flag_field.resolve(member) {
                            Some(Json::Bool(flag)) => *flag,

                            _ => {
                                return Err(Error::Unavailable {
                                    detail: "model discovery thinking-level member lacks \
                                     its flag"
                                        .to_string(),
                                });
                            }
                        },

                        Json::Null => false,

                        _ => {
                            return Err(Error::Unavailable {
                                detail: "model discovery thinking-level member carries a \
                                 malformed flag"
                                    .to_string(),
                            });
                        }
                    };

                    if is_supported && let Some(level) = ReasoningLevel::from_vocabulary_str(name) {
                        levels.push(level);
                    }
                }

                Some(ReasoningLevel::canonicalize_levels(levels))
            }
        },

        None => None,
    };

    let max_input_tokens = extract_token_cap(entry, &capabilities.max_input_tokens)?;
    let max_output_tokens = extract_token_cap(entry, &capabilities.max_output_tokens)?;

    let compaction = extract_compaction_facts(entry, capabilities.compaction.as_ref())?;

    Ok(EntryCapabilities {
        thinking_levels,
        max_input_tokens,
        max_output_tokens,
        compaction,
    })
}

/// One entry's extracted capability facts (README §5.5), the pre-insertion shape of the
/// capability columns: every fact `None` — *unknown* — until a configured extraction reads it
/// off the entry.
struct EntryCapabilities {
    /// Extracted thinking levels; `None` when unknown.
    thinking_levels: Option<Vec<ReasoningLevel>>,

    /// Extracted maximum input tokens; `None` when unknown.
    max_input_tokens: Option<i64>,

    /// Extracted maximum output tokens; `None` when unknown.
    max_output_tokens: Option<i64>,

    /// Extracted compaction support facts — only known members; `None` when the entry carries
    /// none.
    compaction: Option<CompactionSupportFacts>,
}

impl EntryCapabilities {
    /// The all-unknown state: no configured extraction, or an entry carrying none of the
    /// configured paths.
    fn unknown() -> Self {
        Self {
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }
    }
}

/// Extracts one entry's token cap: an unconfigured path, or a configured path absent or `null`
/// on the entry, is the unknown state; a positive integer is the value; any other present value
/// — a non-integer or a non-positive integer — fails the refresh as a malformed upstream fact.
///
/// # Errors
///
/// Returns [`Error`] as the upstream-error category for a malformed or non-positive cap.
fn extract_token_cap(entry: &Json, path: &Option<ResponsePath>) -> Result<Option<i64>, Error> {
    let Some(path) = path else {
        return Ok(None);
    };

    let Some(value) = path.resolve(entry) else {
        return Ok(None);
    };

    // An explicit null at the addressed member reads as the absent member — the unknown
    // state, the same fold the level-array and compaction extractions give a nulled slot.
    if value.is_null() {
        return Ok(None);
    }

    let Some(cap) = value.as_i64() else {
        return Err(Error::Unavailable {
            detail: "model discovery entry carries a malformed token cap".to_string(),
        });
    };

    if cap <= 0 {
        return Err(Error::Unavailable {
            detail: "model discovery entry carries a non-positive token cap".to_string(),
        });
    }

    Ok(Some(cap))
}

/// Extracts one entry's compaction support facts (README §5.5): each configured mechanism reads
/// one boolean flag out of the object its `object_at` path names — `true` extracts the
/// mechanism's `supported` fact, `false` its `unsupported` fact, and an absent-or-null object or
/// flag leaves the mechanism unknown. A present object or flag of the wrong shape — a
/// non-object where an object is addressed, a non-boolean where a flag is addressed — fails the
/// refresh as a malformed upstream fact, like every other present-wrong-shape capability value.
///
/// # Errors
///
/// Returns [`Error`] as the upstream-error category for a malformed compaction object or flag.
fn extract_compaction_facts(
    entry: &Json,
    compaction: Option<&CompactionCapabilityExtraction>,
) -> Result<Option<CompactionSupportFacts>, Error> {
    let Some(compaction) = compaction else {
        return Ok(None);
    };

    let mut pairs = Vec::new();

    for (mechanism, extraction) in [
        (
            Mechanism::MessagesOnDemandCompaction,
            compaction.messages_on_demand.as_ref(),
        ),
        (
            Mechanism::MessagesThresholdCompaction,
            compaction.messages_threshold.as_ref(),
        ),
    ] {
        let Some(extraction) = extraction else {
            continue;
        };

        // An absent path is the unknown state; an explicit null object reads the same way, so
        // an upstream that nulls a capability slot for a model extracts nothing for it.
        let Some(object) = extraction.object_at.resolve(entry) else {
            continue;
        };

        if object.is_null() {
            continue;
        }

        if object.as_object().is_none() {
            return Err(Error::Unavailable {
                detail: "model discovery entry carries a malformed compaction object".to_string(),
            });
        }

        match extraction.flag_field.resolve(object) {
            None | Some(Json::Null) => continue,

            Some(Json::Bool(flag)) => pairs.push((
                mechanism,
                if *flag {
                    MechanismSupport::Supported
                } else {
                    MechanismSupport::Unsupported
                },
            )),

            Some(_) => {
                return Err(Error::Unavailable {
                    detail: "model discovery entry carries a malformed compaction flag".to_string(),
                });
            }
        }
    }

    Ok(CompactionSupportFacts::from_pairs(pairs))
}
