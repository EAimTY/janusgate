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
    compaction::{
        mechanism::{Mechanism, MechanismSupport},
        support_facts::CompactionSupportFacts,
    },
    ir::IrReasoningLevel,
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
pub(crate) async fn run_discovery(
    http: &Client,
    discovery: &Discovery,
    env: &ValueEnv<'_>,
) -> Result<Vec<DiscoveredModel>, Error> {
    let mut request = discovery.request.clone();

    let models = match &discovery.pagination {
        None | Some(Pagination::None) => {
            let body =
                control::execute(http, &request, env, encoding_of(discovery), "upstream").await?;

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
    let encoding = encoding_of(discovery);

    let mut models = Vec::new();

    // `Some` while the last fetched page still reports a continuation: a budget that ends on
    // `Some` fails below instead of delivering a truncated list.
    let mut continuation = None;

    for _ in 0..max_pages.get() {
        let body = control::execute(http, request, env, encoding, "upstream").await?;

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
    ValueEnv::for_discovery(
        provider.credentials.slots(),
        provider.base_url.as_deref(),
        provider.credentials.secret(),
    )
}

/// The response encoding a discovery's responses arrive in: the descriptor declares one encoding
/// for the whole paginated exchange.
fn encoding_of(discovery: &Discovery) -> ResponseEncoding {
    match discovery.extract {
        DiscoveryExtract::Array { .. } | DiscoveryExtract::MapKeys { .. } => {
            // The extraction is JSON-shaped or form-flat per the request's response declaration;
            // discovery responses are JSON documents in every researched shape, and the form
            // encoding is only meaningful for token endpoints.
            ResponseEncoding::Json
        }
    }
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
            // The entry carries no level array: the capability is unknown, not empty.
            None => None,

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
                    if let Some(level) = IrReasoningLevel::from_vocabulary_str(level) {
                        levels.push(level);
                    }
                }

                Some(IrReasoningLevel::canonicalize_levels(levels))
            }
        },

        Some(ThinkingLevelsExtraction::FlaggedKeys {
            object_at,
            flag_field,
        }) => match object_at.resolve(entry) {
            None => None,

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

                    if is_supported && let Some(level) = IrReasoningLevel::from_vocabulary_str(name)
                    {
                        levels.push(level);
                    }
                }

                Some(IrReasoningLevel::canonicalize_levels(levels))
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
    thinking_levels: Option<Vec<IrReasoningLevel>>,

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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::unwrap_used,
        reason = "the fixtures are constructed so each unwrap lands on its documented side"
    )]
    #![expect(
        clippy::panic,
        reason = "a fixture landing on the wrong side of its check is the failure report"
    )]

    use super::{Error, run_discovery};
    use crate::{
        compaction::{
            mechanism::{Mechanism, MechanismSupport},
            support_facts::CompactionSupportFacts,
        },
        ir::IrReasoningLevel,
        provider::definition::{discovery::Discovery, eval::ValueEnv},
        test_support::MockUpstream,
    };
    use serde_json::{Value as Json, json};
    use std::collections::BTreeMap;

    /// The environment one discovery test carries: no slots, the mock base URL, and a static
    /// secret, leaked into `'static` references the descriptor can borrow.
    fn discovery_env(base_url: &str) -> ValueEnv<'static> {
        let base_url: &'static str = Box::leak(base_url.to_string().into_boxed_str());
        let slots: &'static BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));

        ValueEnv::for_discovery(slots, Some(base_url), "secret")
    }

    /// Maps one discovery outcome to its identifiers, in order.
    ///
    /// # Panics
    ///
    /// Panics when the outcome is a failure.
    fn upstream_ids(models: Result<Vec<crate::provider::DiscoveredModel>, Error>) -> Vec<String> {
        models
            .unwrap()
            .into_iter()
            .map(|model| model.upstream_id)
            .collect()
    }

    /// The token-paginated discovery fixture: two pages of one identifier each, the next-page
    /// token under `nextPageToken`, paged back through `pageToken`.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse.
    fn token_discovery(max_pages: u64) -> Discovery {
        serde_json::from_value(json!({
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"]
            },
            "pagination": {
                "type": "token",
                "request_param": "pageToken",
                "response_path": ["nextPageToken"],
                "max_pages": max_pages
            }
        }))
        .unwrap()
    }

    /// The cursor-paginated discovery fixture: pages of one identifier each, the continuation
    /// facts under `has_more`/`last_id`, paged back through `after_id`.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse.
    fn cursor_discovery(max_pages: u64) -> Discovery {
        serde_json::from_value(json!({
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {
                "mode": "array",
                "array_at": ["data"],
                "id_field": ["id"]
            },
            "pagination": {
                "type": "cursor",
                "has_more_path": ["has_more"],
                "last_id_path": ["last_id"],
                "request_param": "after_id",
                "max_pages": max_pages
            }
        }))
        .unwrap()
    }

    /// A page budget that ends while the upstream still reports a next page fails the refresh
    /// with the upstream-error category instead of letting a truncated identifier list reach the
    /// apply step, whose blanket mark would mass-vanish every unlisted row (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the walk fails to diverge from the expectation.
    #[tokio::test]
    async fn token_budget_exhaustion_fails_the_refresh() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "nextPageToken": "page-2"
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "data": [{"id": "model-b"}],
                "nextPageToken": "page-3"
            }))
            .unwrap(),
        ])
        .await;

        let outcome = run_discovery(
            &crate::http_client::control_plane(),
            &token_discovery(2),
            &discovery_env(&mock.base()),
        )
        .await;

        let Err(error) = outcome else {
            panic!("a budget ending on an indicated next page must fail the refresh");
        };

        assert!(matches!(error, Error::Unavailable { .. }));

        assert_eq!(
            error.to_string(),
            "Upstream request failed: upstream reported more pages than the 2-page discovery \
             budget fetches; refusing to truncate the provider's model rows"
        );

        mock.shutdown.send(()).ok();
    }

    /// A present next-page token that is not a usable string is a malformed page, not the end of
    /// the walk: reading it as absent would let the budget guard pass and deliver a truncated
    /// list, so the refresh fails like the cursor mode's missing last identifier. An explicit
    /// null token reads as the absent member and still ends the walk normally.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the walk fails to diverge from the expectation.
    #[tokio::test]
    async fn non_string_next_token_fails_the_refresh() {
        let client = crate::http_client::control_plane();

        // A numeric token on the first page fails the whole refresh.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "nextPageToken": 5
            }))
            .unwrap(),
        ])
        .await;

        let outcome =
            run_discovery(&client, &token_discovery(3), &discovery_env(&mock.base())).await;

        let Err(error) = outcome else {
            panic!("a present non-string next token must fail the refresh");
        };

        assert!(matches!(error, Error::Unavailable { .. }));

        assert_eq!(
            error.to_string(),
            "Upstream request failed: model discovery page carries a malformed next-page token"
        );

        mock.shutdown.send(()).ok();

        // An explicit null token reads as the absent member: the walk ends and the page's
        // identifiers deliver normally.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "nextPageToken": null
            }))
            .unwrap(),
        ])
        .await;

        let ids = upstream_ids(
            run_discovery(&client, &token_discovery(3), &discovery_env(&mock.base())).await,
        );

        assert_eq!(ids, vec!["model-a"]);

        mock.shutdown.send(()).ok();
    }

    /// A cursor upstream whose final budgeted page still reports `has_more` fails the refresh
    /// with the upstream-error category, like the token mode's exhaustion (README §7.1).
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the walk fails to diverge from the expectation.
    #[tokio::test]
    async fn cursor_budget_exhaustion_fails_the_refresh() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "has_more": true,
                "last_id": "model-a"
            }))
            .unwrap(),
        ])
        .await;

        let outcome = run_discovery(
            &crate::http_client::control_plane(),
            &cursor_discovery(1),
            &discovery_env(&mock.base()),
        )
        .await;

        let Err(error) = outcome else {
            panic!("a budget ending on an indicated continuation must fail the refresh");
        };

        assert!(matches!(error, Error::Unavailable { .. }));

        assert_eq!(
            error.to_string(),
            "Upstream request failed: upstream reported more pages than the 1-page discovery \
             budget fetches; refusing to truncate the provider's model rows"
        );

        mock.shutdown.send(()).ok();
    }

    /// A budget that exactly covers the pages still succeeds in both pagination modes: the
    /// exhaustion failure fires only when the final budgeted page itself indicates more data.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the walk diverges from the expectation.
    #[tokio::test]
    async fn budget_covering_exactly_the_pages_succeeds() {
        let client = crate::http_client::control_plane();

        // Token mode: the final budgeted page carries no next-page token.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "nextPageToken": "page-2"
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "data": [{"id": "model-b"}]
            }))
            .unwrap(),
        ])
        .await;

        let ids = upstream_ids(
            run_discovery(&client, &token_discovery(2), &discovery_env(&mock.base())).await,
        );

        assert_eq!(ids, vec!["model-a", "model-b"]);

        mock.shutdown.send(()).ok();

        // Cursor mode: the final budgeted page reports `has_more` false.
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "data": [{"id": "model-a"}],
                "has_more": true,
                "last_id": "model-a"
            }))
            .unwrap(),
            serde_json::to_string(&json!({
                "data": [{"id": "model-b"}],
                "has_more": false
            }))
            .unwrap(),
        ])
        .await;

        let ids = upstream_ids(
            run_discovery(&client, &cursor_discovery(2), &discovery_env(&mock.base())).await,
        );

        assert_eq!(ids, vec!["model-a", "model-b"]);

        mock.shutdown.send(()).ok();
    }

    /// The capability-extraction fixture (README §5.5): one identifier page whose entries may
    /// carry capability facts, extracted through the given `capabilities` descriptor object.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse.
    fn capability_discovery(capabilities: &str) -> Discovery {
        serde_json::from_value(json!({
            "request": {
                "method": "GET",
                "url": [{"type": "base"}, {"type": "literal", "value": "/models"}]
            },
            "extract": {
                "mode": "array",
                "array_at": ["items"],
                "id_field": ["slug"],
                "capabilities": serde_json::from_str::<Json>(capabilities).unwrap()
            }
        }))
        .unwrap()
    }

    /// Verifies the `member_array` level extraction (README §5.5): each array element's string
    /// member names one level, a foreign level — the Codex `ultra` — is silently skipped, the
    /// extracted set canonicalizes into vocabulary order, an absent token path leaves the
    /// capability unknown, and a present integer cap extracts verbatim.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the extraction diverges from the expectation.
    #[tokio::test]
    async fn extracts_member_array_levels_skipping_foreign_ones() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "items": [
                    {
                        "slug": "gpt-5.1",
                        "supported_reasoning_levels": [
                            {"effort": "high"},
                            {"effort": "medium", "description": "the default"},
                            {"effort": "ultra"},
                            {"effort": "high"}
                        ],
                        "context_window": 272_000
                    },
                    {"slug": "bare"}
                ]
            }))
            .unwrap(),
        ])
        .await;

        let models = run_discovery(
            &crate::http_client::control_plane(),
            &capability_discovery(
                r#"{"thinking_levels": {
                        "shape": "member_array",
                        "array_at": ["supported_reasoning_levels"],
                        "level_field": ["effort"]
                    },
                    "max_input_tokens": ["context_window"],
                    "max_output_tokens": ["max_output_tokens"]}"#,
            ),
            &discovery_env(&mock.base()),
        )
        .await
        .unwrap();

        assert_eq!(models.len(), 2);

        #[expect(clippy::expect_used, reason = "the fixture lists the model first")]
        let first = models.first().expect("the first model is listed");

        assert_eq!(first.upstream_id, "gpt-5.1");
        assert_eq!(
            first.thinking_levels,
            Some(vec![IrReasoningLevel::Medium, IrReasoningLevel::High])
        );
        assert_eq!(first.max_input_tokens, Some(272_000));
        assert_eq!(first.max_output_tokens, None);

        // An entry carrying none of the configured paths stays wholly unknown.
        #[expect(clippy::expect_used, reason = "the fixture lists the model second")]
        let second = models.get(1).expect("the second model is listed");

        assert_eq!(second.upstream_id, "bare");
        assert_eq!(second.thinking_levels, None);
        assert_eq!(second.max_input_tokens, None);
        assert_eq!(second.max_output_tokens, None);

        mock.shutdown.send(()).ok();
    }

    /// Verifies the `flagged_keys` level extraction (README §5.5): the Anthropic
    /// `capabilities.effort` object's member names are levels, each member an object carrying
    /// its boolean flag — a false flag and a `null` member (the Anthropic nullable `xhigh`)
    /// read as unsupported, and the aggregate bare-boolean `supported` member participates as
    /// its own flag, a key no vocabulary filter admits — while the object's absent entry-level
    /// sibling leaves the second model's capability unknown.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the extraction diverges from the expectation.
    #[tokio::test]
    async fn extracts_flagged_keys_levels_in_the_anthropic_shape() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "items": [
                    {
                        "slug": "claude-opus",
                        "capabilities": {
                            "effort": {
                                "high": {"supported": true},
                                "low": {"supported": true},
                                "medium": {"supported": false},
                                "max": {"supported": true},
                                "xhigh": null,
                                "supported": true
                            }
                        },
                        "max_input_tokens": 200_000,
                        "max_tokens": 32_000
                    },
                    {"slug": "bare", "capabilities": null}
                ]
            }))
            .unwrap(),
        ])
        .await;

        let models = run_discovery(
            &crate::http_client::control_plane(),
            &capability_discovery(
                r#"{"thinking_levels": {
                        "shape": "flagged_keys",
                        "object_at": ["capabilities", "effort"],
                        "flag_field": ["supported"]
                    }}"#,
            ),
            &discovery_env(&mock.base()),
        )
        .await
        .unwrap();

        assert_eq!(models.len(), 2);

        #[expect(clippy::expect_used, reason = "the fixture lists the model first")]
        let first = models.first().expect("the first model is listed");

        assert_eq!(first.upstream_id, "claude-opus");
        assert_eq!(
            first.thinking_levels,
            Some(vec![
                IrReasoningLevel::Low,
                IrReasoningLevel::High,
                IrReasoningLevel::Max
            ])
        );

        // The token paths are unconfigured in this fixture: both stay unknown.
        assert_eq!(first.max_input_tokens, None);
        assert_eq!(first.max_output_tokens, None);

        // A `null` capabilities object is the absent member: unknown, not empty.
        #[expect(clippy::expect_used, reason = "the fixture lists the model second")]
        let second = models.get(1).expect("the second model is listed");

        assert_eq!(second.thinking_levels, None);

        mock.shutdown.send(()).ok();
    }

    /// Verifies the compaction-flag extraction (README §5.5): a `true` flag extracts the
    /// mechanism's `supported` fact and a `false` flag its `unsupported` fact, while an absent
    /// or `null` object, an absent or `null` flag, and an unconfigured mechanism all leave the
    /// model's compaction facts at `None` — the nothing-known state.
    ///
    /// # Panics
    ///
    /// Panics when the fixture fails to parse or the extraction diverges from the expectation.
    #[tokio::test]
    async fn extracts_compaction_facts_in_the_anthropic_shape() {
        let mock = MockUpstream::start(vec![
            serde_json::to_string(&json!({
                "items": [
                    {
                        "slug": "claude-opus",
                        "capabilities": {
                            "compaction": {"supported": true},
                            "context_management": {
                                "compact_20260112": {"supported": false}
                            }
                        }
                    },
                    {
                        "slug": "null-slots",
                        "capabilities": {
                            "compaction": null,
                            "context_management": {"compact_20260112": null}
                        }
                    },
                    {"slug": "absent"}
                ]
            }))
            .unwrap(),
        ])
        .await;

        let models = run_discovery(
            &crate::http_client::control_plane(),
            &capability_discovery(
                r#"{"compaction": {
                        "messages_on_demand": {
                            "object_at": ["capabilities", "compaction"],
                            "flag_field": ["supported"]
                        },
                        "messages_threshold": {
                            "object_at": ["capabilities", "context_management", "compact_20260112"],
                            "flag_field": ["supported"]
                        }
                    }}"#,
            ),
            &discovery_env(&mock.base()),
        )
        .await
        .unwrap();

        assert_eq!(models.len(), 3);

        #[expect(clippy::expect_used, reason = "the fixture lists the model first")]
        let first = models.first().expect("the first model is listed");

        assert_eq!(first.upstream_id, "claude-opus");
        assert_eq!(
            first.compaction,
            CompactionSupportFacts::from_pairs([
                (
                    Mechanism::MessagesOnDemandCompaction,
                    MechanismSupport::Supported
                ),
                (
                    Mechanism::MessagesThresholdCompaction,
                    MechanismSupport::Unsupported,
                ),
            ]),
        );

        // Explicit `null` slots read as the absent member: unknown, not unsupported.
        #[expect(clippy::expect_used, reason = "the fixture lists the model second")]
        let second = models.get(1).expect("the second model is listed");

        assert_eq!(second.upstream_id, "null-slots");
        assert_eq!(second.compaction, None);

        // An entry carrying none of the configured paths stays wholly unknown.
        #[expect(clippy::expect_used, reason = "the fixture lists the model third")]
        let third = models.get(2).expect("the third model is listed");

        assert_eq!(third.upstream_id, "absent");
        assert_eq!(third.compaction, None);

        mock.shutdown.send(()).ok();
    }

    /// Verifies the capability strictness (README §5.5): a present value of the wrong shape — a
    /// level element without its string member, a flag member carrying no boolean at the
    /// configured flag path, a compaction object that is not an object, a compaction flag that
    /// is not a boolean, a non-integer token cap, a non-positive token cap — fails the refresh
    /// as an upstream error before any storage write.
    ///
    /// # Panics
    ///
    /// Panics when a fixture unexpectedly succeeds or lands off its documented side.
    #[tokio::test]
    async fn malformed_capability_facts_fail_the_refresh() {
        let client = crate::http_client::control_plane();

        // (capabilities descriptor object, one-entry payload)
        let cases: [(Json, Json); 6] = [
            (
                json!({
                    "thinking_levels": {
                        "shape": "member_array",
                        "array_at": ["levels"],
                        "level_field": ["effort"]
                    }
                }),
                json!({"slug": "m", "levels": [{"description": "no effort member"}]}),
            ),
            (
                json!({
                    "thinking_levels": {
                        "shape": "flagged_keys",
                        "object_at": ["effort"],
                        "flag_field": ["supported"]
                    }
                }),
                json!({"slug": "m", "effort": {"high": {"enabled": true}}}),
            ),
            (
                json!({
                    "thinking_levels": {
                        "shape": "flagged_keys",
                        "object_at": ["effort"],
                        "flag_field": ["supported"]
                    },
                    "max_input_tokens": ["context_window"]
                }),
                json!({"slug": "m", "effort": {}, "context_window": "272000"}),
            ),
            (
                json!({
                    "thinking_levels": {
                        "shape": "flagged_keys",
                        "object_at": ["effort"],
                        "flag_field": ["supported"]
                    },
                    "max_input_tokens": ["context_window"]
                }),
                json!({"slug": "m", "effort": {}, "context_window": 0}),
            ),
            (
                json!({
                    "compaction": {
                        "messages_on_demand": {
                            "object_at": ["capabilities", "compaction"],
                            "flag_field": ["supported"]
                        }
                    }
                }),
                json!({"slug": "m", "capabilities": {"compaction": "yes"}}),
            ),
            (
                json!({
                    "compaction": {
                        "messages_on_demand": {
                            "object_at": ["capabilities", "compaction"],
                            "flag_field": ["supported"]
                        }
                    }
                }),
                json!({"slug": "m", "capabilities": {"compaction": {"supported": "yes"}}}),
            ),
        ];

        for (index, (capabilities, entry)) in cases.into_iter().enumerate() {
            let mock = MockUpstream::start(vec![
                serde_json::to_string(&json!({"items": [entry]})).unwrap(),
            ])
            .await;

            let descriptor = capability_discovery(&capabilities.to_string());

            let outcome = run_discovery(&client, &descriptor, &discovery_env(&mock.base())).await;

            let Err(error) = outcome else {
                panic!("case {index}: the malformed fact must fail the refresh");
            };

            assert!(matches!(error, Error::Unavailable { .. }), "case {index}");

            mock.shutdown.send(()).ok();
        }
    }
}
