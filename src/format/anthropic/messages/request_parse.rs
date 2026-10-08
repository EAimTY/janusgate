//! Client-request parsing for the Anthropic Messages codec: the inbound `POST /messages` body
//! deserialized into the IR request.
//!
//! The surface parses request bodies strictly into IR (`max_tokens` required per the spec,
//! streaming via body `stream: true`): unknown members are 400s, known semantic controls without
//! gateway support are 400s, and presentation-only controls are accepted-and-ignored
//! (README §6.6).

use super::{TypedBlock, block_opaque};
use crate::{
    compaction_model::{identity::InstructionRole, sealing::CARRIER_NAMESPACE},
    format::{
        BoundaryPlacement, CompactionBoundary, CompactionControl, NativeArtifact, ParsedCompaction,
        ParsedInstructionItem, ParsedRequest, RoleRun, StringOrArray, compaction_control,
        join_text_parts,
    },
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrRequest, IrResponseFormat, IrRole, IrTool,
        IrToolChoice, StateFamily, StateItem,
    },
    vocabulary::ReasoningLevel,
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

/// Strictly parsed `POST /messages` request body: unknown members are 400s, known semantic
/// controls without gateway support are 400s, and presentation-only controls are
/// accepted-and-ignored (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundRequest {
    /// Requested model, routed against the public model list.
    model: String,

    /// Output token budget, required per the spec.
    max_tokens: u64,

    /// Conversation turns.
    messages: Vec<InboundMessage>,

    /// System instructions: a plain string or an array of text blocks.
    #[serde(default)]
    system: Option<StringOrArray<InboundTextBlock>>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Sampling temperature, passed through verbatim; the upstream decides support.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold, passed through verbatim like `temperature`.
    #[serde(default)]
    top_p: Option<f64>,

    /// Top-of-distribution truncation, known-but-unsupported: the IR has no top-k slot, so
    /// present is a 400 naming the field (README §6.6).
    #[serde(default)]
    top_k: Option<IgnoredAny>,

    /// Cross-request diagnostics (`diagnostics.previous_message_id` foremost),
    /// known-but-unsupported: it references a prior response's message state, which a stateless
    /// gateway never carries, so present is a 400 naming the field (README §6.6).
    #[serde(default)]
    diagnostics: Option<IgnoredAny>,

    /// Inference-region routing constraint, known-but-unsupported: a stateless gateway cannot
    /// honor a region pin downstream, so present is a 400 naming the field (README §6.6).
    #[serde(default)]
    inference_geo: Option<IgnoredAny>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<InboundTool>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<InboundToolChoice>,

    /// Legacy thinking control: `disabled` parses into `none`, `enabled`/`adaptive` are
    /// accepted-and-ignored, and `budget_tokens` is a 400 naming the field.
    #[serde(default)]
    thinking: Option<InboundThinking>,

    /// Reasoning level and output-format container: `effort` parses into the closed shared
    /// vocabulary and `format` into the IR response format, each optional per the official
    /// contract.
    #[serde(default)]
    output_config: Option<InboundOutputConfig>,

    /// On-demand compaction control, the official envelope's `compaction` member.
    #[serde(default)]
    compaction: Option<InboundCompaction>,

    /// Threshold compaction control, the official envelope's `context_management` member.
    #[serde(default)]
    context_management: Option<InboundContextManagement>,

    /// Summary control, accepted and ignored: summaries never enter the IR.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    thinking_summaries: Option<IgnoredAny>,

    /// Request metadata (`metadata.user_id` foremost), accepted and ignored: the gateway is
    /// stateless and never persists request metadata (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    metadata: Option<IgnoredAny>,

    /// Capacity-tier selection, accepted and ignored: a stateless gateway has no tier to honor
    /// downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<IgnoredAny>,

    /// Container reuse identifier, accepted and ignored: the gateway is stateless and never
    /// chains containers (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    container: Option<IgnoredAny>,

    /// Top-level prompt-caching breakpoint, accepted and ignored: the stateless gateway renders
    /// no caching markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,
}

/// One inbound conversation turn, tagged by `role`.
#[derive(Debug, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase", deny_unknown_fields)]
enum InboundMessage {
    /// User turn carrying text, images, and tool results.
    User {
        /// Turn content: a plain string or an array of user blocks.
        content: StringOrArray<InboundUserBlock>,
    },

    /// Assistant turn carrying text, tool calls, and reasoning blocks.
    Assistant {
        /// Turn content: a plain string or an array of assistant blocks.
        content: StringOrArray<InboundAssistantBlock>,
    },
}

/// Content block of a user turn; a member outside the block's own shape is rejected. Placement
/// is enforced by the type itself: `tool_use` and thinking blocks are not user content, so they
/// fail as unknown variants rather than parsing into the wrong turn side.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundUserBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Image segment.
    Image {
        /// Inline data or remote URL source.
        source: InboundImageSource,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },

    /// Tool result segment.
    ToolResult {
        /// Identifier of the invocation being answered.
        tool_use_id: String,

        /// Result content: a plain string or an array of result parts.
        content: StringOrArray<InboundToolResultPart>,

        /// Whether the tool reported an error.
        #[serde(default)]
        is_error: Option<bool>,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },
}

/// Content block of an assistant turn; placement is enforced by the type itself like the user
/// blocks. `document` blocks parse only to be rejected as known-unsupported (README §6.6).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundAssistantBlock {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Tool invocation segment.
    ToolUse {
        /// Invocation identifier.
        id: String,

        /// Invoked function name.
        name: String,

        /// JSON arguments object; the fragment's shape is the caller's to define.
        input: Value,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },

    /// Reasoning segment with its upstream signature.
    Thinking {
        /// Reasoning text.
        thinking: String,

        /// Upstream signature terminating the block.
        #[serde(default)]
        signature: Option<String>,
    },

    /// Opaque redacted reasoning segment.
    RedactedThinking {
        /// Redacted payload, replayed verbatim on same-family upstreams.
        data: Value,
    },

    /// Compaction state block: a native block replayed from a compaction turn, or a gateway
    /// carrier recognizable by its namespaced `signature`. The block carries compaction state,
    /// never an IR part; the conversion records it beside the message loop.
    Compaction {
        /// The readable summary content; absent is the native null-content no-op block, which
        /// retains its no-op meaning and cannot authorize discarding history.
        #[serde(default)]
        content: Option<String>,

        /// The vendor signature, or the namespaced gateway carrier the gateway rendered.
        #[serde(default)]
        signature: Option<String>,

        /// Opaque vendor ciphertext, when the block carries one.
        #[serde(default)]
        encrypted_content: Option<String>,

        /// Tool-advertisement changes the compaction applied, when the block carries any.
        #[serde(default)]
        tool_changes: Option<Vec<Value>>,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,
    },

    /// Document segment, known-but-unsupported: the conversion rejects it, and any member
    /// beyond `source` fails even earlier as an unknown field.
    Document {
        /// Document source, ignored: the block is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Text block of the `system` array form.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTextBlock {
    /// Block type tag, always `text` for system content.
    #[serde(rename = "type")]
    kind: String,

    /// The instruction text.
    text: String,

    /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no caching
    /// markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,

    /// Citations replayed from a citations-enabled upstream turn, accepted and ignored: the
    /// stateless gateway renders no citations downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    citations: Option<IgnoredAny>,
}

/// Image source of an inbound image block.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundImageSource {
    /// Base64-encoded inline data.
    Base64 {
        /// MIME type of the image data.
        media_type: String,

        /// Base64-encoded image bytes.
        data: String,
    },

    /// Remote image referenced by URL.
    Url {
        /// Remote URL.
        url: String,
    },
}

/// Content part of a `tool_result` array: text flattens into the tool output, image content is
/// a 400 (the IR tool output has no media channel).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundToolResultPart {
    /// Text segment.
    Text {
        /// The text content.
        text: String,

        /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no
        /// caching markers downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        cache_control: Option<IgnoredAny>,

        /// Citations replayed from a citations-enabled upstream turn, accepted and ignored:
        /// the stateless gateway renders no citations downstream (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        citations: Option<IgnoredAny>,
    },

    /// Image segment, known-but-unsupported in tool results.
    Image {
        /// Image source, ignored: the part is rejected either way.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        source: Option<IgnoredAny>,
    },
}

/// Advertised function tool: function tools only — a server-side tool discriminator is a 400
/// naming `type`, and a server-side tool shape fails on its unknown members.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTool {
    /// Function name the model calls.
    name: String,

    /// Tool discriminator: `"custom"` marks a function tool; any other value names a
    /// server-side tool and is a 400 naming `type` (README §6.6).
    #[serde(default, rename = "type")]
    kind: Option<String>,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of the accepted arguments.
    input_schema: Value,

    /// Prompt-caching breakpoint, accepted and ignored: the stateless gateway renders no caching
    /// markers downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    cache_control: Option<IgnoredAny>,

    /// Schema-validation guarantee, accepted and ignored: the gateway exposes no such control
    /// downstream (README §6.6).
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    strict: Option<bool>,
}

/// Tool choice restriction: `disable_parallel_tool_use` is accepted and ignored.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundToolChoice {
    /// Choice type: `auto`, `any`, `tool`, or `none`.
    #[serde(rename = "type")]
    kind: String,

    /// Required function name for the `tool` type.
    #[serde(default)]
    name: Option<String>,

    /// Parallel tool-use switch, accepted and ignored: the gateway exposes no such control.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    disable_parallel_tool_use: Option<bool>,
}

/// Legacy thinking control — the official `ThinkingConfig` union: `disabled` carries its `type`
/// alone, while `enabled`/`adaptive` each add the display control, and only `enabled` adds the
/// legacy budget (README §6.4).
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundThinking {
    /// The native off spelling: parses into `none` (§6.4). An empty struct variant rather than a
    /// unit one: a unit variant would silently swallow unknown members instead of denying them.
    Disabled {},

    /// Accepted and ignored; its `budget_tokens` is the legacy numeric mechanism, a 400 naming
    /// the field, and its `display` never enters the IR.
    Enabled {
        /// Legacy numeric budget mechanism, rejected naming the field like Gemini
        /// `thinkingBudget`.
        #[serde(default)]
        budget_tokens: Option<IgnoredAny>,

        /// Display control, accepted and ignored: display never enters the IR.
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        display: Option<IgnoredAny>,
    },

    /// Accepted and ignored, like `enabled` minus the budget member the official variant lacks.
    Adaptive {
        /// Display control, accepted and ignored: display never enters the IR.
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        display: Option<IgnoredAny>,
    },
}

/// The synthesized name of a schema-bearing Anthropic response format: the Messages wire carries
/// no schema-name slot in `output_config.format`, so the IR's named-schema form carries this
/// stand-in — the Gemini surfaces' posture — and the Anthropic upstream render drops it again.
const ANTHROPIC_RESPONSE_SCHEMA_NAME: &str = "response";

/// Reasoning level container — the official `OutputConfig`, whose `effort` and `format` members
/// are each optional.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputConfig {
    /// Reasoning effort in the closed shared vocabulary; optional per the official contract, so
    /// the container may carry a format alone.
    #[serde(default)]
    effort: Option<String>,

    /// Structured-outputs schema, parsed into the IR response format.
    #[serde(default)]
    format: Option<InboundOutputFormat>,
}

/// The inbound `output_config.format` member — the official `JSONOutputFormat`, whose only
/// variant is `json_schema` with both members required. Malformed shapes are validated at the
/// conversion so the rejections name `format` instead of rendering serde's generic wording.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundOutputFormat {
    /// Format discriminator: `"json_schema"` is the official union's only member.
    #[serde(default, rename = "type")]
    kind: Option<String>,

    /// The JSON Schema of the response.
    #[serde(default)]
    schema: Option<Value>,
}

/// The on-demand compaction beta: required on a request carrying the on-demand control or a
/// leading native compaction block.
const ON_DEMAND_BETA: &str = "compact-2026-09-04";

/// The threshold compaction beta: required on a request carrying the threshold control or a
/// historical native compaction block.
const THRESHOLD_BETA: &str = "compact-2026-01-12";

/// Maximum character length of custom compaction instructions; a longer value is a 400 naming
/// `instructions`.
const MAX_COMPACTION_INSTRUCTIONS_CHARS: usize = 16_384;

/// Minimum input-token threshold of a `compact_20260112` trigger; a smaller value is a 400
/// naming `trigger`, never a clamp.
const MIN_TRIGGER_TOKENS: u64 = 50_000;

/// The on-demand compaction control — the official request envelope's `compaction` member.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundCompaction {
    /// Control discriminator: `"summarize"` is the only supported operation; any other value is
    /// a 400 naming `type`.
    #[serde(rename = "type")]
    kind: String,

    /// Custom summarization instructions replacing the summarizer's default prompt.
    #[serde(default)]
    instructions: Option<String>,
}

/// The threshold compaction control — the official request envelope's `context_management`
/// member.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundContextManagement {
    /// The requested context edits, in order.
    edits: Vec<InboundContextEdit>,
}

/// One context-management edit. `compact_20260112` is the only supported edit; every other
/// discriminator — the `clear_tool_uses_20250919` and `clear_thinking_20251015` edits included —
/// parses into [`InboundContextEdit::Unsupported`] and is rejected naming `type`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InboundContextEdit {
    /// Threshold compaction edit.
    #[serde(rename = "compact_20260112")]
    Compact {
        /// Input-token trigger; absent or null keeps the protocol's 150,000-token default,
        /// recorded as [`None`](Option).
        #[serde(default)]
        trigger: Option<InboundTrigger>,

        /// Whether the caller requested pausing after the compaction instead of continuing
        /// generation; absent keeps the protocol default `false`.
        #[serde(default)]
        pause_after_compaction: Option<bool>,

        /// Custom summarization instructions; null and whitespace-only mean none.
        #[serde(default)]
        instructions: Option<String>,
    },

    /// Any other edit discriminator, rejected naming `type` at the conversion.
    #[serde(other)]
    Unsupported,
}

/// The input-token trigger of a `compact_20260112` edit.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InboundTrigger {
    /// Trigger discriminator: `"input_tokens"` is the only supported trigger; any other value is
    /// a 400 naming `trigger.type`.
    #[serde(rename = "type")]
    kind: String,

    /// The threshold in input tokens; below the documented minimum is a 400, never a clamp.
    value: u64,
}

/// The vendor-shaped compaction block preserved verbatim into a state item: the same members
/// with the same presence, no absent member rendered as null.
#[derive(Debug, Serialize)]
struct CompactionBlockState<'a> {
    /// Always `compaction`.
    #[serde(rename = "type")]
    kind: &'static str,

    /// The readable summary content, when the block carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<&'a String>,

    /// The signature, when the block carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<&'a String>,

    /// The opaque ciphertext, when the block carries one.
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted_content: Option<&'a String>,

    /// The tool changes, when the block carries any.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_changes: Option<&'a Vec<Value>>,
}

/// Compaction facts collected while converting the inbound history: the ordered boundaries and
/// the verbatim state items beside them, plus the compaction block count for the one-block rule.
#[derive(Debug, Default)]
struct CollectedCompaction {
    /// Compaction state markers in history order.
    boundaries: Vec<CompactionBoundary>,

    /// Verbatim state items at their offsets.
    state_items: Vec<StateItem>,

    /// Number of compaction blocks seen so far.
    block_count: usize,
}

impl CollectedCompaction {
    /// Records one compaction content block at its history position: a namespaced signature
    /// marks a gateway carrier, every other signature — an absent one included — a native block
    /// whose replay placement follows its position (first content block of the first message is
    /// leading, a block anywhere else is historical).
    ///
    /// # Errors
    ///
    /// Returns [`InvalidRequest`] naming `content` when the block is the second compaction block
    /// of the request: the protocol allows exactly one per request.
    ///
    /// # Panics
    ///
    /// Panics if the first-party state block fails to serialize, which cannot occur.
    fn record_block(
        &mut self,
        content: &Option<String>,
        signature: &Option<String>,
        encrypted_content: &Option<String>,
        tool_changes: &Option<Vec<Value>>,
        is_leading: bool,
        offset: usize,
    ) -> Result<(), InvalidRequest> {
        self.block_count += 1;

        if self.block_count > 1 {
            return Err(InvalidRequest::field(
                "content",
                "at most one compaction block is allowed per request",
            ));
        }

        let state = CompactionBlockState {
            kind: "compaction",
            content: content.as_ref(),
            signature: signature.as_ref(),
            encrypted_content: encrypted_content.as_ref(),
            tool_changes: tool_changes.as_ref(),
        };

        #[expect(
            clippy::expect_used,
            reason = "the state block is a first-party type whose serialization is total"
        )]
        let item = serde_json::to_value(&state).expect("first-party wire types serialize");

        match signature {
            Some(carrier) if carrier.starts_with(CARRIER_NAMESPACE) => {
                self.boundaries.push(CompactionBoundary::Gateway {
                    offset,
                    carrier: carrier.clone(),
                    visible_content: content.clone(),
                });

                self.state_items.push(StateItem {
                    offset,
                    item,
                    is_gateway: true,
                    is_compaction: true,
                    family: StateFamily::Messages,
                });
            }

            _ => {
                self.boundaries.push(CompactionBoundary::Native {
                    offset,
                    artifact: NativeArtifact::MessagesCompactionBlock {
                        is_null_no_op: content.is_none(),
                        placement: if is_leading {
                            BoundaryPlacement::Leading
                        } else {
                            BoundaryPlacement::Historical
                        },
                    },
                });

                self.state_items.push(StateItem {
                    offset,
                    item,
                    is_gateway: false,
                    is_compaction: true,
                    family: StateFamily::Messages,
                });
            }
        }

        Ok(())
    }
}

/// Normalizes custom compaction instructions: null and whitespace-only mean none, and an
/// instruction longer than the documented maximum is a 400 naming `instructions`.
///
/// # Errors
///
/// Returns [`InvalidRequest`] when the instructions exceed
/// [`MAX_COMPACTION_INSTRUCTIONS_CHARS`].
fn normalize_instructions(instructions: Option<&str>) -> Result<Option<String>, InvalidRequest> {
    let Some(instructions) = instructions else {
        return Ok(None);
    };

    if instructions.chars().count() > MAX_COMPACTION_INSTRUCTIONS_CHARS {
        return Err(InvalidRequest::field(
            "instructions",
            format!(
                "compaction instructions must not exceed \
                 {MAX_COMPACTION_INSTRUCTIONS_CHARS} characters"
            ),
        ));
    }

    Ok((!instructions.trim().is_empty()).then(|| instructions.to_string()))
}

/// Validates one inbound beta requirement: the caller's `anthropic-beta` entries must carry the
/// named beta exactly as spelled.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `anthropic-beta` when the entries carry no such beta.
fn require_beta(betas: &[String], beta: &str) -> Result<(), InvalidRequest> {
    if betas.iter().any(|candidate| candidate == beta) {
        return Ok(());
    }

    Err(InvalidRequest::field(
        "anthropic-beta",
        format!("The anthropic-beta header must carry `{beta}` for this compaction request"),
    ))
}

/// Parses the two compaction controls of the inbound envelope into their typed forms: the
/// on-demand `compaction` member and the threshold `context_management` member, validating each
/// control's own contract (type discriminators, trigger minimum, instruction bounds).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for an unsupported compaction type (naming `type`), an
/// unsupported trigger type (naming `trigger.type`), a sub-minimum trigger (naming `trigger`),
/// an unsupported edit type (naming `type`), and out-of-range instructions (naming
/// `instructions`).
fn parse_compaction_controls(
    request: &InboundRequest,
) -> Result<Option<CompactionControl>, InvalidRequest> {
    let on_demand = match &request.compaction {
        None => None,

        Some(compaction) => {
            if compaction.kind != "summarize" {
                return Err(InvalidRequest::field(
                    "type",
                    format!(
                        "Unsupported compaction type `{}`; only `summarize` is supported",
                        compaction.kind
                    ),
                ));
            }

            Some(CompactionControl::MessagesOnDemand {
                instructions: normalize_instructions(compaction.instructions.as_deref())?,
            })
        }
    };

    let threshold = match &request.context_management {
        None => None,

        Some(management) => {
            let mut edits = Vec::with_capacity(management.edits.len());

            for edit in &management.edits {
                match edit {
                    InboundContextEdit::Compact {
                        trigger,
                        pause_after_compaction,
                        instructions,
                    } => {
                        if let Some(trigger) = trigger {
                            if trigger.kind != "input_tokens" {
                                return Err(InvalidRequest::field(
                                    "trigger.type",
                                    format!(
                                        "Unsupported compaction trigger type `{}`; only \
                                         `input_tokens` is supported",
                                        trigger.kind
                                    ),
                                ));
                            }

                            if trigger.value < MIN_TRIGGER_TOKENS {
                                return Err(InvalidRequest::field(
                                    "trigger",
                                    format!(
                                        "compaction trigger threshold must be at least \
                                         {MIN_TRIGGER_TOKENS} input tokens"
                                    ),
                                ));
                            }
                        }

                        edits.push(compaction_control::ThresholdEdit {
                            trigger: trigger.as_ref().map(|trigger| trigger.value),
                            pause_after_compaction: pause_after_compaction.unwrap_or(false),
                            instructions: normalize_instructions(instructions.as_deref())?,
                        });
                    }

                    InboundContextEdit::Unsupported => {
                        return Err(InvalidRequest::field(
                            "type",
                            "Unsupported context_management edit type; only `compact_20260112` \
                             is supported (`clear_tool_uses_20250919` and \
                             `clear_thinking_20251015` are not supported)",
                        ));
                    }
                }
            }

            Some(CompactionControl::MessagesThreshold { edits })
        }
    };

    // The two controls are mutually exclusive on the native contract (README §6.8).
    if on_demand.is_some() && threshold.is_some() {
        return Err(InvalidRequest::field(
            "compaction",
            "compaction and context_management are mutually exclusive",
        ));
    }

    Ok(on_demand.or(threshold))
}

/// Parses a `POST /messages` request body into IR. `betas` carries the caller's `anthropic-beta`
/// entries (README §6.5 route context), validated against the inbound beta requirements of the
/// compaction controls and compaction blocks the request carries.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON and for malformed, unknown, or unsupported
/// members and values, and for a compaction control or compaction block whose required
/// `anthropic-beta` entry is missing.
pub(super) fn parse_inbound_request(
    body: &[u8],
    betas: &[String],
) -> Result<ParsedRequest, InvalidRequest> {
    let mut request: InboundRequest = crate::format::parse_client_request(body)?;

    if request.messages.is_empty() {
        let message = if request.compaction.is_some() {
            "compaction requires a non-empty message history: nothing to summarize"
        } else {
            "messages must contain at least one message"
        };

        return Err(InvalidRequest::field("messages", message));
    }

    // Known-but-unsupported sampling control: the IR carries no top-k slot, so present is a
    // 400 naming the field rather than the generic unknown-field rejection (README §6.6).
    if request.top_k.is_some() {
        return Err(InvalidRequest::field("top_k", "top_k is not supported"));
    }

    // Known-but-unsupported envelope controls: both are 400s naming their field rather than
    // generic unknown-field rejections (README §6.6).
    if request.diagnostics.is_some() {
        return Err(InvalidRequest::field(
            "diagnostics",
            "diagnostics is not supported; the gateway carries no cross-request message state",
        ));
    }

    if request.inference_geo.is_some() {
        return Err(InvalidRequest::field(
            "inference_geo",
            "inference_geo is not supported; the gateway cannot pin inference to a region",
        ));
    }

    let compaction_control = parse_compaction_controls(&request)?;

    let instruction_items = match &request.system {
        None => Vec::new(),

        Some(StringOrArray::Text(system)) => {
            vec![ParsedInstructionItem {
                role: InstructionRole::System,
                text: system.clone(),
            }]
        }

        Some(StringOrArray::Array(blocks)) => {
            for block in blocks {
                if block.kind != "text" {
                    return Err(InvalidRequest::field(
                        "system",
                        "system blocks must be text",
                    ));
                }
            }

            blocks
                .iter()
                .map(|block| ParsedInstructionItem {
                    role: InstructionRole::System,
                    text: block.text.clone(),
                })
                .collect::<Vec<_>>()
        }
    };

    // The instructions string joins the parsed items: an absent `system` yields no items and no
    // instructions, while a present one yields at least its single text item, whose verbatim
    // content — blank included — is the instructions string.
    let instructions = (!instruction_items.is_empty()).then(|| {
        instruction_items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    });

    let mut messages: Vec<IrMessage> = Vec::new();
    let mut compaction_state = CollectedCompaction::default();

    for (message_index, message) in request.messages.iter().enumerate() {
        match message {
            InboundMessage::User { content } => {
                // A user turn may mix text/images with tool results; the IR separates the sides,
                // so consecutive same-side parts chunk into one IR message each, in order.
                let mut run = RoleRun::default();

                let parts = match content {
                    StringOrArray::Text(text) => vec![IrPart::Text { text: text.clone() }],

                    StringOrArray::Array(blocks) => {
                        let mut converted = Vec::with_capacity(blocks.len());

                        for block in blocks {
                            converted.push(user_block_to_part(block)?);
                        }

                        converted
                    }
                };

                for part in parts {
                    let role = if matches!(part, IrPart::ToolOutput { .. }) {
                        IrRole::Tool
                    } else {
                        IrRole::User
                    };

                    run.push(role, vec![part]);
                }

                messages.extend(run.finish());
            }

            InboundMessage::Assistant { content } => {
                // A compaction block carries compaction state, never an IR part: the conversion
                // records it beside the loop, at the offset of the IR messages converted before
                // the turn holding it, and places it leading exactly at the first content block
                // of the first message.
                let mut parts = Vec::new();

                match content {
                    StringOrArray::Text(text) => parts.push(IrPart::Text { text: text.clone() }),

                    StringOrArray::Array(blocks) => {
                        for (block_index, block) in blocks.iter().enumerate() {
                            if let InboundAssistantBlock::Compaction {
                                content,
                                signature,
                                encrypted_content,
                                tool_changes,
                                ..
                            } = block
                            {
                                compaction_state.record_block(
                                    content,
                                    signature,
                                    encrypted_content,
                                    tool_changes,
                                    message_index == 0 && block_index == 0,
                                    messages.len(),
                                )?;

                                continue;
                            }

                            parts.push(assistant_block_to_part(block)?);
                        }
                    }
                }

                // A turn whose blocks all carry state — a compaction-block-only replay turn —
                // or an empty content array yields no parts and no IR message, mirroring the
                // user branch: an empty message is IR noise every consumer skips.
                if !parts.is_empty() {
                    messages.push(IrMessage {
                        role: IrRole::Assistant,
                        parts,
                    });
                }
            }
        }
    }

    let mut tools = Vec::new();

    // The deserialized request is owned and consumed in place: taking the tools moves each
    // one's members into the IR without cloning.
    for tool in request.tools.take().unwrap_or_default() {
        // A non-`custom` discriminator names a server-side tool: function tools only, and
        // the rejection names `type` (README §6.6).
        if let Some(other) = tool.kind.as_deref().filter(|kind| *kind != "custom") {
            return Err(InvalidRequest::field(
                "type",
                format!("Unsupported tool type `{other}`; function tools only"),
            ));
        }

        if !tool.input_schema.is_object() {
            return Err(InvalidRequest::field(
                "input_schema",
                "Tool input_schema must be a JSON object",
            ));
        }

        tools.push(IrTool {
            name: tool.name,
            description: tool.description,
            parameters: Some(tool.input_schema),
        });
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            crate::format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice.kind.as_str() {
                "auto" => IrToolChoice::Auto,
                "none" => IrToolChoice::None,
                "any" => IrToolChoice::Required,

                "tool" => match &choice.name {
                    Some(name) => IrToolChoice::Function { name: name.clone() },

                    None => {
                        return Err(InvalidRequest::field(
                            "tool_choice",
                            "tool_choice `tool` requires a `name`",
                        ));
                    }
                },

                other => {
                    return Err(InvalidRequest::field(
                        "tool_choice",
                        format!("Unknown tool_choice `{other}`"),
                    ));
                }
            })
        }
    };

    // Forced tool use contradicts the on-demand contract (README §6.8).
    if compaction_control.is_some()
        && matches!(
            tool_choice,
            Some(IrToolChoice::Required) | Some(IrToolChoice::Function { .. })
        )
    {
        return Err(InvalidRequest::field(
            "tool_choice",
            "compaction does not support forced tool use (`any` or `tool`)",
        ));
    }

    // The legacy numeric budget mechanism survives parsing only on the variant where the
    // official member exists: a 400 naming the field, like Gemini `thinkingBudget` (README §6.6).
    if let Some(InboundThinking::Enabled {
        budget_tokens: Some(_),
        ..
    }) = &request.thinking
    {
        return Err(InvalidRequest::field(
            "budget_tokens",
            "budget_tokens is not supported; use output_config.effort",
        ));
    }

    let thinking_effort = match &request.thinking {
        None => None,

        // The native off spelling parses into `none` (§6.4).
        Some(InboundThinking::Disabled {}) => Some(ReasoningLevel::None),

        // Accepted and ignored: no replayable state, no IR slot.
        Some(InboundThinking::Enabled { .. } | InboundThinking::Adaptive { .. }) => None,
    };

    let output_effort = request
        .output_config
        .as_ref()
        .and_then(|output_config| output_config.effort.as_deref())
        .map(|effort| ReasoningLevel::parse(effort, "output_config"))
        .transpose()?;

    // The structured-outputs member parses into the IR response format; a shape outside the
    // official `JSONOutputFormat` union — any other `type`, a missing or non-object `schema` —
    // is a 400 naming `format` (README §6.6).
    let response_format = match &request.output_config {
        None => None,

        Some(output_config) => match &output_config.format {
            None => None,

            Some(format) => {
                if format.kind.as_deref() != Some("json_schema") {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format requires type `json_schema`",
                    ));
                }

                let Some(schema) = format.schema.as_ref() else {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format `json_schema` requires a `schema`",
                    ));
                };

                if !schema.is_object() {
                    return Err(InvalidRequest::field(
                        "format",
                        "output format schema must be a JSON object",
                    ));
                }

                Some(IrResponseFormat::JsonSchema {
                    name: ANTHROPIC_RESPONSE_SCHEMA_NAME.to_string(),
                    schema: schema.clone(),
                })
            }
        },
    };

    // A disabled legacy control contradicts an explicit effort level; an ignored legacy control
    // beside one is simply redundant.
    if thinking_effort.is_some() && output_effort.is_some() {
        return Err(InvalidRequest::field(
            "thinking",
            "thinking and output_config.effort are mutually exclusive",
        ));
    }

    // A structured-output format contradicts the on-demand contract (README §6.8).
    if compaction_control.is_some() && response_format.is_some() {
        return Err(InvalidRequest::field(
            "format",
            "compaction does not support output_config.format",
        ));
    }

    // A threshold edit cannot run on a request carrying a native compaction block; a gateway
    // carrier block is not native state and does not trigger the rule.
    if matches!(
        compaction_control,
        Some(CompactionControl::MessagesThreshold { .. })
    ) && compaction_state
        .boundaries
        .iter()
        .any(|boundary| matches!(boundary, CompactionBoundary::Native { .. }))
    {
        return Err(InvalidRequest::field(
            "context_management",
            "compact_20260112 cannot run on a request carrying a native compaction block",
        ));
    }

    // The inbound beta requirements, validated independently from any outbound requirement: the
    // on-demand beta covers the on-demand control and a leading native block, the threshold beta
    // the threshold control and a historical native block. Gateway carrier blocks follow their
    // source mechanism's contract at preparation, not here.
    let is_leading_block = |boundary: &CompactionBoundary| {
        matches!(
            boundary,
            CompactionBoundary::Native {
                artifact: NativeArtifact::MessagesCompactionBlock {
                    placement: BoundaryPlacement::Leading,
                    ..
                },
                ..
            }
        )
    };

    // On-demand compaction never fabricates a tool result: a request whose
    // final assistant turn ends in an unresolved tool call is rejected instead
    // of summarized (the API's own contract). A tool result after the call
    // resolves it, and a final user message is otherwise valid history.
    if matches!(
        compaction_control,
        Some(CompactionControl::MessagesOnDemand { .. })
    ) && let Some(last_assistant) = messages
        .iter()
        .rfind(|message| message.role == IrRole::Assistant)
        && let Some(IrPart::ToolCall { call_id, .. }) = last_assistant.parts.last()
        && !messages.iter().any(|message| {
            message.role == IrRole::Tool
                && message.parts.iter().any(|part| {
                    matches!(part, IrPart::ToolOutput { call_id: answered, .. } if answered == call_id)
                })
        })
    {
        return Err(InvalidRequest::field(
            "messages",
            "on-demand compaction cannot summarize a request whose final tool call has no result",
        ));
    }

    if matches!(
        compaction_control,
        Some(CompactionControl::MessagesOnDemand { .. })
    ) || compaction_state.boundaries.iter().any(is_leading_block)
    {
        require_beta(betas, ON_DEMAND_BETA)?;
    }

    if matches!(
        compaction_control,
        Some(CompactionControl::MessagesThreshold { .. })
    ) || compaction_state.boundaries.iter().any(|boundary| {
        matches!(
            boundary,
            CompactionBoundary::Native {
                artifact: NativeArtifact::MessagesCompactionBlock {
                    placement: BoundaryPlacement::Historical,
                    ..
                },
                ..
            }
        )
    }) {
        require_beta(betas, THRESHOLD_BETA)?;
    }

    let compaction = if compaction_control.is_some() || !compaction_state.boundaries.is_empty() {
        Some(ParsedCompaction {
            control: compaction_control,
            boundaries: compaction_state.boundaries,
            instruction_items,
        })
    } else {
        None
    };

    Ok(ParsedRequest {
        affinity: crate::format::affinity::Sources::from_wire_messages(
            crate::format::UpstreamFormat::AnthropicMessages,
            &messages,
        ),
        ir: IrRequest {
            has_compaction_trigger: false,
            state_items: compaction_state.state_items,
            model: request.model,
            instructions,
            messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: Some(request.max_tokens),
            response_format,
            reasoning_effort: output_effort.or(thinking_effort),
        },
        is_streaming: request.stream.unwrap_or(false),
        // Usage rides every Anthropic response unconditionally; no opt-in chunk exists.
        should_include_usage: false,
        compaction,
    })
}

/// Converts one user content block into its IR part.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for image content inside tool results.
fn user_block_to_part(block: &InboundUserBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundUserBlock::Text { text, .. } => Ok(IrPart::Text { text: text.clone() }),

        InboundUserBlock::Image { source, .. } => Ok(IrPart::Image {
            image: match source {
                InboundImageSource::Base64 { media_type, data } => IrImage::Inline {
                    mime_type: media_type.clone(),
                    data_base64: data.clone(),
                },

                InboundImageSource::Url { url } => IrImage::Url(url.clone()),
            },
        }),

        InboundUserBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            let text = match content {
                StringOrArray::Text(text) => text.clone(),

                StringOrArray::Array(parts) => {
                    let texts = parts
                        .iter()
                        .map(|part| match part {
                            InboundToolResultPart::Text { text, .. } => Ok(text.as_str()),

                            InboundToolResultPart::Image { .. } => Err(InvalidRequest::field(
                                "content",
                                "Image content inside tool results is not supported",
                            )),
                        })
                        .collect::<Result<Vec<_>, _>>()?;

                    // The shared text-part flattening contract (README §6.6): a leading run of
                    // empty parts carries no separator, and the rest joins on the blank line.
                    join_text_parts(texts)
                }
            };

            Ok(IrPart::ToolOutput {
                call_id: tool_use_id.clone(),
                content: text,
                is_error: is_error.unwrap_or(false),
            })
        }
    }
}

/// Converts one assistant content block into its IR part: thinking blocks keep their verbatim
/// replay payload so same-family upstreams receive them byte-exact.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for `document` blocks and for tool arguments that do not encode a
/// JSON object.
///
/// # Panics
///
/// Panics on a compaction block, which the message loop never converts: it records the block
/// beside the history conversion instead.
fn assistant_block_to_part(block: &InboundAssistantBlock) -> Result<IrPart, InvalidRequest> {
    match block {
        InboundAssistantBlock::Text { text, .. } => Ok(IrPart::Text { text: text.clone() }),

        InboundAssistantBlock::ToolUse {
            id, name, input, ..
        } => {
            if !input.is_object() {
                return Err(InvalidRequest::field(
                    "input",
                    "Tool use input must be a JSON object",
                ));
            }

            // The input is a proven object, so its rendered arguments are one by construction;
            // no re-validation of the rendered text.
            let arguments = crate::format::render_json(input);

            Ok(IrPart::ToolCall {
                call_id: id.clone(),
                name: name.clone(),
                arguments,
                thought_signature: None,
            })
        }

        InboundAssistantBlock::Thinking {
            thinking,
            signature,
        } => {
            let block = TypedBlock::Thinking {
                thinking: thinking.clone(),
                signature: signature.clone(),
            };

            Ok(IrPart::Reasoning {
                text: thinking.clone(),
                signature: signature.clone(),
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::RedactedThinking { data } => {
            let block = TypedBlock::RedactedThinking { data: data.clone() };

            Ok(IrPart::Reasoning {
                text: String::new(),
                signature: None,
                opaque: block_opaque(&block),
            })
        }

        InboundAssistantBlock::Document { .. } => Err(InvalidRequest::field(
            "content",
            "document blocks are not supported",
        )),

        #[expect(
            clippy::unreachable,
            reason = "the message loop records compaction blocks before this conversion and \
                      never converts one"
        )]
        InboundAssistantBlock::Compaction { .. } => {
            unreachable!("compaction blocks are recorded by the message loop, never converted")
        }
    }
}
