//! Remote-compaction wire behavior of the Responses codec: the standalone
//! compact endpoint's request and response payloads, the compaction state item
//! builders shared by parsing and rendering, and the trigger lifecycle's
//! streaming renderer.
//!
//! The compact endpoint (README §2.2) is not a generation request at a
//! different URL: its body carries no generation-only controls and no `stream`
//! member, its upstream framing is always non-streaming, and its success body
//! is a `response.compaction` object whose output is the canonical next
//! window.

use super::{
    client_render::SseRenderer,
    request_parse::{ItemIn, build_instructions, convert_input},
    upstream_parse::{CompactedResponseNative, UpstreamUsage},
};
use crate::{
    format::{
        self, ClientSseRenderer as _, CompactionControl, ParsedCompaction, ParsedRequest,
        StringOrArray,
    },
    ids,
    ir::{InvalidRequest, IrEvent, IrRequest, IrUsage},
    unix_time,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The `object` spelling of a compacted response body.
const COMPACTED_RESPONSE_OBJECT: &str = "response.compaction";

/// The strictly parsed standalone compact request body.
///
/// The endpoint carries no generation-only controls — no tools, sampling,
/// reasoning, text, or streaming members — so a body carrying one is rejected
/// by the strict shape itself. The caching and tiering conventions are the
/// ordinary Responses envelope's accepted-and-ignored controls (README
/// §4.1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompactRequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation input: a plain string or an ordered item array.
    input: StringOrArray<ItemIn>,

    /// System instructions.
    #[serde(default)]
    instructions: Option<String>,

    /// Stateful continuation identifier; the gateway is stateless, so a
    /// present, non-null value is rejected rather than silently dropped.
    #[serde(default)]
    previous_response_id: Option<String>,

    /// Upstream prompt-cache placement key; accepted and ignored (README
    /// §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    prompt_cache_key: Option<String>,

    /// Upstream prompt-cache options; accepted and ignored (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    prompt_cache_options: Option<PromptCacheOptions>,

    /// Upstream prompt-cache retention hint; accepted and ignored (README
    /// §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    prompt_cache_retention: Option<String>,

    /// Requested service tier; accepted and ignored (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<String>,
}

/// Upstream prompt-cache options.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptCacheOptions {
    /// Cache placement mode.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    mode: Option<String>,

    /// Cache time-to-live hint.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    ttl: Option<String>,
}

/// Parses a standalone compact request body into IR with its compaction
/// control: the route itself is the control
/// ([`CompactionControl::ResponsesStandalone`]), the body never states one.
///
/// The request is always non-streaming and the input conversion is the
/// ordinary `/responses` path's own, so history carrying compaction and
/// reasoning state parses identically on both endpoints.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported
/// fields, a present `previous_response_id` (stateful continuation is not
/// supported), a `compaction_trigger` item (the trigger belongs to the
/// ordinary endpoint), and every input-conversion rejection.
pub(crate) fn parse_compact_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: CompactRequestBody = format::parse_client_request(body)?;

    if let Some(previous) = request.previous_response_id {
        return Err(InvalidRequest::field(
            "previous_response_id",
            format!(
                "Stateful continuation is not supported; the gateway is stateless, so \
                 previous_response_id `{previous}` cannot be resolved"
            ),
        ));
    }

    let input_items: Vec<ItemIn> = match request.input {
        StringOrArray::Text(text) => vec![ItemIn::user_text(text)],

        StringOrArray::Array(items) => items,
    };

    let mut converted = convert_input(input_items)?;
    if let Some(instructions) = &request.instructions {
        converted.instruction_items.insert(
            0,
            crate::format::ParsedInstructionItem {
                role: crate::compaction::identity::InstructionRole::System,
                text: instructions.clone(),
            },
        );
    }

    if converted.has_trigger {
        return Err(InvalidRequest::field(
            "input",
            "compaction_trigger is not valid on the compact endpoint",
        ));
    }

    Ok(ParsedRequest {
        ir: IrRequest {
            state_items: converted.state_items,
            model: request.model,
            instructions: build_instructions(request.instructions, converted.instruction_parts),
            messages: converted.messages,
            ..IrRequest::default()
        },
        is_streaming: false,
        should_include_usage: false,
        compaction: Some(ParsedCompaction {
            control: Some(CompactionControl::ResponsesStandalone),
            boundaries: converted.boundaries,
            instruction_items: converted.instruction_items,
        }),
    })
}

/// A `compaction` item as rendered outward and replayed inbound: the tag, the
/// item identifier, the opaque state, and — on response items only, when
/// known — the authoring provenance.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename = "compaction")]
struct CompactionItemOut {
    /// Gateway- or provider-minted item identifier.
    id: String,

    /// The opaque replay state.
    encrypted_content: String,

    /// Authoring provenance, carried only when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    created_by: Option<String>,
}

/// Builds the JSON value of one `compaction` item in the official member
/// order (`type`, `id`, `encrypted_content`, then `created_by` when present).
///
/// The item is a vendor-shaped fragment: its shape is the Responses protocol's,
/// so the value is the wire form both the compacted response output and the
/// replayed state item carry.
///
/// # Panics
///
/// Panics if the item fails to serialize, which cannot occur for a first-party
/// serde type.
pub(crate) fn compaction_item_json(
    id: Option<String>,
    encrypted_content: String,
    created_by: Option<String>,
) -> Value {
    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered item serializes totally"
    )]
    serde_json::to_value(CompactionItemOut {
        id: id.unwrap_or_else(|| format!("cmp_{}", ids::new_uuid())),
        encrypted_content,
        created_by,
    })
    .expect("a compaction item serializes")
}

/// Builds the JSON value of one replayed reasoning state item: the typed
/// members re-serialized with their presence preserved, under the item's
/// `reasoning` type tag.
///
/// # Panics
///
/// Panics if the item fails to serialize, which cannot occur for a first-party
/// serde type.
pub(super) fn reasoning_item_json(reasoning: &super::request_parse::ReasoningItem) -> Value {
    /// The serialization wrapper supplying the item's type tag; the tagged
    /// members ride flattened, so their presence is preserved exactly.
    #[derive(Serialize)]
    #[serde(tag = "type", rename = "reasoning")]
    struct ReasoningItemOut<'a> {
        /// The typed members.
        #[serde(flatten)]
        item: &'a super::request_parse::ReasoningItem,
    }

    #[expect(
        clippy::expect_used,
        reason = "a first-party rendered item serializes totally"
    )]
    serde_json::to_value(ReasoningItemOut { item: reasoning }).expect("a reasoning item serializes")
}

/// The standalone compact endpoint's success body: the `response.compaction`
/// object. The output is the canonical next window — the complete ordered
/// vendor-shaped item list — never reduced to its compaction item (README
/// §4.1).
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CompactedResponseBody {
    /// Gateway-issued response identifier.
    id: String,

    /// Always `response.compaction`.
    object: &'static str,

    /// Unix creation timestamp in seconds.
    created_at: i64,

    /// The canonical next window, in output order.
    output: Vec<Value>,

    /// Token accounting of the calls that produced the window.
    usage: CompactUsage,
}

/// Token accounting of a compacted response, in the official Responses usage
/// shape.
///
/// The five official fields are required integers — the detail objects
/// included (openai-python `ResponseUsage`) — so absent upstream breakdowns
/// zero-fill; xAI's optional `dropped_message_count` rides beside them only
/// when the upstream reported it.
#[derive(Debug, Clone, Serialize)]
struct CompactUsage {
    /// Tokens consumed by the input.
    input_tokens: u64,

    /// Input token breakdown.
    input_tokens_details: CompactInputTokensDetails,

    /// Tokens produced by the output.
    output_tokens: u64,

    /// Output token breakdown.
    output_tokens_details: CompactOutputTokensDetails,

    /// Sum of input and output tokens.
    total_tokens: u64,

    /// Messages the upstream dropped while compacting (xAI extension);
    /// rendered only when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    dropped_message_count: Option<u64>,
}

/// The input token breakdown of a compacted response.
#[derive(Debug, Clone, Serialize)]
struct CompactInputTokensDetails {
    /// Input tokens served from the upstream prompt cache.
    cached_tokens: u64,

    /// Input tokens written to the upstream prompt cache.
    cache_write_tokens: u64,
}

/// The output token breakdown of a compacted response.
#[derive(Debug, Clone, Serialize)]
struct CompactOutputTokensDetails {
    /// Output tokens spent on model reasoning.
    reasoning_tokens: u64,
}

impl CompactUsage {
    /// Renders one native upstream observation, zero-filling every absent
    /// breakdown.
    fn from_upstream(usage: UpstreamUsage) -> Self {
        let input_details = usage.input_tokens_details;

        Self {
            input_tokens: usage.input_tokens,
            input_tokens_details: CompactInputTokensDetails {
                cached_tokens: input_details
                    .as_ref()
                    .and_then(|details| details.cached_tokens)
                    .unwrap_or(0),
                cache_write_tokens: input_details
                    .as_ref()
                    .and_then(|details| details.cache_write_tokens)
                    .unwrap_or(0),
            },
            output_tokens: usage.output_tokens,
            output_tokens_details: CompactOutputTokensDetails {
                reasoning_tokens: usage
                    .output_tokens_details
                    .as_ref()
                    .and_then(|details| details.reasoning_tokens)
                    .unwrap_or(0),
            },
            total_tokens: usage.input_tokens + usage.output_tokens,
            dropped_message_count: usage.dropped_message_count,
        }
    }

    /// Renders one gateway observation from IR usage, zero-filling every
    /// breakdown.
    fn from_ir(usage: IrUsage) -> Self {
        Self {
            input_tokens: usage.input_tokens,
            input_tokens_details: CompactInputTokensDetails {
                cached_tokens: usage.cached_tokens.unwrap_or(0),
                cache_write_tokens: usage.cache_write_tokens.unwrap_or(0),
            },
            output_tokens: usage.output_tokens,
            output_tokens_details: CompactOutputTokensDetails {
                reasoning_tokens: usage.reasoning_tokens.unwrap_or(0),
            },
            total_tokens: usage.input_tokens + usage.output_tokens,
            dropped_message_count: None,
        }
    }
}

/// Renders a native compacted upstream result into the compact endpoint's
/// success body: the upstream's identity, creation time, complete ordered
/// output, and usage — with its `dropped_message_count` — carried through.
pub(crate) fn render_compacted_response(native: CompactedResponseNative) -> CompactedResponseBody {
    CompactedResponseBody {
        id: native
            .id
            .unwrap_or_else(|| format!("resp_{}", ids::new_uuid())),
        object: COMPACTED_RESPONSE_OBJECT,
        created_at: native.created_at.unwrap_or_else(unix_time::unix_now),
        output: native.output,
        usage: CompactUsage::from_upstream(native.usage.unwrap_or_default()),
    }
}

/// Renders a gateway-produced compaction into the compact endpoint's success
/// body: one `compaction` item carrying the sealed gateway carrier, with the
/// summary call's usage and zero-filled breakdowns.
pub(crate) fn render_gateway_compacted_response(
    carrier: String,
    usage: IrUsage,
) -> CompactedResponseBody {
    CompactedResponseBody {
        id: format!("resp_{}", ids::new_uuid()),
        object: COMPACTED_RESPONSE_OBJECT,
        created_at: unix_time::unix_now(),
        output: vec![compaction_item_json(None, carrier, None)],
        usage: CompactUsage::from_ir(usage),
    }
}

/// Renders the explicit trigger lifecycle's complete SSE frame sequence:
/// `response.created`, `response.in_progress`, one `output_item.added` and
/// `output_item.done` pair per prefix item, then `response.completed` carrying
/// `usage` — the framing Codex requires (README §6.8), rendered through
/// the ordinary [`SseRenderer`] so the envelope, item, and usage shapes cannot
/// drift from the ordinary lifecycle's.
pub(crate) fn render_trigger_stream_frames(
    model: String,
    prefix_items: Vec<Value>,
    usage: IrUsage,
) -> Vec<String> {
    let mut renderer = SseRenderer::new(model).with_prefix_items(prefix_items);

    let mut frames = renderer.render(&IrEvent::MessageStart {
        id: String::new(),
        model: String::new(),
    });

    frames.extend(renderer.render(&IrEvent::Usage(usage)));
    frames.extend(renderer.render(&IrEvent::Done));

    frames
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::expect_used,
        reason = "test fixtures parse and render values constructed to succeed"
    )]
    #![expect(
        clippy::panic,
        reason = "a failing fixture assertion is the test failure report"
    )]
    #![expect(
        clippy::indexing_slicing,
        reason = "every index follows a length assertion or constructs the indexed fixture"
    )]

    use super::{
        compaction_item_json, parse_compact_request, render_compacted_response,
        render_gateway_compacted_response, render_trigger_stream_frames,
    };
    use crate::{
        format::{
            CompactionControl, ParsedCompaction, ParsedRequest,
            openai::response::upstream_parse::{CompactedResponseNative, UpstreamUsage},
        },
        ir::{InvalidRequest, IrUsage},
    };
    use serde_json::{Value, json};

    /// Parses one compact request body given as JSON.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_compact_request(body.to_string().as_bytes())
    }

    /// The happy path: model, string input, and instructions parse into the
    /// standalone control, a non-streaming request, and the IR instructions.
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn compact_request_parses_to_the_standalone_control() {
        let parsed = parse(json!({
            "model": "m",
            "input": "hi",
            "instructions": "summarize",
        }))
        .expect("a compact request parses");

        assert_eq!(parsed.ir.model, "m");

        assert_eq!(parsed.ir.instructions.as_deref(), Some("summarize"));

        assert_eq!(parsed.ir.messages.len(), 1);

        assert!(!parsed.is_streaming);

        assert!(!parsed.should_include_usage);

        let Some(ParsedCompaction {
            control,
            boundaries,
            ..
        }) = parsed.compaction
        else {
            panic!("the compact request carries the standalone control");
        };

        assert!(matches!(
            control,
            Some(CompactionControl::ResponsesStandalone)
        ));

        assert!(boundaries.is_empty());
    }

    /// A present, non-null `previous_response_id` is rejected: the gateway is
    /// stateless and rejects stateful continuation instead of pretending it
    /// can retrieve the referenced history (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the rejection lands outside its documented wording.
    #[test]
    fn previous_response_id_is_rejected() {
        let error = parse(json!({
            "model": "m",
            "input": "hi",
            "previous_response_id": "resp_1",
        }))
        .expect_err("a stateful continuation must be rejected");

        assert_eq!(error.param.as_deref(), Some("previous_response_id"));

        // A null is the field's absence: the official body carries the member
        // nullable, and null must parse.

        let parsed = parse(json!({
            "model": "m",
            "input": "hi",
            "previous_response_id": null,
        }))
        .expect("a null previous_response_id parses");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// The accepted-and-ignored controls parse beside the supported ones,
    /// while a generation-only control or any other unknown member is
    /// rejected by the strict shape.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn caching_controls_are_ignored_and_unknown_members_rejected() {
        let parsed = parse(json!({
            "model": "m",
            "input": "hi",
            "prompt_cache_key": "cache-key",
            "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
            "prompt_cache_retention": "24h",
            "service_tier": "auto",
        }))
        .expect("the accepted-and-ignored controls parse");

        assert_eq!(parsed.ir.messages.len(), 1);

        for (field, value) in [
            ("tools", json!([])),
            ("stream", json!(false)),
            ("store", json!(false)),
            ("temperature", json!(0.5)),
            ("bogus", json!(1)),
        ] {
            let error = parse(json!({
                "model": "m",
                "input": "hi",
                field: value,
            }))
            .expect_err("a generation-only or unknown member must be rejected");

            assert!(
                error.to_string().contains(&format!("`{field}`")),
                "unexpected rejection for {field}: {error}"
            );
        }
    }

    /// A `compaction_trigger` item belongs to the ordinary endpoint: on the
    /// compact endpoint it is an ambiguous second control beside the route's
    /// own standalone control, so it is rejected.
    ///
    /// # Panics
    ///
    /// Panics if the trigger parses or the rejection misnames the field.
    #[test]
    fn compaction_trigger_is_rejected_on_the_compact_endpoint() {
        let error = parse(json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "compaction_trigger"},
            ],
        }))
        .expect_err("a trigger item must be rejected on the compact endpoint");

        assert_eq!(error.param.as_deref(), Some("input"));
    }

    /// The state vocabulary converts with exact offsets: a reasoning item
    /// carrying `encrypted_content` becomes a non-gateway state item, a native
    /// compaction item becomes a native boundary, and a gateway-namespaced
    /// carrier becomes a gateway boundary — each at the number of IR messages
    /// parsed before it, and each also riding `state_items`.
    ///
    /// # Panics
    ///
    /// Panics if the conversion diverges from the documented offsets.
    #[test]
    fn state_items_and_boundaries_convert_with_exact_offsets() {
        let parsed = parse(json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "first"},
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "thought"}],
                    "encrypted_content": "cipher",
                },
                {"type": "message", "role": "user", "content": "second"},
                {
                    "type": "compaction",
                    "id": "cmp_1",
                    "encrypted_content": "native-state",
                },
                {
                    "type": "compaction_summary",
                    "encrypted_content": "janusgate.compaction.v1.primary.abc",
                },
                {"type": "message", "role": "user", "content": "third"},
            ],
        }))
        .expect("the stateful compact body parses");

        assert_eq!(parsed.ir.messages.len(), 3);

        assert_eq!(parsed.ir.state_items.len(), 3);

        // Offsets: the reasoning item sat after one message, the two
        // compaction items after two.
        assert_eq!(parsed.ir.state_items[0].offset, 1);
        assert_eq!(parsed.ir.state_items[0].is_gateway, false);
        assert_eq!(parsed.ir.state_items[1].offset, 2);
        assert_eq!(parsed.ir.state_items[2].offset, 2);

        // The reasoning item replays with its member presence preserved.
        let reasoning = &parsed.ir.state_items[0].item;

        assert_eq!(reasoning.get("type"), Some(&json!("reasoning")));
        assert_eq!(reasoning.get("encrypted_content"), Some(&json!("cipher")));
        assert_eq!(reasoning.get("content"), None);

        // A native compaction item and a gateway carrier split by namespace.
        let Some(ParsedCompaction {
            control,
            boundaries,
            ..
        }) = parsed.compaction
        else {
            panic!("the compact request carries the standalone control");
        };

        assert!(matches!(
            control,
            Some(CompactionControl::ResponsesStandalone)
        ));

        assert_eq!(boundaries.len(), 2);

        assert!(matches!(
            boundaries[0],
            crate::format::CompactionBoundary::Native {
                offset: 2,
                artifact: crate::format::NativeArtifact::ResponsesCompactionItem,
            }
        ));

        match &boundaries[1] {
            crate::format::CompactionBoundary::Gateway {
                offset,
                carrier,
                visible_content,
            } => {
                assert_eq!(*offset, 2);

                assert_eq!(carrier, "janusgate.compaction.v1.primary.abc");

                assert_eq!(*visible_content, None);
            }

            other => panic!("the second boundary is the gateway carrier: {other:?}"),
        }

        // The gateway carrier rides the state items flagged as gateway, so no
        // render ever forwards it as native state.
        assert_eq!(parsed.ir.state_items[2].is_gateway, true);
    }

    /// A reasoning item without `encrypted_content` carries no replayable
    /// state and is dropped, exactly as on the ordinary endpoint.
    ///
    /// # Panics
    ///
    /// Panics if the item survives into the state items.
    #[test]
    fn plain_reasoning_items_are_dropped() {
        let parsed = parse(json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "thought"}]},
            ],
        }))
        .expect("a plain reasoning item parses");

        assert!(parsed.ir.state_items.is_empty());
    }

    /// The compaction item builder renders the official member order —
    /// `type`, `id`, `encrypted_content`, then `created_by` only when
    /// provided — so the outward item matches the protocol shape byte for
    /// byte.
    ///
    /// # Panics
    ///
    /// Panics if the built item diverges from the official member order.
    #[test]
    fn compaction_item_json_renders_the_official_member_order() {
        let plain = compaction_item_json(Some("cmp_1".to_string()), "state".to_string(), None);

        assert_eq!(
            plain,
            json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "state"}),
        );

        let attributed = compaction_item_json(
            Some("cmp_2".to_string()),
            "state".to_string(),
            Some("gateway".to_string()),
        );

        assert_eq!(
            attributed,
            json!({
                "type": "compaction",
                "id": "cmp_2",
                "encrypted_content": "state",
                "created_by": "gateway",
            }),
        );
    }

    /// A native compacted result renders through with its identity, creation
    /// time, complete output, and usage — the `dropped_message_count`
    /// included — and absent optionals zero-fill or mint rather than render
    /// as null (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the documented shape.
    #[test]
    fn native_compacted_results_render_through() {
        let body = render_compacted_response(CompactedResponseNative {
            id: Some("resp_native".to_string()),
            created_at: Some(1_000),
            output: vec![json!({"type": "message"}), json!({"type": "compaction"})],
            usage: Some(UpstreamUsage {
                input_tokens: 10,
                output_tokens: 4,
                input_tokens_details: None,
                output_tokens_details: None,
                dropped_message_count: Some(3),
            }),
        });

        let rendered = serde_json::to_value(&body).expect("the body serializes");

        assert_eq!(rendered.get("id"), Some(&json!("resp_native")));

        assert_eq!(rendered.get("object"), Some(&json!("response.compaction")));

        assert_eq!(rendered.get("created_at"), Some(&json!(1_000)));

        assert_eq!(
            rendered.get("output"),
            Some(&json!([{"type": "message"}, {"type": "compaction"}]))
        );

        assert_eq!(
            rendered.get("usage"),
            Some(&json!({
                "input_tokens": 10,
                "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0},
                "output_tokens": 4,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 14,
                "dropped_message_count": 3,
            })),
        );
    }

    /// A gateway-produced compaction renders one `compaction` item carrying
    /// the sealed carrier beside a gateway-minted `cmp_` identifier and the
    /// summary call's usage with zero-filled breakdowns — the shape of
    /// README §6.8's carrier.
    ///
    /// # Panics
    ///
    /// Panics if the rendered body diverges from the carrier shape.
    #[test]
    fn gateway_compacted_results_render_one_carrier_item() {
        let body = render_gateway_compacted_response(
            "sealed-carrier".to_string(),
            IrUsage {
                input_tokens: 5,
                output_tokens: 1,
                ..IrUsage::default()
            },
        );

        #[expect(
            clippy::expect_used,
            reason = "the rendered body is under test; a failing render is the failure"
        )]
        let rendered = serde_json::to_value(&body).expect("the body serializes");

        assert_eq!(rendered.get("object"), Some(&json!("response.compaction")));

        let output = rendered
            .get("output")
            .and_then(Value::as_array)
            .expect("the body carries its output");

        assert_eq!(output.len(), 1);

        let item = &output[0];

        assert_eq!(item.get("type"), Some(&json!("compaction")));

        assert_eq!(
            item.get("encrypted_content"),
            Some(&json!("sealed-carrier"))
        );

        assert!(
            item.get("id")
                .and_then(Value::as_str)
                .is_some_and(|id| id.starts_with("cmp_"))
        );

        assert_eq!(item.get("created_by"), None);

        assert_eq!(
            rendered.get("usage"),
            Some(&json!({
                "input_tokens": 5,
                "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0},
                "output_tokens": 1,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 6,
            })),
        );
    }

    /// The trigger lifecycle's stream is exactly the sequence Codex requires —
    /// `response.created`, `response.in_progress`, one added/done pair for the
    /// compaction item, then `response.completed` carrying the usage — with
    /// sequence numbers from zero and the same item in the terminal output as
    /// in the item events (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the rendered frames diverge from the required sequence.
    #[test]
    fn trigger_stream_frames_render_the_required_sequence() {
        let item = json!({
            "type": "compaction",
            "id": "cmp_1",
            "encrypted_content": "state",
        });

        let frames = render_trigger_stream_frames(
            "m".to_string(),
            vec![item.clone()],
            IrUsage {
                input_tokens: 9,
                output_tokens: 3,
                ..IrUsage::default()
            },
        );

        let payloads: Vec<Value> = frames
            .iter()
            .map(|frame| {
                let data = frame
                    .split_once("\ndata: ")
                    .expect("every frame carries a data line")
                    .1;

                serde_json::from_str(data.trim_end()).expect("frame data is rendered JSON")
            })
            .collect();

        let types: Vec<&str> = payloads
            .iter()
            .filter_map(|payload| payload.get("type").and_then(Value::as_str))
            .collect();

        assert_eq!(
            types,
            vec![
                "response.created",
                "response.in_progress",
                "response.output_item.added",
                "response.output_item.done",
                "response.completed",
            ],
        );

        // Sequence numbers run from zero without gaps.
        for (sequence, payload) in payloads.iter().enumerate() {
            assert_eq!(
                payload.get("sequence_number"),
                Some(&json!(sequence)),
                "sequence of frame {sequence}",
            );
        }

        // The item event and the terminal output carry the same artifact.
        assert_eq!(
            payloads[2].pointer("/item"),
            Some(&json!({"type": "compaction", "id": "cmp_1", "encrypted_content": "state"})),
        );

        let terminal = payloads
            .last()
            .expect("the sequence ends with response.completed");

        assert_eq!(terminal.pointer("/response/output"), Some(&json!([item])));

        assert_eq!(
            terminal.pointer("/response/status"),
            Some(&json!("completed"))
        );

        assert_eq!(
            terminal.pointer("/response/usage"),
            Some(&json!({
                "input_tokens": 9,
                "input_tokens_details": {"cached_tokens": 0, "cache_write_tokens": 0},
                "output_tokens": 3,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 12,
            })),
        );
    }
}
