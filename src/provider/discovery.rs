//! Upstream model discovery, driven entirely by the provider definition's discovery descriptor
//! (README §5.5, §5.6).
//!
//! Discovery configures a request, a response extraction, and a pagination loop; it discovers
//! upstream model identifiers and never assigns protocol capabilities. A discovered model that
//! does not support its definition's format produces the upstream error when called. The
//! existing `discovered`/`vanished`/`manual` state machine is untouched: an empty or failed
//! upstream list never mass-vanishes existing rows, because a failed refresh is rejected before
//! the storage layer is reached.

use super::{Provider, control, http_body, oauth};
use crate::provider::definition::{
    discovery as descriptor,
    discovery::{Discovery, DiscoveryExtract, Pagination},
    eval::ValueEnv,
    template::{ControlRequest, ResponseEncoding},
};
use reqwest::Client;
use serde_json::Value as Json;

/// Upstream model discovery failures. These paths never forward foreign statuses, so an upstream
/// rejection pins 502 with the status embedded in the message.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// The upstream answered a non-2xx status.
    #[error("Upstream request failed with status {status}: {detail}")]
    Rejected {
        /// Upstream status code, embedded in the message.
        status: u16,

        /// Truncated upstream body excerpt.
        detail: String,
    },

    /// The upstream failed without a usable response, or answered with a payload that does not
    /// carry any recognizable model list.
    #[error("Upstream request failed: {detail}")]
    Unavailable {
        /// Human-readable failure detail.
        detail: String,
    },

    /// Discovery failed on JanusGate's side: the stored provider configuration violates an
    /// invariant.
    #[error("{message}")]
    Internal {
        /// Human-readable failure message.
        message: String,
    },
}

impl From<oauth::Error> for Error {
    fn from(error: oauth::Error) -> Self {
        match error {
            oauth::Error::Rejected { status, detail } => Self::Rejected { status, detail },
            oauth::Error::Unavailable { detail }
            | oauth::Error::Timeout { detail }
            | oauth::Error::NotFound { message: detail } => Self::Unavailable { detail },
            oauth::Error::Internal { message } => Self::Internal { message },
        }
    }
}

impl From<control::ControlFailure> for Error {
    fn from(failure: control::ControlFailure) -> Self {
        match failure {
            control::ControlFailure::Rejected { status, body } => Self::Rejected {
                status,
                detail: http_body::truncate_excerpt(&body),
            },
            // Discovery has no timeout-dependent schedule, so a transport timeout collapses
            // into the unavailable class like every other transport failure.
            control::ControlFailure::Unavailable { detail }
            | control::ControlFailure::Timeout { detail } => Self::Unavailable { detail },
        }
    }
}

/// Fetches the upstream model identifiers (`upstream_id` values) of a provider through its
/// definition's discovery descriptor, refreshing the access token first when a known expiry
/// requires it.
///
/// # Errors
///
/// Returns [`Error`] when the token refresh fails, the upstream call fails, the payload does not
/// carry the configured shape, or a discovered identifier is not routing-safe.
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
/// Returns [`Error`] per the discovery failure contract.
pub(crate) async fn run_discovery(
    http: &Client,
    discovery: &Discovery,
    env: &ValueEnv<'_>,
) -> Result<Vec<String>, Error> {
    let mut ids = Vec::new();

    let mut request = discovery.request.clone();

    match &discovery.pagination {
        None | Some(Pagination::None) => {
            let body =
                control::execute(http, &request, env, encoding_of(discovery), "upstream").await?;

            ids = extract_ids(&body, &discovery.extract)?;
        }

        Some(Pagination::Token {
            request_param,
            response_path,
            max_pages,
        }) => {
            for _ in 0..max_pages.get() {
                let body =
                    control::execute(http, &request, env, encoding_of(discovery), "upstream")
                        .await?;

                ids.extend(extract_ids(&body, &discovery.extract)?);

                let next = value_at(&body, response_path)
                    .and_then(Json::as_str)
                    .map(str::to_string);

                let Some(next) = next.filter(|token| !token.is_empty()) else {
                    break;
                };

                set_query_param(&mut request, request_param, &next);
            }
        }

        Some(Pagination::Cursor {
            has_more_path,
            last_id_path,
            request_param,
            max_pages,
        }) => {
            for _ in 0..max_pages.get() {
                let body =
                    control::execute(http, &request, env, encoding_of(discovery), "upstream")
                        .await?;

                ids.extend(extract_ids(&body, &discovery.extract)?);

                let has_more = value_at(&body, has_more_path).is_some_and(|value| match value {
                    Json::Bool(done) => *done,
                    Json::Number(number) => number.as_i64() == Some(1),
                    Json::String(text) => text == "true",
                    _ => false,
                });

                if !has_more {
                    break;
                }

                let Some(last_id) = value_at(&body, last_id_path)
                    .and_then(Json::as_str)
                    .map(str::to_string)
                else {
                    return Err(Error::Unavailable {
                        detail: "model discovery page lacks its last identifier".to_string(),
                    });
                };

                set_query_param(&mut request, request_param, &last_id);
            }
        }
    }

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
