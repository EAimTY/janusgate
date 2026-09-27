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
use crate::provider::definition::{
    discovery as descriptor,
    discovery::{Discovery, DiscoveryExtract, Pagination},
    eval::ValueEnv,
    template::{ControlRequest, ResponseEncoding},
};
use reqwest::Client;
use serde_json::Value as Json;
use std::num::NonZeroU64;

/// Fetches the upstream model identifiers (`upstream_id` values) of a provider through its
/// definition's discovery descriptor, refreshing the access token first when a known expiry
/// requires it.
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
) -> Result<Vec<String>, Error> {
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
) -> Result<Vec<String>, Error> {
    let mut request = discovery.request.clone();

    let ids = match &discovery.pagination {
        None | Some(Pagination::None) => {
            let body =
                control::execute(http, &request, env, encoding_of(discovery), "upstream").await?;

            extract_ids(&body, &discovery.extract)?
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
    if let Some(invalid) = ids
        .iter()
        .find(|id| !crate::routing_text::is_routing_safe_token(id))
    {
        return Err(Error::Unavailable {
            detail: format!("model discovery reported the unroutable identifier `{invalid}`"),
        });
    }

    Ok(ids)
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
) -> Result<Vec<String>, Error> {
    let encoding = encoding_of(discovery);

    let mut ids = Vec::new();

    // `Some` while the last fetched page still reports a continuation: a budget that ends on
    // `Some` fails below instead of delivering a truncated list.
    let mut continuation = None;

    for _ in 0..max_pages.get() {
        let body = control::execute(http, request, env, encoding, "upstream").await?;

        ids.extend(extract_ids(&body, &discovery.extract)?);

        continuation = continuation_of(&body)?;

        let Some(param_value) = &continuation else {
            break;
        };

        set_query_param(request, request_param, param_value);
    }

    if continuation.is_some() {
        return Err(page_budget_exhausted(max_pages));
    }

    Ok(ids)
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

/// Extracts one page's identifiers per the discovery extraction shape.
fn extract_ids(
    body: &control::DecodedBody,
    extract: &descriptor::DiscoveryExtract,
) -> Result<Vec<String>, Error> {
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

                    Ok(match strip_prefix {
                        Some(prefix) => id.strip_prefix(prefix).map(str::to_string).unwrap_or(id),
                        None => id,
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

            Ok(object.keys().cloned().collect())
        }
    }
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
        provider::definition::{discovery::Discovery, eval::ValueEnv},
        test_support::MockUpstream,
    };
    use serde_json::json;
    use std::collections::BTreeMap;

    /// The environment one discovery test carries: no slots, the mock base URL, and a static
    /// secret, leaked into `'static` references the descriptor can borrow.
    fn discovery_env(base_url: &str) -> ValueEnv<'static> {
        let base_url: &'static str = Box::leak(base_url.to_string().into_boxed_str());
        let slots: &'static BTreeMap<String, String> = Box::leak(Box::new(BTreeMap::new()));

        ValueEnv::for_discovery(slots, Some(base_url), "secret")
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

        let ids = run_discovery(&client, &token_discovery(3), &discovery_env(&mock.base())).await;

        assert_eq!(ids.unwrap(), vec!["model-a"]);

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

        let ids = run_discovery(&client, &token_discovery(2), &discovery_env(&mock.base())).await;

        assert_eq!(ids.unwrap(), vec!["model-a", "model-b"]);

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

        let ids = run_discovery(&client, &cursor_discovery(2), &discovery_env(&mock.base())).await;

        assert_eq!(ids.unwrap(), vec!["model-a", "model-b"]);

        mock.shutdown.send(()).ok();
    }
}
