//! Definition-backed data-plane connection and concrete codec body construction.

use super::{CompactionDispatch, Error, is_upstream_streaming};
use crate::{
    format::{UpstreamFormat, anthropic, gemini, openai},
    ids,
    ir::IrRequest,
    provider::{
        ModelRouting, Provider,
        definition::{
            eval::ValueEnv,
            template::{ParamRow, StringExpr, TemplateNode},
        },
    },
};
use reqwest::Client;

/// Builds one internal compaction-path HTTP request from explicit connection
/// facts: the evaluated URL expression, the evaluated header rows, the JSON
/// body spliced through the optional envelope, and the fixed `Accept:
/// application/json` the non-streaming compaction framing carries (the header
/// rows override it by name, exactly as the inference dispatch's do).
///
/// # Errors
///
/// Returns [`Error::Internal`] when the connection's expressions reference a
/// value this dispatch does not carry — stored drift — or when a header row
/// carries an unrenderable name or value.
pub(crate) fn build_internal_request<T: serde::Serialize>(
    http: &Client,
    provider: &Provider,
    routing: &ModelRouting,
    url: &StringExpr,
    header_rows: &[ParamRow],
    envelope: Option<&TemplateNode>,
    body: &T,
) -> Result<reqwest::RequestBuilder, Error> {
    let request_uuid = ids::new_uuid();

    let env = ValueEnv::for_compaction_operation(
        provider.credentials.slots(),
        provider.base_url.as_deref(),
        provider.credentials.secret(),
        &request_uuid,
        &routing.model.upstream_id,
    );

    let builder = evaluated_connection(
        http,
        &env,
        url,
        header_rows,
        false,
        "compaction operation URL",
        "compaction operation header",
    )?;
    attach_body(
        builder,
        &env,
        envelope,
        body,
        "compaction operation envelope",
    )
}

/// Builds the upstream HTTP request for one dispatch, entirely from the provider definition
/// (README §5.2): the evaluated URL (the streaming URL when this dispatch streams and
/// the definition carries one), the codec-rendered body attached through the definition's
/// envelope, the engine-owned `Accept` with the definition's header rows overriding it, and one
/// request-UUID cached across every expression of the dispatch.
///
/// # Errors
///
/// Returns [`Error`] when the definition's expressions reference a value this dispatch does not
/// carry — stored drift — or when the client request cannot be rendered into the body.
pub(super) fn build_upstream_request(
    http: &Client,
    provider: &Provider,
    routing: &ModelRouting,
    ir: &IrRequest,
    is_client_streaming: bool,
    compaction: Option<&CompactionDispatch>,
) -> Result<reqwest::RequestBuilder, Error> {
    let inference = &provider.definition.payload.inference;

    let is_upstream_streaming = is_upstream_streaming(inference, is_client_streaming);

    // One request UUID per dispatch, reused by every expression reference in it.
    let request_uuid = ids::new_uuid();

    let slots = provider.credentials.slots();

    let env = ValueEnv::for_inference(
        slots,
        provider.base_url.as_deref(),
        provider.credentials.secret(),
        &request_uuid,
        &routing.model.upstream_id,
    );

    // The streaming URL only when this dispatch streams and one is configured (README §5.2).
    let url_expr = if is_upstream_streaming {
        inference.stream_url.as_ref().unwrap_or(&inference.url)
    } else {
        &inference.url
    };

    // The compaction dispatch's overlay rows replace same-named inference rows
    // (the operation betas ride the operation definition), then the surviving
    // inference rows follow in order.
    let overlay = compaction
        .map(|dispatch| dispatch.overlay_headers.as_slice())
        .unwrap_or(&[]);

    let merged_rows = overlay_header_rows(&inference.headers, overlay);

    let builder = evaluated_connection(
        http,
        &env,
        url_expr,
        &merged_rows,
        is_upstream_streaming,
        "inference URL",
        "inference header",
    )?;

    attach_upstream_body(
        builder,
        &env,
        provider,
        ir,
        is_upstream_streaming,
        compaction,
    )
}

/// Renders the upstream body through the definition format's codec and attaches it to the
/// dispatch's request builder (README §5.2).
///
/// # Errors
///
/// Returns [`Error::InvalidRequest`] when the client request cannot be rendered into the
/// definition format's body, and [`Error::Internal`] when the envelope's expressions reference a
/// value this dispatch does not carry — stored drift.
fn attach_upstream_body(
    builder: reqwest::RequestBuilder,
    env: &ValueEnv<'_>,
    provider: &Provider,
    ir: &IrRequest,
    is_upstream_streaming: bool,
    compaction: Option<&CompactionDispatch>,
) -> Result<reqwest::RequestBuilder, Error> {
    let inference = &provider.definition.payload.inference;

    let envelope = inference.envelope.as_ref();

    match provider.definition.payload.format {
        UpstreamFormat::OpenAiResponse => attach_body(
            builder,
            env,
            envelope,
            &openai::response::render_request_body_with_extensions(
                ir,
                is_upstream_streaming,
                &inference.render_options,
                compaction.and_then(|dispatch| dispatch.context_management.as_deref()),
            )
            .map_err(Error::from)?,
            "inference envelope",
        ),

        UpstreamFormat::OpenAiChatCompletion => attach_body(
            builder,
            env,
            envelope,
            &openai::chat_completion::render_request_body(ir, is_upstream_streaming)
                .map_err(Error::from)?,
            "inference envelope",
        ),

        UpstreamFormat::GeminiGenerateContent => attach_body(
            builder,
            env,
            envelope,
            &gemini::generate_content::render_generate_content_request(ir).map_err(Error::from)?,
            "inference envelope",
        ),

        UpstreamFormat::GeminiInteraction => attach_body(
            builder,
            env,
            envelope,
            &gemini::interaction::render_request_body(ir, &ir.model, is_upstream_streaming)
                .map_err(Error::from)?,
            "inference envelope",
        ),

        UpstreamFormat::AnthropicMessages => {
            // A native threshold passthrough renders the vendor-shaped edits
            // through the threshold body render; every other Messages dispatch
            // renders the stripped form (the base render carries no compaction
            // members).
            let body = match compaction.and_then(|dispatch| dispatch.threshold_edits.as_deref()) {
                Some(edits) => {
                    anthropic::messages::render_threshold_body(ir, is_upstream_streaming, edits)
                }

                None => anthropic::messages::render_request_body(ir, is_upstream_streaming),
            }
            .map_err(Error::from)?;

            attach_body(builder, env, envelope, &body, "inference envelope")
        }
    }
}

/// Attaches one codec-rendered body to the request builder: without an envelope the typed body
/// rides to reqwest and serializes exactly once onto the wire; with an envelope, the evaluated
/// template borrows that same concrete body at its placeholder without constructing a JSON DOM.
///
/// # Errors
///
/// Returns [`Error::Internal`] when the envelope's expressions reference a value this dispatch
/// does not carry — stored drift.
fn attach_body<T: serde::Serialize>(
    builder: reqwest::RequestBuilder,
    env: &ValueEnv<'_>,
    envelope: Option<&TemplateNode>,
    body: &T,
    purpose: &str,
) -> Result<reqwest::RequestBuilder, Error> {
    let Some(envelope) = envelope else {
        return Ok(builder.json(body));
    };

    let body = env
        .render_json(envelope, body, purpose)
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;

    Ok(builder.json(&body))
}

/// Evaluates connection expressions and overlays engine-owned Accept with authored header rows.
///
/// # Errors
///
/// Returns stored expression drift or unrenderable header failures with the caller's diagnostic
/// context.
fn evaluated_connection(
    http: &Client,
    env: &ValueEnv<'_>,
    url: &StringExpr,
    rows: &[ParamRow],
    is_streaming: bool,
    url_purpose: &str,
    header_purpose: &str,
) -> Result<reqwest::RequestBuilder, Error> {
    let url = env
        .evaluate(url, url_purpose)
        .map_err(|error| Error::Internal {
            message: error.to_string(),
        })?;
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_static(if is_streaming {
            "text/event-stream"
        } else {
            "application/json"
        }),
    );
    for row in rows {
        let value = env
            .evaluate(&row.value, header_purpose)
            .map_err(|error| Error::Internal {
                message: error.to_string(),
            })?;
        let name = reqwest::header::HeaderName::from_bytes(row.name.as_bytes()).map_err(|_| {
            Error::Internal {
                message: format!(
                    "the definition carries an invalid header name `{}`",
                    row.name
                ),
            }
        })?;
        let value =
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| Error::Internal {
                message: "the definition carries an unrenderable header value".to_string(),
            })?;
        headers.insert(name, value);
    }
    Ok(http.post(url).headers(headers))
}

/// Merges an operation's overlay header rows over the inference header rows: an overlay row
/// replaces the base row of the same name — the operation's value wins, the base's position is
/// not kept — and the result preserves overlay order first, then the remaining base rows in
/// their order. This is how a compaction operation's pinned header (the `anthropic-beta` beta
/// declaration foremost) overrides the inference header set without reordering the rest.
#[must_use]
pub(crate) fn overlay_header_rows(
    base: &[crate::provider::definition::template::ParamRow],
    overlay: &[crate::provider::definition::template::ParamRow],
) -> Vec<crate::provider::definition::template::ParamRow> {
    let mut rows: Vec<_> = overlay.to_vec();

    for row in base {
        if !overlay
            .iter()
            .any(|overlay_row| overlay_row.name.eq_ignore_ascii_case(&row.name))
        {
            rows.push(row.clone());
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::overlay_header_rows;
    use crate::provider::definition::template::{ParamRow, StringExpr};

    /// Builds one literal header row.
    fn row(name: &str, value: &str) -> ParamRow {
        ParamRow {
            name: name.to_string(),
            value: StringExpr::literal(value),
        }
    }

    /// Overlay rows replace same-named base rows, keep overlay order ahead of the surviving
    /// base rows, and never reorder the untouched base rows.
    ///
    /// # Panics
    ///
    /// Panics if the merged rows diverge from the replace-by-name contract.
    #[test]
    fn overlay_header_rows_replace_by_name_and_preserve_order() {
        let base = vec![
            row("x-api-key", "key"),
            row("anthropic-version", "2023-06-01"),
            row("Anthropic-Beta", "old-beta"),
            row("x-api-key", "second-key"),
        ];
        let overlay = vec![
            row("anthropic-beta", "compact-2026-09-04"),
            row("x-trace", "t1"),
            row("ANTHROPIC-beta", "second-beta"),
        ];

        let merged = overlay_header_rows(&base, &overlay);

        assert_eq!(
            merged,
            vec![
                // The overlay row replaced the same-named base row and carries its own value.
                row("anthropic-beta", "compact-2026-09-04"),
                row("x-trace", "t1"),
                row("ANTHROPIC-beta", "second-beta"),
                // The surviving base rows keep their order.
                row("x-api-key", "key"),
                row("anthropic-version", "2023-06-01"),
                row("x-api-key", "second-key"),
            ]
        );

        // An empty overlay keeps the base rows untouched in order.
        assert_eq!(overlay_header_rows(&base, &[]), base);
    }
    /// Stream selection, connection expressions, and typed envelopes share one dispatch UUID.
    /// Overlay order reaches actual headers with case-insensitive last-insert-wins semantics.
    ///
    /// # Panics
    ///
    /// Panics if a first-party fixture cannot build or its connection/body contract diverges.
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the authored transport fixture builds and serializes"
    )]
    fn connection_selection_and_envelope_share_the_dispatch_identity() {
        use crate::provider::{
            DiscoveryState, Model, ModelRouting,
            definition::{
                inference::UpstreamStreaming,
                template::{TemplateNode, ValueExpr},
            },
        };
        use std::collections::BTreeMap;
        let mut provider = super::super::tests::fixture_provider(Some(TemplateNode::Object {
            fields: BTreeMap::from([
                (
                    "dispatch".into(),
                    TemplateNode::String {
                        expr: StringExpr::single(ValueExpr::RequestUuid),
                    },
                ),
                ("request".into(), TemplateNode::Body),
            ]),
        }));
        let inference = &mut provider.definition.payload.inference;
        inference.url = StringExpr(vec![
            ValueExpr::Literal {
                value: "https://upstream.test/json/".into(),
            },
            ValueExpr::RequestUuid,
        ]);
        inference.stream_url = Some(StringExpr(vec![
            ValueExpr::Literal {
                value: "https://upstream.test/stream/".into(),
            },
            ValueExpr::RequestUuid,
        ]));
        inference.headers = vec![
            ParamRow {
                name: "x-dispatch".into(),
                value: StringExpr::single(ValueExpr::RequestUuid),
            },
            row("X-Overlay", "inference"),
            row("x-overlay", "inference-last"),
        ];
        let routing = ModelRouting {
            model_route_id: 1,
            public_id: "public".into(),
            provider: provider.clone(),
            model: Model {
                id: 1,
                upstream_id: "physical".into(),
                is_enabled: true,
                discovery_state: DiscoveryState::Manual,
                thinking_levels: None,
                max_input_tokens: None,
                max_output_tokens: None,
                thinking_levels_override: None,
                max_input_tokens_override: None,
                max_output_tokens_override: None,
                compaction_support: None,
                compaction_support_override: None,
            },
        };
        let dispatch = super::CompactionDispatch {
            overlay_headers: vec![row("x-OVERLAY", "first"), row("X-overlay", "last")],
            ..Default::default()
        };
        let ir = super::super::tests::fixture_ir();
        for (mode, is_client_streaming, is_streaming) in [
            (UpstreamStreaming::MatchClient, false, false),
            (UpstreamStreaming::MatchClient, true, true),
            (UpstreamStreaming::Always, false, true),
        ] {
            provider.definition.payload.inference.upstream_streaming = mode;
            let request = super::build_upstream_request(
                &reqwest::Client::new(),
                &provider,
                &routing,
                &ir,
                is_client_streaming,
                Some(&dispatch),
            )
            .expect("the fixture builds")
            .build()
            .expect("the request completes");
            let uuid = request.headers()["x-dispatch"]
                .to_str()
                .expect("UUID header");
            assert_eq!(
                request.url().path(),
                format!("/{}/{uuid}", if is_streaming { "stream" } else { "json" })
            );
            assert_eq!(
                request.headers()["accept"],
                if is_streaming {
                    "text/event-stream"
                } else {
                    "application/json"
                }
            );
            assert_eq!(request.headers()["x-overlay"], "last");
            let body: serde_json::Value = serde_json::from_slice(
                request
                    .body()
                    .expect("JSON body")
                    .as_bytes()
                    .expect("buffered JSON"),
            )
            .expect("typed envelope JSON");
            assert_eq!(
                body.get("dispatch").and_then(serde_json::Value::as_str),
                Some(uuid)
            );
            assert_eq!(
                body.get("request")
                    .expect("the fixture envelope carries its codec body"),
                &serde_json::to_value(
                    crate::format::openai::chat_completion::render_request_body(&ir, is_streaming)
                        .expect("rendered body")
                )
                .expect("serialized body")
            );
        }
    }
}
