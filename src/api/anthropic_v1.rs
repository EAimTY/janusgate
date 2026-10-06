//! Anthropic inbound surface (`/api/anthropic-v1`).
//!
//! `POST /messages` (stream via body `stream: true`, `max_tokens` required per spec) and
//! `GET /models` per README §6.3. Authentication is `x-api-key` only with the
//! `anthropic-version` header required present (value not validated). Request parsing and
//! rendering go through [`crate::format::anthropic::messages`]; errors render through
//! [`crate::api::error::anthropic`]; the model stays in the body for this surface, so parsing
//! uses [`crate::format::RouteContext::empty`] extended with the caller's `anthropic-beta`
//! entries — the inbound beta requirements the request parsers validate; the accounting endpoint
//! is [`crate::accounting::ApiLogEndpointType::AnthropicMessages`].
//!
//! The shared outward serving pipeline (completion handling, responses, authentication
//! middleware, fallbacks) lives in [`crate::api::serving`]; this module contributes the Anthropic
//! surface identity — `x-api-key` credentials with the required `anthropic-version` header and
//! the Anthropic error envelope — plus the router, the completion endpoint, and the native
//! model-list shape.

use crate::{
    accounting::ApiLogEndpointType,
    api::{Error, serving},
    compaction_model::support_facts::CompactionSupportFacts,
    context::Context,
    database::Database,
    format::{RouteContext, anthropic::messages::Format as MessagesFormat},
};
use axum::{
    Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
};
use axum_extra::extract::WithRejection;
use serde::Serialize;

/// The Anthropic surface's identity for the shared pipeline: `x-api-key` credentials with a
/// required-present `anthropic-version` header, and the Anthropic error envelope.
struct Surface;

impl serving::Surface for Surface {
    fn render_error(error: &Error) -> (StatusCode, impl Serialize) {
        super::error::anthropic::render(error)
    }

    fn extract_credential(request: &Request) -> Result<String, serving::AuthRejection> {
        let Some(presented) = request
            .headers()
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
        else {
            return Err(serving::AuthRejection::MissingCredential);
        };

        if !request.headers().contains_key("anthropic-version") {
            return Err(serving::AuthRejection::Malformed(Error::MalformedRequest {
                param: Some("anthropic-version".to_string()),
                message: "The anthropic-version header is required".to_string(),
            }));
        }

        Ok(presented.to_owned())
    }
}

/// The `GET /models` success body in the native Anthropic shape: the full effective-model set in
/// one page (`has_more: false`), ignoring pagination parameters. The page bookends
/// (`first_id`/`last_id`) and the per-entry members (`display_name`, `created_at`) that the
/// official shape and its SDKs require are always rendered.
#[derive(Debug, Serialize)]
struct ModelList {
    /// One entry per served model, ordered by public ID.
    data: Vec<ModelObject>,

    /// The first entry's public ID; `null` when the page is empty.
    first_id: Option<String>,

    /// Always `false`: the gateway returns the full list in one page.
    has_more: bool,

    /// The last entry's public ID; `null` when the page is empty.
    last_id: Option<String>,
}

/// One served model in the native Anthropic model shape.
#[derive(Debug, Serialize)]
struct ModelObject {
    /// Always `model`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// The public routing ID, which doubles as the display name: the gateway serves no separate
    /// display name.
    display_name: String,

    /// Public routing ID (README §3).
    id: String,

    /// The official shape types `created_at` as an RFC 3339 date-time string; the gateway serves
    /// no model creation timestamps, so it renders the epoch in that string type — the same
    /// "unknown release date" convention the OpenAI surface expresses through its integer
    /// `created: 0`.
    created_at: &'static str,

    /// Effective maximum input tokens — the official `ModelInfo` member's own name (README
    /// §6.5); omitted when unknown, never fabricated.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_input_tokens: Option<i64>,

    /// Effective maximum output tokens, rendered in the official `ModelInfo` member's own
    /// spelling `max_tokens` (README §6.5); omitted when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<i64>,

    /// Effective compaction support facts keyed by the mechanism spellings — a gateway
    /// extension rendering only the known mechanisms; omitted when no mechanism is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    compaction: Option<CompactionSupportFacts>,
}

/// Builds the native model-list page from one effective-model listing: every entry renders its
/// public routing ID as both `id` and `display_name` at the epoch `created_at`, and the page
/// bookends are the page's own first and last entry ids. Shared by the `GET /models` handler
/// and the shape test, so the test pins the handler's actual mapping.
fn native_model_page(listed: Vec<crate::storage::models::EnabledModel>) -> ModelList {
    let data: Vec<ModelObject> = listed
        .into_iter()
        .map(|model| ModelObject {
            kind: "model",
            display_name: model.public_id.clone(),
            id: model.public_id,
            created_at: "1970-01-01T00:00:00Z",
            max_input_tokens: model.max_input_tokens,
            max_tokens: model.max_output_tokens,
            compaction: model.compaction,
        })
        .collect();

    ModelList {
        // The page bookends are the page's own first and last entry ids.
        first_id: data.first().map(|entry| entry.id.clone()),
        last_id: data.last().map(|entry| entry.id.clone()),
        data,
        has_more: false,
    }
}

/// Constructs the surface router with Anthropic-key authentication and its own 404/405 responses.
pub(super) fn router(database: Database) -> Router<Context> {
    serving::surface_router::<Surface>(
        database,
        Router::new()
            .route("/messages", post(messages))
            .route("/models", get(models)),
    )
}

/// Extracts the caller's `anthropic-beta` entries for the inbound beta requirements the
/// request parsers validate: every header line split on commas, entries trimmed, empty entries
/// dropped, in header order and case as sent (beta names are case-sensitive).
fn anthropic_betas(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all("anthropic-beta")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Handles `POST /messages`.
async fn messages(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
    headers: HeaderMap,
    WithRejection(body, _): WithRejection<Bytes, serving::BodyError<Surface>>,
) -> Response {
    let route = RouteContext::empty().with_anthropic_betas(anthropic_betas(&headers));

    serving::outcome_response::<Surface>(
        serving::run_completion::<MessagesFormat>(
            &context,
            &body,
            api_key.0,
            route,
            serving::OperationKind::Completion,
            ApiLogEndpointType::AnthropicMessages,
        )
        .await,
    )
}

/// Handles `GET /models` in the native Anthropic shape over the same effective-model set every
/// surface lists, ignoring pagination parameters and returning the full list in one page.
async fn models(
    State(context): State<Context>,
    api_key: serving::AuthenticatedApiKeyRow,
) -> Response {
    serving::model_list_response::<Surface, _, _>(&context.database, api_key.0, native_model_page)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::models::EnabledModel;

    /// Extracts the caller's `anthropic-beta` entries: every header line splits on commas,
    /// entries are trimmed, empty entries are dropped, header order and case are preserved.
    ///
    /// # Panics
    ///
    /// Panics if an extraction diverges from the documented rule.
    #[test]
    fn extracts_anthropic_beta_entries() {
        let mut headers = HeaderMap::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture header values are valid header strings"
        )]
        let first = "compact-2026-09-04,  interleaved-thinking-2025-05-14 "
            .parse()
            .expect("the fixture header value parses");

        #[expect(
            clippy::expect_used,
            reason = "the fixture header values are valid header strings"
        )]
        let second = "compact-2026-01-12"
            .parse()
            .expect("the fixture header value parses");

        headers.insert("anthropic-beta", first);
        headers.append("anthropic-beta", second);

        assert_eq!(
            anthropic_betas(&headers),
            vec![
                "compact-2026-09-04".to_string(),
                "interleaved-thinking-2025-05-14".to_string(),
                "compact-2026-01-12".to_string(),
            ]
        );

        // A missing header extracts no entries; empty entries between separators drop.
        let mut headers = HeaderMap::new();

        #[expect(
            clippy::expect_used,
            reason = "the fixture header values are valid header strings"
        )]
        let sparse = ",,".parse().expect("the fixture header value parses");

        headers.insert("anthropic-beta", sparse);

        assert!(anthropic_betas(&headers).is_empty());
        assert!(anthropic_betas(&HeaderMap::new()).is_empty());
    }

    /// One unknown-capability listing entry: every declaration `None` (README §3).
    fn unknown_entry(public_id: &str, provider_name: &str) -> EnabledModel {
        EnabledModel {
            public_id: public_id.to_string(),
            provider_name: provider_name.to_string(),
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            compaction: None,
        }
    }

    /// One known-capability listing entry: both token caps declared (the Anthropic surface renders
    /// exactly those two, README §6.5).
    fn known_entry(public_id: &str, max_input_tokens: i64, max_tokens: i64) -> EnabledModel {
        EnabledModel {
            max_input_tokens: Some(max_input_tokens),
            max_output_tokens: Some(max_tokens),
            ..unknown_entry(public_id, "provider")
        }
    }

    /// Verifies the native model-listing shape: every entry carries the doc-pinned members
    /// (`type`, `id`, `display_name`, `created_at`), the page carries `first_id`/`last_id`
    /// bookending its `data` (`null` when the page is empty), `has_more` is always `false`, and
    /// the official token-cap members ride along when known and stay absent when unknown — the
    /// gateway-extension `compaction` facts beside them, keyed by the mechanism spellings and
    /// carrying only known mechanisms (README §6.5).
    ///
    /// # Panics
    ///
    /// Panics if the rendered shape diverges from the native contract.
    #[test]
    fn renders_native_model_listing_shape() {
        use crate::compaction_model::mechanism::{Mechanism, MechanismSupport};

        let render = native_model_page;

        #[expect(clippy::expect_used, reason = "the fixture fact pairs are non-empty")]
        let compaction = CompactionSupportFacts::from_pairs([(
            Mechanism::MessagesOnDemandCompaction,
            MechanismSupport::Supported,
        )])
        .expect("fixture fact pairs are non-empty");

        let listed = vec![unknown_entry("b-model", "b-provider"), {
            let mut known = known_entry("a-model", 200_000, 32_000);

            known.compaction = Some(compaction);

            known
        }];

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(render(listed)).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "data": [
                    {
                        "type": "model",
                        "display_name": "b-model",
                        "id": "b-model",
                        "created_at": "1970-01-01T00:00:00Z",
                    },
                    {
                        "type": "model",
                        "display_name": "a-model",
                        "id": "a-model",
                        "created_at": "1970-01-01T00:00:00Z",
                        "max_input_tokens": 200000,
                        "max_tokens": 32000,
                        "compaction": {"messages_on_demand_compaction": "supported"},
                    },
                ],
                "first_id": "b-model",
                "has_more": false,
                "last_id": "a-model",
            })
        );

        #[expect(clippy::expect_used, reason = "the listing body always serializes")]
        let value = serde_json::to_value(render(Vec::new())).expect("the listing serializes");

        assert_eq!(
            value,
            serde_json::json!({
                "data": [],
                "first_id": null,
                "has_more": false,
                "last_id": null,
            })
        );
    }
}
