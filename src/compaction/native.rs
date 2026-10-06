//! Native compaction operation dispatches (README §5.9, §6.8): the two
//! native operations the executor coordinates — the Responses standalone
//! compact endpoint described by its definition operation, and the Messages
//! on-demand compaction riding the ordinary inference connection with its
//! overlay headers.
//!
//! A dispatched native operation that answers with a positively classified
//! unsupported-mechanism response reports [`NativeAttemptFailure::ClassifiedUnsupported`]
//! — the one runtime evidence that permits the executor's single summary
//! fallback. Every other failure keeps its existing upstream semantics and
//! propagates unchanged. Pre-dispatch failures record no usage; a successful
//! dispatch records the usage the upstream reported into the ledger.

use super::error::CompactionError;
use crate::{
    compaction_model::accounting::CompactionLedger,
    context::Context,
    format::{
        anthropic::messages,
        openai::response::{
            CompactedResponseNative, UpstreamUsage, parse_compact_response,
            render_compact_request_body,
        },
    },
    gateway::{self, Error as GatewayError},
    ir::{IrAggregation, IrRequest, IrUsage},
    provider::{ModelRouting, definition::compaction::StandaloneCompactOperation},
    storage::api_logs::ApiLogDraft,
};
use serde::Deserialize;
use serde_json::Value;

/// Why one native dispatch failed, before any artifact was committed.
#[derive(Debug)]
pub(crate) enum NativeAttemptFailure {
    /// The upstream answered with a response the definition's typed classifier
    /// positively matches — status, error code, or error parameter. The one
    /// runtime evidence that permits the single summary fallback.
    ClassifiedUnsupported(String),

    /// Any other failure: it keeps its existing upstream semantics and
    /// propagates unchanged — there is no semantic fallback.
    Failed(GatewayError),
}

impl From<GatewayError> for NativeAttemptFailure {
    fn from(error: GatewayError) -> Self {
        Self::Failed(error)
    }
}

impl From<CompactionError> for NativeAttemptFailure {
    fn from(error: CompactionError) -> Self {
        Self::Failed(error.into())
    }
}

/// The typed view of a vendor compaction block the on-demand validation reads:
/// every member optional, the rest of the vendor shape ignored. The raw block
/// value stays the replay artifact; this view only decides which members the
/// gateway reads.
#[derive(Debug, Default, Deserialize)]
struct CompactionBlockView {
    /// The readable summary content; absent or null marks the native
    /// null-content no-op block.
    #[serde(default)]
    content: Option<String>,

    /// The vendor signature authenticating the block.
    #[serde(default)]
    signature: Option<String>,
}

/// Converts one native Responses usage observation into the IR usage the ledger
/// records, mapping each reported breakdown and leaving unreported ones absent.
#[must_use]
fn usage_to_ir(usage: &UpstreamUsage) -> IrUsage {
    IrUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cached_tokens),
        cache_write_tokens: usage
            .input_tokens_details
            .as_ref()
            .and_then(|details| details.cache_write_tokens),
        reasoning_tokens: usage
            .output_tokens_details
            .as_ref()
            .and_then(|details| details.reasoning_tokens),
    }
}

/// Whether a non-2xx response is a positively classified unsupported-mechanism
/// boundary: any listed status hits, or the bounded error body's `code` or
/// `param` member matches a listed value. An absent classifier never positively
/// classifies.
pub(crate) async fn classify_failure(
    classifier: Option<&crate::provider::definition::compaction::UnsupportedClassifier>,
    status: reqwest::StatusCode,
    body: &str,
) -> Option<String> {
    let classifier = classifier?;

    let detail = format!("upstream rejected the compaction operation with status {status}");

    if classifier
        .statuses
        .iter()
        .any(|listed| reqwest::StatusCode::from_u16(*listed).is_ok_and(|listed| listed == status))
    {
        return Some(detail);
    }

    // The code and parameter members are read out of the bounded error body
    // through typed probes; a body that is not the expected error object shape
    // positively classifies by neither member.
    #[derive(Deserialize)]
    struct ErrorShape {
        /// The error object.
        #[serde(default)]
        error: Option<ErrorMember>,
    }

    #[derive(Deserialize)]
    struct ErrorMember {
        /// Upstream error code.
        #[serde(default, deserialize_with = "crate::serde_helpers::string_or_none")]
        code: Option<String>,

        /// Offending parameter.
        #[serde(default)]
        param: Option<String>,
    }

    let shape = serde_json::from_str::<ErrorShape>(body).ok()?;

    let error = shape.error?;

    if let Some(code) = &error.code
        && classifier.codes.iter().any(|listed| listed == code)
    {
        return Some(format!("{detail}, code `{code}`"));
    }

    if let Some(param) = &error.param
        && classifier.parameters.iter().any(|listed| listed == param)
    {
        return Some(format!("{detail}, parameter `{param}`"));
    }

    None
}

/// Dispatches the Responses standalone compact operation against the
/// definition's own connection: the operation's URL and full header set, the
/// codec-rendered compact body through the operation's envelope, and always
/// non-streaming framing (the operation frames itself; an inference descriptor
/// that forces SSE does not force it here).
///
/// On success the parsed native result — the complete ordered output window
/// beside its usage — is returned, with the reported usage recorded into the
/// ledger. A non-2xx answer first records positively classified quota evidence, which propagates
/// without a summary fallback. Otherwise a positively classified unsupported boundary reports
/// [`NativeAttemptFailure::ClassifiedUnsupported`],
/// every other rejection propagates as its existing upstream error.
///
/// # Errors
///
/// Returns [`NativeAttemptFailure`] for every dispatch failure; the request's
/// accounting draft is stamped at the send, so a dispatched-but-failed
/// operation still recorded its dispatch instant.
///
/// # Panics
///
/// Panics if the codec-rendered compact body fails to serialize, which cannot
/// occur for a first-party rendered body.
pub(crate) async fn responses_standalone(
    context: &Context,
    routing: &ModelRouting,
    operation: &StandaloneCompactOperation,
    ir: &IrRequest,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<CompactedResponseNative, NativeAttemptFailure> {
    let provider = crate::provider::oauth::ensure_fresh_token(
        &context.database,
        &context.control_plane,
        &routing.provider,
    )
    .await
    .map_err(GatewayError::from)?;
    let mut ir = ir.clone();
    ir.model = routing.model.upstream_id.clone();
    let ir = &ir;
    let body = render_compact_request_body(ir).map_err(GatewayError::from)?;

    let request = gateway::build_internal_request(
        &context.data_plane,
        &provider,
        routing,
        &operation.url,
        &operation.headers,
        operation.envelope.as_ref(),
        &body,
    )?;

    let response = match gateway::dispatch(
        context,
        request,
        &provider,
        &routing.model.upstream_id,
        log,
        ownership,
        gateway::QuotaPolicy::Classify,
    )
    .await?
    {
        gateway::DispatchOutcome::Accepted(response) => response,
        gateway::DispatchOutcome::Rejected { status, body } => {
            let detail = body.lossy_text();
            let excerpt = crate::http_body::truncate_excerpt(&detail);

            // The operation's own classifier decides first; the compaction
            // section's declaration classifier backs it up.
            let classification =
                match classify_failure(operation.unsupported.as_ref(), status, &detail).await {
                    Some(classification) => Some(classification),

                    None => {
                        classify_failure(
                            routing
                                .provider
                                .definition
                                .payload
                                .compaction
                                .as_ref()
                                .and_then(|section| {
                                    section.mechanisms.responses_standalone_compact.as_ref()
                                })
                                .and_then(|declaration| declaration.unsupported.as_ref()),
                            status,
                            &detail,
                        )
                        .await
                    }
                };

            if let Some(classification) = classification {
                return Err(NativeAttemptFailure::ClassifiedUnsupported(classification));
            }

            return Err(NativeAttemptFailure::Failed(GatewayError::Upstream {
                status,
                detail: excerpt,
            }));
        }
    };

    let bytes = response
        .bytes()
        .await
        .map_err(|error| GatewayError::Unavailable {
            detail: format!("upstream read failed: {error}"),
        })?;

    let body = gateway::unwrap_response(&bytes, &operation.response_unwrap)?;

    #[derive(Deserialize)]
    struct UsageReport {
        /// Reported usage survives an invalid compacted window.
        usage: Option<UpstreamUsage>,
    }
    if let Ok(report) = serde_json::from_slice::<UsageReport>(body) {
        ledger.record(report.usage.as_ref().map(usage_to_ir));
        if report.usage.is_some() {
            log.stamp_billed_usage(ledger.reported_billed_totals());
        }
    }
    let native = parse_compact_response(body).map_err(GatewayError::from)?;

    Ok(native)
}

/// Dispatches the definition's `count_tokens` operation: the codec-rendered
/// count body through the operation's own connection, always non-streaming,
/// yielding the upstream-reported input-token measurement. The count is a
/// measurement, never billed usage: it records no ledger iteration and
/// no token totals (README §6.8).
///
/// # Errors
///
/// Returns the gateway error for transport failures, non-2xx answers, and
/// malformed bodies; the caller falls back to the documented text estimator,
/// so a counter failure never fails the request on its own.
///
/// # Panics
///
/// Panics if the codec-rendered count body fails to serialize, which cannot
/// occur for a first-party rendered body.
pub(crate) async fn count_tokens(
    context: &Context,
    routing: &ModelRouting,
    operation: &crate::provider::definition::compaction::CountTokensOperation,
    ir: &IrRequest,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<u64, GatewayError> {
    let provider = crate::provider::oauth::ensure_fresh_token(
        &context.database,
        &context.control_plane,
        &routing.provider,
    )
    .await
    .map_err(GatewayError::from)?;
    let mut ir = ir.clone();
    ir.model = routing.model.upstream_id.clone();
    let ir = &ir;
    let body = crate::format::anthropic::messages::render_count_tokens_request(ir)
        .map_err(GatewayError::from)?;

    let request = gateway::build_internal_request(
        &context.data_plane,
        &provider,
        routing,
        &operation.url,
        &operation.headers,
        operation.envelope.as_ref(),
        &body,
    )?;

    let response = gateway::dispatch(
        context,
        request,
        &provider,
        &routing.model.upstream_id,
        log,
        ownership,
        gateway::QuotaPolicy::Disabled,
    )
    .await?
    .into_accepted()?;

    let bytes = response
        .bytes()
        .await
        .map_err(|error| GatewayError::Unavailable {
            detail: format!("upstream read failed: {error}"),
        })?;

    let body = gateway::unwrap_response(&bytes, &operation.response_unwrap)?;

    #[derive(Deserialize)]
    struct CountShape {
        /// The upstream-reported input-token measurement.
        #[serde(default)]
        input_tokens: Option<u64>,
    }

    let shape: CountShape =
        serde_json::from_slice(body).map_err(|error| GatewayError::Unavailable {
            detail: format!("malformed count response: {error}"),
        })?;

    let measured = shape.input_tokens.ok_or(GatewayError::Unavailable {
        detail: "the count response carries no input_tokens measurement".to_string(),
    })?;

    Ok(measured)
}

/// The outcome of a native Messages on-demand compaction dispatch.
#[derive(Debug)]
pub(crate) enum OnDemandOutcome {
    /// The upstream returned one complete, signed compaction block: the exact
    /// native artifact, beside the raw vendor iteration entries the usage
    /// render carries.
    Success {
        /// The vendor-shaped block, verbatim.
        block: Value,

        /// The raw `usage.iterations` entries the upstream reported.
        iterations: Vec<crate::ir::compaction::UsageIteration>,
    },

    /// The documented native no-summary outcome: the upstream answered without
    /// a usable compaction block — after a refusal, truncation, a tool use, an
    /// exhausted context window, or a plain answer. The original history
    /// stands; the consumed response renders with its own semantics. This is
    /// neither an error nor a success artifact.
    NoSummary {
        /// The aggregated upstream response.
        aggregation: IrAggregation,
    },
}

/// Dispatches the native Messages on-demand compaction: the ordinary inference
/// connection with the declaration's overlay headers replacing same-named rows,
/// the on-demand body at the given output budget, and always non-streaming
/// framing (an always-streaming definition still streams, and the SSE parser
/// captures the block the same way).
///
/// The validation is the protocol's own: exactly one compaction block whose
/// content and signature are both present is a success artifact; content
/// without a signature is an unusable native result; a null-content block or no
/// block at all is the documented no-summary outcome.
///
/// # Errors
///
/// Returns [`NativeAttemptFailure`] as [`responses_standalone`], plus an
/// unusable-result failure for a multi-block or unsigned native result.
///
/// # Panics
///
/// Panics if the codec-rendered on-demand body fails to serialize, which cannot
/// occur for a first-party rendered body.
#[expect(
    clippy::too_many_arguments,
    reason = "the dispatch carries the connection overlay, the operation input, and the \
              accounting facts the native call shares; splitting them would obscure the \
              call's single contract"
)]
pub(crate) async fn messages_on_demand(
    context: &Context,
    routing: &ModelRouting,
    overlay_headers: &[crate::provider::definition::template::ParamRow],
    classifier: Option<&crate::provider::definition::compaction::UnsupportedClassifier>,
    ir: &IrRequest,
    instructions: Option<&str>,
    max_tokens: u64,
    ledger: &mut CompactionLedger,
    log: &mut ApiLogDraft,
    ownership: Option<&mut crate::affinity::Ownership>,
) -> Result<OnDemandOutcome, NativeAttemptFailure> {
    let provider = crate::provider::oauth::ensure_fresh_token(
        &context.database,
        &context.control_plane,
        &routing.provider,
    )
    .await
    .map_err(GatewayError::from)?;
    let mut ir = ir.clone();
    ir.model = routing.model.upstream_id.clone();
    let ir = &ir;
    let body = messages::render_on_demand_body(
        ir,
        gateway::is_upstream_streaming(&provider.definition.payload.inference, false),
        instructions,
        max_tokens,
    )
    .map_err(GatewayError::from)?;

    let inference = &routing.provider.definition.payload.inference;

    let merged_headers = gateway::overlay_header_rows(&inference.headers, overlay_headers);

    let request = gateway::build_internal_request(
        &context.data_plane,
        &provider,
        routing,
        &inference.url,
        &merged_headers,
        inference.envelope.as_ref(),
        &body,
    )?;

    let response = match gateway::dispatch(
        context,
        request,
        &provider,
        &routing.model.upstream_id,
        log,
        ownership,
        gateway::QuotaPolicy::Classify,
    )
    .await?
    {
        gateway::DispatchOutcome::Accepted(response) => response,
        gateway::DispatchOutcome::Rejected { status, body } => {
            let detail = body.lossy_text();
            let excerpt = crate::http_body::truncate_excerpt(&detail);

            if let Some(classification) = classify_failure(classifier, status, &detail).await {
                return Err(NativeAttemptFailure::ClassifiedUnsupported(classification));
            }

            return Err(NativeAttemptFailure::Failed(GatewayError::Upstream {
                status,
                detail: excerpt,
            }));
        }
    };

    // An always-streaming definition streams the block; the SSE parser captures
    // it identically to the JSON body parse.
    let is_streaming = gateway::is_upstream_streaming(inference, false);

    let mut parser = messages::UpstreamParser::new();

    let (aggregation, is_terminal_real) = gateway::pump_internal_response(
        response,
        &mut parser,
        is_streaming,
        inference.response_unwrap.clone(),
        ledger,
        log,
    )
    .await?;

    if !is_terminal_real {
        return Err(NativeAttemptFailure::Failed(GatewayError::Unavailable {
            detail: "the upstream response ended without a terminal completion".to_string(),
        }));
    }

    match parser.compaction_blocks.as_slice() {
        [(index, block)] => {
            let view = CompactionBlockView::deserialize(block).map_err(|_| {
                NativeAttemptFailure::Failed(GatewayError::Unavailable {
                    detail: format!(
                        "the upstream compaction block at content index {index} is not a \
                         compaction block"
                    ),
                })
            })?;

            match (view.content, view.signature) {
                (Some(_content), Some(_signature)) => Ok(OnDemandOutcome::Success {
                    block: block.clone(),
                    iterations: parser.compaction_iterations.clone(),
                }),

                (Some(_), None) => Err(NativeAttemptFailure::Failed(GatewayError::Unavailable {
                    detail: "the upstream compaction block carries content without its signature"
                        .to_string(),
                })),

                // The null-content block is the native no-op: it cannot carry
                // successful gateway state and cannot authorize discarding
                // history.
                (None, _) => Ok(OnDemandOutcome::NoSummary { aggregation }),
            }
        }

        [] => Ok(OnDemandOutcome::NoSummary { aggregation }),

        [_, _, ..] => Err(NativeAttemptFailure::Failed(GatewayError::Unavailable {
            detail: "the upstream answered with more than one compaction block".to_string(),
        })),
    }
}

/// Converts parsed threshold edits to their concrete upstream wire representation.
#[must_use]
pub(crate) fn threshold_edits(
    edits: &[crate::format::compaction_control::ThresholdEdit],
) -> Vec<crate::ir::compaction::ThresholdEdit> {
    edits.iter().map(Into::into).collect()
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "test fixtures parse and render values constructed to succeed"
    )]

    use super::{CompactionBlockView, threshold_edits, usage_to_ir};
    use crate::{
        format::{compaction_control::ThresholdEdit, openai::response::UpstreamUsage},
        ir::{IrUsage, compaction::AutomaticEntry},
    };
    use serde_json::json;

    /// The usage conversion maps every reported breakdown and leaves the
    /// unreported ones absent.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    fn usage_conversion_maps_reported_breakdowns() {
        let full = usage_to_ir(&UpstreamUsage {
            input_tokens: 100,
            output_tokens: 20,
            input_tokens_details: Some(crate::format::openai::PromptTokensDetails {
                cached_tokens: Some(5),
                cache_write_tokens: None,
            }),
            output_tokens_details: Some(crate::format::openai::CompletionTokensDetails {
                reasoning_tokens: Some(7),
            }),
            dropped_message_count: Some(3),
        });

        assert_eq!(
            full,
            IrUsage {
                input_tokens: 100,
                output_tokens: 20,
                cached_tokens: Some(5),
                cache_write_tokens: None,
                reasoning_tokens: Some(7),
            }
        );

        let bare = usage_to_ir(&UpstreamUsage::default());

        assert_eq!(bare, IrUsage::default());
    }

    /// The passthrough entry renders round-trip to the vendor shapes the
    /// upstream render members carry: thresholds and triggers only when
    /// stated, the pause flag only when requested.
    ///
    /// # Panics
    ///
    /// Panics if a rendered value diverges from the vendor shape.
    #[test]
    fn passthrough_values_render_the_vendor_shapes() {
        assert_eq!(
            serde_json::json!(vec![
                AutomaticEntry {
                    compact_threshold: Some(90_000),
                },
                AutomaticEntry {
                    compact_threshold: None,
                },
            ]),
            json!([
                {"type": "compaction", "compact_threshold": 90_000},
                {"type": "compaction"},
            ])
        );

        assert_eq!(
            serde_json::json!(threshold_edits(&[ThresholdEdit {
                trigger: Some(120_000),
                pause_after_compaction: true,
                instructions: Some(String::from("focus")),
            }])),
            json!([{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 120_000},
                "pause_after_compaction": true,
                "instructions": "focus",
            }])
        );
    }

    /// The vendor block view reads content and signature and ignores the rest.
    ///
    /// # Panics
    ///
    /// Panics if the view diverges from the tolerant-read contract.
    #[test]
    fn block_view_reads_content_and_signature() {
        let view: CompactionBlockView =
            serde_json::from_value(json!({"type": "compaction", "content": "s", "signature":
            "sig", "encrypted_content": "x", "tool_changes": []}))
            .expect("the vendor block view parses");

        assert_eq!(view.content.as_deref(), Some("s"));
        assert_eq!(view.signature.as_deref(), Some("sig"));

        let null_block: CompactionBlockView =
            serde_json::from_value(json!({"type": "compaction", "content": null})).expect("parses");

        assert_eq!(null_block.content, None);
    }
}
