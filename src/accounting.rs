//! The request-accounting vocabulary: the closed sets the accounting layer records and the
//! management surface publishes (README §3 *ApiLog*, §7.1).
//!
//! One home for [`ApiLogEndpointType`], [`ApiLogOutcome`], and [`UsageGroupBy`]: every layer
//! that names a recorded fact — the outward surfaces stamping the accounting draft, the
//! api-log store writing the rows, the management reads filtering and grouping them, and the
//! wire payloads echoing them — speaks these types directly, so no layer imports another's
//! internals to name a vocabulary word.
//!
//! The serde and TypeScript names of the stored sets are the storage names — the `CHECK`
//! vocabulary of the `api_logs` columns and the `as_str` renderings are one vocabulary (README
//! §3 *ApiLog*); [`UsageGroupBy`]'s names are the management wire names (README §7.1).

use serde::{Deserialize, Serialize};

/// The outward completion endpoint a request arrived on, as recorded in `api_logs.endpoint_type`.
///
/// The serde and TypeScript names are the storage names — the `CHECK` vocabulary of the column
/// and [`ApiLogEndpointType::as_str`] are one vocabulary (README §3 *ApiLog*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum ApiLogEndpointType {
    /// `POST /api/openai-v1/chat/completions`.
    #[serde(rename = "openai_chat_completions")]
    OpenaiChatCompletions,

    /// `POST /api/openai-v1/responses`.
    #[serde(rename = "openai_responses")]
    OpenaiResponses,

    /// `POST /api/openai-v1/responses/compact`, the Responses standalone compact endpoint.
    #[serde(rename = "openai_responses_compact")]
    OpenaiResponsesCompact,

    /// Gemini GenerateContent inbound surface (`POST /api/gemini-v1beta/models/...`).
    #[serde(rename = "gemini_generate_content")]
    GeminiGenerateContent,

    /// Gemini Interactions inbound surface (`POST /api/gemini-v1beta/interactions`).
    #[serde(rename = "gemini_interactions")]
    GeminiInteractions,

    /// Anthropic Messages inbound surface (`POST /api/anthropic-v1/messages`).
    #[serde(rename = "anthropic_messages")]
    AnthropicMessages,
}

impl ApiLogEndpointType {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The storage
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 6] = [
        Self::OpenaiChatCompletions,
        Self::OpenaiResponses,
        Self::OpenaiResponsesCompact,
        Self::GeminiGenerateContent,
        Self::GeminiInteractions,
        Self::AnthropicMessages,
    ];

    /// Renders the storage spelling of the endpoint type — the exact string the schema `CHECK`
    /// accepts.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::OpenaiChatCompletions => "openai_chat_completions",
            Self::OpenaiResponses => "openai_responses",
            Self::OpenaiResponsesCompact => "openai_responses_compact",
            Self::GeminiGenerateContent => "gemini_generate_content",
            Self::GeminiInteractions => "gemini_interactions",
            Self::AnthropicMessages => "anthropic_messages",
        }
    }

    /// Parses the stored spelling back into the closed set; anything outside it is storage drift,
    /// because the column is written only through [`ApiLogEndpointType::as_str`] and its schema
    /// `CHECK` pins the vocabulary.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|endpoint_type| endpoint_type.as_str() == text)
    }
}

/// The recorded outcome of one dispatched request, as stored in `api_logs.outcome`.
///
/// The serde and TypeScript names are the storage names — the `CHECK` vocabulary of the column
/// and [`ApiLogOutcome::as_str`] are one vocabulary (README §3 *ApiLog*).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum ApiLogOutcome {
    /// The response completed normally: `completed` + `200`.
    #[serde(rename = "completed")]
    Completed,

    /// The client disconnected before the response was delivered: `http_status` is `NULL` when
    /// nothing had been enqueued or handed off, and the already-declared `200` once streaming
    /// frames had been enqueued.
    #[serde(rename = "client_aborted")]
    ClientAborted,

    /// The upstream failed — a mirrored non-2xx status, a transport failure mapped to 502, or a
    /// mid-stream failure under an already-declared `200`.
    #[serde(rename = "upstream_error")]
    UpstreamError,

    /// A gateway-side failure under an already-declared `200`.
    #[serde(rename = "internal")]
    Internal,
}

impl ApiLogOutcome {
    /// Every variant in canonical order: the closed table [`Self::parse`] searches. The storage
    /// words live once each, in the exhaustive [`Self::as_str`] match. A new variant joins
    /// `ALL`; a partial table update is a compile error (the length pin), and the exhaustive
    /// match is what forces the variant's spelling to be stated.
    const ALL: [Self; 4] = [
        Self::Completed,
        Self::ClientAborted,
        Self::UpstreamError,
        Self::Internal,
    ];

    /// Renders the storage spelling of the outcome — the exact string the schema `CHECK` accepts.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::ClientAborted => "client_aborted",
            Self::UpstreamError => "upstream_error",
            Self::Internal => "internal",
        }
    }

    /// Parses the stored spelling back into the closed set; see
    /// [`ApiLogEndpointType::parse`] for the drift classification.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|outcome| outcome.as_str() == text)
    }
}

/// The bucketing dimension of the usage aggregation (README §7.1 `usage:query`): a closed,
/// unordered set — the element type is the validation, so any other value is rejected while the
/// request payload parses. The serde and TypeScript names are the wire names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "management-api-typescript-bindings", derive(ts_rs::TS))]
pub(crate) enum UsageGroupBy {
    /// UTC day of `started_at`, rendered `YYYY-MM-DD`.
    #[serde(rename = "day")]
    Day,

    /// Routed model row id.
    #[serde(rename = "model")]
    Model,

    /// Requested public route identity, stable through target edits.
    #[serde(rename = "model_route")]
    ModelRoute,

    /// Owning provider row id.
    #[serde(rename = "provider")]
    Provider,

    /// Authenticating API key row id.
    #[serde(rename = "api_key")]
    ApiKey,

    /// Outward completion endpoint the request arrived on.
    #[serde(rename = "endpoint_type")]
    EndpointType,

    /// Recorded outcome of the request.
    #[serde(rename = "outcome")]
    Outcome,

    /// Whether the client requested a streamed response.
    #[serde(rename = "is_stream")]
    IsStream,

    /// HTTP status delivered to the client; the `NULL` rows form their own bucket.
    #[serde(rename = "http_status")]
    HttpStatus,
}

impl UsageGroupBy {
    /// Renders the wire spelling of the dimension — the exact string the request vocabulary
    /// carries.
    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Model => "model",
            Self::ModelRoute => "model_route",
            Self::Provider => "provider",
            Self::ApiKey => "api_key",
            Self::EndpointType => "endpoint_type",
            Self::Outcome => "outcome",
            Self::IsStream => "is_stream",
            Self::HttpStatus => "http_status",
        }
    }
}
