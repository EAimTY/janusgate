//! Client-request parsing for the Anthropic Messages codec: the inbound `POST /messages` body
//! deserialized into the IR request.
//!
//! The surface parses request bodies strictly into IR (`max_tokens` required per the spec,
//! streaming via body `stream: true`): unknown members are 400s, known semantic controls without
//! gateway support are 400s, and presentation-only controls are accepted-and-ignored
//! (README §6.6).

use super::{TypedBlock, block_opaque};
use crate::{
    compaction::{identity::InstructionRole, sealing::CARRIER_NAMESPACE},
    format::{
        BoundaryPlacement, CompactionBoundary, CompactionControl, NativeArtifact, ParsedCompaction,
        ParsedInstructionItem, ParsedRequest, RoleRun, StringOrArray, compaction_control,
    },
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice, StateItem,
    },
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
    let request: InboundRequest = crate::format::parse_client_request(body)?;

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

    let instructions = match &request.system {
        None => None,
        Some(_) => {
            let combined = instruction_items
                .iter()
                .map(|item| item.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n");

            Some(combined)
        }
    };

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

                messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts,
                });
            }
        }
    }

    let mut tools = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
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
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: Some(tool.input_schema.clone()),
            });
        }
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
        Some(InboundThinking::Disabled {}) => Some(IrReasoningLevel::None),

        // Accepted and ignored: no replayable state, no IR slot.
        Some(InboundThinking::Enabled { .. } | InboundThinking::Adaptive { .. }) => None,
    };

    let output_effort = request
        .output_config
        .as_ref()
        .and_then(|output_config| output_config.effort.as_deref())
        .map(|effort| IrReasoningLevel::parse(effort, "output_config"))
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

                    // The separator only ever separates text the accumulator already holds, so
                    // a leading run of empty parts carries none: skip it, then join the rest
                    // with the blank-line separator.
                    texts
                        .iter()
                        .skip_while(|text| text.is_empty())
                        .copied()
                        .collect::<Vec<&str>>()
                        .join("\n\n")
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// Reads one parsed message part by position.
    ///
    /// # Panics
    ///
    /// Panics if the position is absent: a missing part is the failure under test.
    fn part(parts: &[IrPart], index: usize) -> &IrPart {
        #[expect(
            clippy::expect_used,
            reason = "parsed messages are under test; a missing part is the failure"
        )]
        parts
            .get(index)
            .expect("the parsed message carries the expected part")
    }
    /// Sets one member of a JSON object fixture body.
    ///
    /// # Panics
    ///
    /// Panics if the body is not an object, which cannot occur for the fixtures.
    fn set_member(body: &mut Value, key: &str, value: Value) {
        #[expect(
            clippy::expect_used,
            reason = "fixture bodies are objects by construction"
        )]
        body.as_object_mut()
            .expect("fixture bodies are JSON objects")
            .insert(key.to_string(), value);
    }
    /// Parses one inbound body covering every content shape: system string, text and both image
    /// sources, tool calls with object inputs, error tool results, and thinking blocks whose
    /// opaque payload replays byte-exact upstream.
    ///
    /// # Panics
    ///
    /// Panics if the parsed IR diverges from the documented mapping.
    #[test]
    fn parses_every_inbound_content_shape() {
        let body = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "stream": true,
            "system": "be terse",
            "temperature": 0.7,
            "top_p": 0.9,
            "output_config": {"effort": "high"},
            "tools": [{
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
            }],
            "tool_choice": {"type": "tool", "name": "get_weather"},
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "image", "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": "aGVsbG8=",
                    }},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                    }},
                ]},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "check", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "enc"},
                    {"type": "text", "text": "looking"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": "sunny", "is_error": true},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the fixture body is constructed to parse"
        )]
        let parsed = parse_inbound_request(body.to_string().as_bytes(), &[])
            .expect("every content shape parses");

        assert!(parsed.is_streaming);
        assert_eq!(parsed.ir.model, "claude-test");
        assert_eq!(parsed.ir.max_output_tokens, Some(128));
        assert_eq!(parsed.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(parsed.ir.temperature, Some(0.7));
        assert_eq!(parsed.ir.top_p, Some(0.9));
        assert_eq!(parsed.ir.reasoning_effort, Some(IrReasoningLevel::High),);
        assert_eq!(
            parsed.ir.tools,
            vec![IrTool {
                name: "get_weather".to_string(),
                description: Some("City weather".to_string()),
                parameters: Some(json!({"type": "object"})),
            }],
        );
        assert_eq!(
            parsed.ir.tool_choice,
            Some(IrToolChoice::Function {
                name: "get_weather".to_string(),
            }),
        );

        assert_eq!(parsed.ir.messages.len(), 3);

        #[expect(clippy::expect_used, reason = "the fixture carries three messages")]
        let user = parsed.ir.messages.first().expect("the user message parses");

        assert_eq!(user.role, IrRole::User);
        assert_eq!(user.parts.len(), 3);
        assert!(matches!(
            part(&user.parts, 1),
            IrPart::Image {
                image: IrImage::Inline { .. },
            },
        ));
        assert_eq!(
            part(&user.parts, 2),
            &IrPart::Image {
                image: IrImage::Url("https://example.com/i.png".to_string()),
            },
        );

        #[expect(
            clippy::expect_used,
            reason = "the fixture carries an assistant message"
        )]
        let assistant = parsed
            .ir
            .messages
            .get(1)
            .expect("the assistant message parses");

        assert_eq!(assistant.parts.len(), 4);
        assert_eq!(
            part(&assistant.parts, 2),
            &IrPart::Text {
                text: "looking".to_string(),
            },
        );
        assert_eq!(
            part(&assistant.parts, 3),
            &IrPart::ToolCall {
                call_id: "call_1".to_string(),
                name: "get_weather".to_string(),
                arguments: r#"{"city":"Paris"}"#.to_string(),
                thought_signature: None,
            },
        );

        // The thinking block keeps its verbatim replay payload: the opaque value equals the
        // inbound block exactly.
        assert_eq!(
            part(&assistant.parts, 0),
            &IrPart::Reasoning {
                text: "check".to_string(),
                signature: Some("sig".to_string()),
                opaque: json!({"type": "thinking", "thinking": "check",
                                "signature": "sig"}),
            },
        );

        #[expect(clippy::expect_used, reason = "the fixture carries a tool message")]
        let tool = parsed.ir.messages.get(2).expect("the tool message parses");

        assert_eq!(tool.role, IrRole::Tool);
        assert_eq!(
            tool.parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "sunny".to_string(),
                is_error: true,
            }],
        );
    }

    /// A `tool_result` content-parts array flattens into the tool output with the blank-line
    /// separator: parts join in order, an empty part between texts keeps both its separators,
    /// and a leading run of empty parts carries none (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed tool outputs diverge from the documented flattening.
    #[test]
    fn tool_result_text_part_arrays_flatten_into_the_tool_output() {
        let between_empty = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [
                        {"type": "text", "text": "a"},
                        {"type": "text", "text": ""},
                        {"type": "text", "text": "b"},
                    ]},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the between-empty parts form is constructed to parse"
        )]
        let parsed = parse_inbound_request(between_empty.to_string().as_bytes(), &[])
            .expect("tool_result content parts must parse");

        #[expect(clippy::expect_used, reason = "the body parses to one message")]
        let tool = parsed.ir.messages.first().expect("the tool message parses");

        assert_eq!(tool.role, IrRole::Tool);

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "a\n\n\n\nb".to_string(),
                is_error: false,
            },
        );

        let leading_empty = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": [
                        {"type": "text", "text": ""},
                        {"type": "text", "text": "c"},
                    ]},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the leading-empty parts form is constructed to parse"
        )]
        let parsed = parse_inbound_request(leading_empty.to_string().as_bytes(), &[])
            .expect("tool_result content parts must parse");

        #[expect(clippy::expect_used, reason = "the body parses to one message")]
        let tool = parsed.ir.messages.first().expect("the tool message parses");

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "c".to_string(),
                is_error: false,
            },
        );
    }

    /// Official envelope and caching controls parse to the same IR as their absence: `metadata`
    /// (`user_id` foremost), `service_tier`, `container`, the top-level `cache_control`,
    /// `cache_control` on tools and on text/image blocks in user/assistant/system positions and
    /// on `tool_use`/`tool_result` blocks and `tool_result` text parts, and the tool `strict`
    /// flag are accepted-and-ignored (README §6.6) and never enter the IR, and the official
    /// `type: "custom"` tool discriminator parses like its absence.
    ///
    /// # Panics
    ///
    /// Panics if either body fails to parse or the two IRs diverge.
    #[test]
    fn accepts_and_ignores_official_envelope_and_caching_controls() {
        let plain = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse"}],
            "tools": [{
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
            }],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                    }},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun"},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny"}]},
                ]},
            ],
        });

        let decorated = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "metadata": {"user_id": "user_abc"},
            "service_tier": "auto",
            "container": {"id": "cntr_1", "skills": []},
            "cache_control": {"type": "ephemeral"},
            "system": [{"type": "text", "text": "be terse",
                        "cache_control": {"type": "ephemeral"}}],
            "tools": [{
                "type": "custom",
                "name": "get_weather",
                "description": "City weather",
                "input_schema": {"type": "object"},
                "cache_control": {"type": "ephemeral"},
                "strict": true,
            }],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?", "cache_control": {"type": "ephemeral"}},
                    {"type": "image", "source": {
                        "type": "url",
                        "url": "https://example.com/i.png",
                     }, "cache_control": {"type": "ephemeral"}},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun", "cache_control": {"type": "ephemeral"}},
                    {"type": "tool_use", "id": "call_1", "name": "get_weather",
                     "input": {"city": "Paris"}, "cache_control": {"type": "ephemeral"}},
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny",
                                  "cache_control": {"type": "ephemeral"}}],
                     "cache_control": {"type": "ephemeral"}},
                ]},
            ],
        });

        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain = parse_inbound_request(plain.to_string().as_bytes(), &[])
            .expect("the plain body parses");

        #[expect(
            clippy::expect_used,
            reason = "the decorated body is constructed to parse"
        )]
        let decorated = parse_inbound_request(decorated.to_string().as_bytes(), &[])
            .expect("the decorated body parses");

        // The decoration carries no semantics, so both parses land on the same IR, byte for
        // byte across every IR member.
        assert_eq!(plain.ir.model, decorated.ir.model);
        assert_eq!(plain.ir.instructions, decorated.ir.instructions);
        assert_eq!(plain.ir.messages, decorated.ir.messages);
        assert_eq!(plain.ir.tools, decorated.ir.tools);
        assert_eq!(plain.ir.tool_choice, decorated.ir.tool_choice);
        assert_eq!(plain.ir.temperature, decorated.ir.temperature);
        assert_eq!(plain.ir.top_p, decorated.ir.top_p);
        assert_eq!(plain.ir.max_output_tokens, decorated.ir.max_output_tokens);
        assert_eq!(plain.ir.response_format, decorated.ir.response_format);
        assert_eq!(plain.ir.reasoning_effort, decorated.ir.reasoning_effort);
        assert_eq!(plain.is_streaming, decorated.is_streaming);

        // Sanity: the shared system array and tool list parse to their expected IR values, so
        // the parity above compares real content rather than two empty shells.
        assert_eq!(plain.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(
            plain.ir.tools,
            vec![IrTool {
                name: "get_weather".to_string(),
                description: Some("City weather".to_string()),
                parameters: Some(json!({"type": "object"})),
            }],
        );
    }

    /// Text-block `citations` — the official replay member on every request text shape —
    /// parse to the same IR as their absence on all four shapes (user text, assistant text,
    /// system text block, `tool_result` text part): typed clients replay citations-bearing
    /// history verbatim, and the stateless gateway renders no citations downstream
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if either body fails to parse or the two IRs diverge.
    #[test]
    fn accepts_and_ignores_text_block_citations() {
        let plain = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse"}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?"},
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny"}]},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun"},
                ]},
            ],
        });

        let citations = json!([{
            "type": "char_location",
            "cited_text": "the sky is blue",
            "document_index": 0,
            "document_title": "weather",
            "start_char_index": 0,
            "end_char_index": 15,
        }]);

        let decorated = json!({
            "model": "claude-test",
            "max_tokens": 128,
            "system": [{"type": "text", "text": "be terse", "citations": citations}],
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "sky?", "citations": citations},
                    {"type": "tool_result", "tool_use_id": "call_1",
                     "content": [{"type": "text", "text": "sunny",
                                  "citations": citations}]},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "sun", "citations": citations},
                ]},
            ],
        });

        #[expect(clippy::expect_used, reason = "the plain body is constructed to parse")]
        let plain = parse_inbound_request(plain.to_string().as_bytes(), &[])
            .expect("the plain body parses");

        #[expect(
            clippy::expect_used,
            reason = "the decorated body is constructed to parse"
        )]
        let decorated = parse_inbound_request(decorated.to_string().as_bytes(), &[])
            .expect("the decorated body parses");

        // The citations carry no semantics, so both parses land on the same IR, byte for
        // byte across every IR member.
        assert_eq!(plain.ir.model, decorated.ir.model);
        assert_eq!(plain.ir.instructions, decorated.ir.instructions);
        assert_eq!(plain.ir.messages, decorated.ir.messages);
        assert_eq!(plain.ir.tools, decorated.ir.tools);
        assert_eq!(plain.ir.tool_choice, decorated.ir.tool_choice);
        assert_eq!(plain.ir.temperature, decorated.ir.temperature);
        assert_eq!(plain.ir.top_p, decorated.ir.top_p);
        assert_eq!(plain.ir.max_output_tokens, decorated.ir.max_output_tokens);
        assert_eq!(plain.ir.response_format, decorated.ir.response_format);
        assert_eq!(plain.ir.reasoning_effort, decorated.ir.reasoning_effort);
        assert_eq!(plain.is_streaming, decorated.is_streaming);

        // Sanity: the shared system array and the text/tool-output/assistant runs parse to
        // their expected IR values, so the parity above compares real content rather than
        // two empty shells.
        assert_eq!(plain.ir.instructions.as_deref(), Some("be terse"));
        assert_eq!(plain.ir.messages.len(), 3);

        #[expect(clippy::expect_used, reason = "the body carries a tool message")]
        let tool = plain.ir.messages.get(1).expect("the tool message parses");

        assert_eq!(
            part(&tool.parts, 0),
            &IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "sunny".to_string(),
                is_error: false,
            },
        );
    }

    /// `output_config.format` parses into the IR JSON-schema response format — the official
    /// `JSONOutputFormat` union carries no schema-name slot, so the IR `name` synthesizes the
    /// surface stand-in — while `effort` is optional beside it (the official `OutputConfig`
    /// carries both its members optional), `format: null` and an empty container parse as no
    /// constraint, and the malformed shapes — a non-`json_schema` `type`, a missing `schema`,
    /// a non-object `schema` — are 400s naming `format` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if any parse diverges from the documented mapping or a rejection lands outside
    /// its documented wording.
    #[test]
    fn parses_output_config_format_and_optional_effort() {
        let base = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            })
        };

        let schema = json!({"type": "object", "properties": {"city": {"type": "string"}}});

        let expected_format = |schema: Value| {
            Some(IrResponseFormat::JsonSchema {
                name: ANTHROPIC_RESPONSE_SCHEMA_NAME.to_string(),
                schema,
            })
        };

        for (output_config, expected_format, expected_effort) in [
            (
                json!({"effort": "high",
                       "format": {"type": "json_schema", "schema": schema.clone()}}),
                expected_format(schema.clone()),
                Some(IrReasoningLevel::High),
            ),
            (
                json!({"format": {"type": "json_schema", "schema": schema.clone()}}),
                expected_format(schema),
                None,
            ),
            (json!({"format": null}), None, None),
            (json!({}), None, None),
        ] {
            let mut body = base();

            set_member(&mut body, "output_config", output_config);

            #[expect(
                clippy::expect_used,
                reason = "each output_config shape is constructed to parse"
            )]
            let parsed = parse_inbound_request(body.to_string().as_bytes(), &[])
                .expect("the output_config shape parses");

            assert_eq!(parsed.ir.response_format, expected_format);
            assert_eq!(parsed.ir.reasoning_effort, expected_effort);
        }

        // The malformed shapes are 400s naming `format` (README §6.6).
        for format in [
            json!({"type": "text"}),
            json!({"type": "json_schema"}),
            json!({"type": "json_schema", "schema": "x"}),
            json!({"type": "unknown", "schema": {}}),
        ] {
            let mut body = base();

            set_member(&mut body, "output_config", json!({"format": format}));

            #[expect(
                clippy::expect_used,
                reason = "each malformed format is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes(), &[])
                .expect_err("the malformed format must be rejected");

            assert_eq!(error.param.as_deref(), Some("format"));
        }
    }

    /// Rejects unsupported and malformed inbound controls exactly where the contract pins them:
    /// `stop_sequences`, `budget_tokens` and `top_k` (naming the field), `diagnostics` and
    /// `inference_geo` (naming the field), `document` blocks, images inside tool results, a
    /// missing `max_tokens`, a non-`custom` tool `type`, and unknown members — while
    /// `thinking: disabled` parses into `none` and the `enabled`/`adaptive` variants plus their
    /// official `display` member are accepted and ignored.
    ///
    /// # Panics
    ///
    /// Panics if any rejection lands outside its documented wording.
    #[test]
    fn rejects_unsupported_controls_and_honors_thinking_dispositions() {
        let base = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            })
        };

        let cases: Vec<(Value, Option<&str>)> = vec![
            (
                json!({"stop_sequences": ["x"], "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "tools": [
                    {"type": "web_search_20250305", "name": "ws",
                     "input_schema": {"type": "object"}},
                ], "messages": [{"role": "user", "content": "hi"}]}),
                Some("type"),
            ),
            (
                json!({"model": "m", "max_tokens": 16,
                       "thinking": {"type": "enabled", "budget_tokens": 1000},
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("budget_tokens"),
            ),
            (
                json!({"top_k": 40, "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("top_k"),
            ),
            (
                json!({"diagnostics": {"previous_message_id": "msg_1"}, "model": "m",
                       "max_tokens": 16, "messages": [{"role": "user", "content": "hi"}]}),
                Some("diagnostics"),
            ),
            (
                json!({"inference_geo": "us", "model": "m", "max_tokens": 16,
                       "messages": [{"role": "user", "content": "hi"}]}),
                Some("inference_geo"),
            ),
            // The official `OutputConfig` carries exactly `effort` and `format`; a `display`
            // member is unknown and rejected like any other unknown field (README §6.6).
            (
                json!({"output_config": {"effort": "low", "display": "none"}, "model": "m",
                       "max_tokens": 16, "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            // The official `ThinkingConfigDisabled` carries its `type` alone; a `display`
            // member is unknown and rejected like any other unknown field (README §6.4).
            (
                json!({"thinking": {"type": "disabled", "display": "summary"}, "model": "m",
                       "max_tokens": 16, "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "assistant", "content": [
                        {"type": "document", "source": {"type": "text", "data": "x"}},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m", "max_tokens": 16, "messages": [
                    {"role": "user", "content": [
                        {"type": "tool_result", "tool_use_id": "c",
                         "content": [{"type": "image",
                                      "source": {"type": "url",
                                                 "url": "https://example.com/i.png"}}]},
                    ]},
                ]}),
                Some("content"),
            ),
            (
                json!({"model": "m",
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
            (
                json!({"model": "m", "max_tokens": 16, "bogus": true,
                       "messages": [{"role": "user", "content": "hi"}]}),
                None,
            ),
        ];

        for (mut body, param) in cases {
            // Every case starts from the valid base shape unless it already states its own.
            if body.get("messages").is_none() {
                body = base();
            }

            #[expect(
                clippy::expect_used,
                reason = "each control is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes(), &[])
                .expect_err("the unsupported control must be rejected");

            assert_eq!(error.param.as_deref(), param, "wrong param for {error}");
        }

        // The named known-unsupported rows pin their fixed messages: each 400 names its field
        // rather than landing in the generic unknown-field accident (README §6.6).
        for (member, value, message) in [
            ("top_k", json!(40), "top_k is not supported"),
            (
                "diagnostics",
                json!({"previous_message_id": "msg_1"}),
                "diagnostics is not supported; the gateway carries no cross-request message state",
            ),
            (
                "inference_geo",
                json!("us"),
                "inference_geo is not supported; the gateway cannot pin inference to a region",
            ),
        ] {
            let mut body = base();

            set_member(&mut body, member, value);

            #[expect(
                clippy::expect_used,
                reason = "each known-unsupported control is constructed to be rejected"
            )]
            let error = parse_inbound_request(body.to_string().as_bytes(), &[])
                .expect_err("the known-unsupported control must be rejected");

            assert_eq!(error.to_string(), message);
        }

        // `thinking: disabled` parses into `none`; `enabled`/`adaptive` and their official
        // `display` member parse without entering the IR.
        for (thinking, output_config, expected) in [
            (
                Some(json!({"type": "disabled"})),
                None,
                Some(IrReasoningLevel::None),
            ),
            (Some(json!({"type": "enabled"})), None, None),
            (Some(json!({"type": "adaptive"})), None, None),
            (
                Some(json!({"type": "enabled", "display": "summary"})),
                None,
                None,
            ),
            (
                Some(json!({"type": "adaptive", "display": "omitted"})),
                None,
                None,
            ),
            (
                None,
                Some(json!({"effort": "low"})),
                Some(IrReasoningLevel::Low),
            ),
        ] {
            let mut body = base();

            if let Some(thinking) = thinking {
                set_member(&mut body, "thinking", thinking);
            }

            if let Some(output_config) = output_config {
                set_member(&mut body, "output_config", output_config);
            }

            set_member(&mut body, "thinking_summaries", json!(["summary"]));

            #[expect(
                clippy::expect_used,
                reason = "each thinking disposition is constructed to parse"
            )]
            let parsed = parse_inbound_request(body.to_string().as_bytes(), &[])
                .expect("the thinking disposition parses");

            assert_eq!(parsed.ir.reasoning_effort, expected);
        }

        // An unknown effort string is a 400 naming `output_config`.
        let mut body = base();

        set_member(&mut body, "output_config", json!({"effort": "ultra"}));

        #[expect(
            clippy::expect_used,
            reason = "the unknown effort is constructed to be rejected"
        )]
        let error = parse_inbound_request(body.to_string().as_bytes(), &[])
            .expect_err("an unknown effort must be rejected");

        assert_eq!(error.param.as_deref(), Some("output_config"));
    }

    /// Parses one inbound body with the given `anthropic-beta` entries.
    ///
    /// # Panics
    ///
    /// Panics if the body is not a JSON object fixture, which cannot occur.
    fn parse_with_betas(body: Value, betas: &[&str]) -> Result<ParsedRequest, InvalidRequest> {
        let betas: Vec<String> = betas.iter().map(|beta| beta.to_string()).collect();

        parse_inbound_request(body.to_string().as_bytes(), &betas)
    }

    /// Builds the minimal valid compaction request body: one user turn plus the given
    /// extra members.
    fn compaction_body(extra: Value) -> Value {
        let mut body = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "summarize this"}],
        });

        set_member(&mut body, "compaction", extra);

        body
    }

    /// The parsed compaction control of one request.
    ///
    /// # Panics
    ///
    /// Panics when the request carries no compaction facts: the absence is the failure.
    fn compaction_of(parsed: &ParsedRequest) -> &CompactionControl {
        #[expect(
            clippy::expect_used,
            reason = "the fixture carries compaction facts; their absence is the failure"
        )]
        parsed
            .compaction
            .as_ref()
            .and_then(|compaction| compaction.control.as_ref())
            .expect("the fixture carries a compaction control")
    }

    /// Reads the only boundary of one parsed request.
    ///
    /// # Panics
    ///
    /// Panics when the request does not carry exactly one boundary: the count is the failure.
    fn only_boundary(parsed: &ParsedRequest) -> &CompactionBoundary {
        #[expect(
            clippy::expect_used,
            reason = "the fixture is constructed to parse; a missing value is the failure"
        )]
        let compaction = parsed
            .compaction
            .as_ref()
            .expect("the fixture carries compaction facts");

        assert_eq!(compaction.boundaries.len(), 1);

        #[expect(clippy::expect_used, reason = "exactly one boundary was just asserted")]
        compaction.boundaries.first().expect("exactly one boundary")
    }

    /// Reads the only state item of one parsed request.
    ///
    /// # Panics
    ///
    /// Panics when the request does not carry exactly one state item: the count is the failure.
    fn only_state_item(parsed: &ParsedRequest) -> &StateItem {
        assert_eq!(parsed.ir.state_items.len(), 1);

        #[expect(clippy::expect_used, reason = "exactly one item was just asserted")]
        parsed
            .ir
            .state_items
            .first()
            .expect("exactly one state item")
    }

    /// Reports a boundary that should have been a gateway carrier.
    ///
    /// # Panics
    ///
    /// Always panics: the caller reached a native boundary where the gateway carrier shape was
    /// required, which is the failure under test.
    #[expect(clippy::panic, reason = "a wrong boundary shape is the failure report")]
    fn panic_boundary(boundary: &CompactionBoundary) -> ! {
        panic!("a namespaced signature must recognize a gateway carrier, got {boundary:?}")
    }

    /// The on-demand control parses with its documented instruction semantics: absent and
    /// whitespace-only instructions mean none, a verbatim non-blank value is kept, an
    /// instruction beyond the documented maximum is a 400 naming `instructions`, and a
    /// non-`summarize` type is a 400 naming `type`.
    ///
    /// # Panics
    ///
    /// Panics if any parse diverges from the documented control contract.
    #[test]
    fn parses_the_on_demand_control() {
        for (instructions, expected) in [
            (None, None),
            (Some("   "), None),
            (
                Some("focus on decisions"),
                Some("focus on decisions".to_string()),
            ),
        ] {
            let extra = match instructions {
                None => json!({"type": "summarize"}),
                Some(value) => json!({"type": "summarize", "instructions": value}),
            };

            #[expect(
                clippy::expect_used,
                reason = "each instruction shape is constructed to parse"
            )]
            let parsed = parse_with_betas(compaction_body(extra), &[super::ON_DEMAND_BETA])
                .expect("the on-demand body parses");

            assert_eq!(
                compaction_of(&parsed),
                &CompactionControl::MessagesOnDemand {
                    instructions: expected,
                }
            );
        }

        // An instruction beyond the maximum is a 400 naming `instructions`.
        let long = "x".repeat(super::MAX_COMPACTION_INSTRUCTIONS_CHARS + 1);
        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            compaction_body(json!({"type": "summarize", "instructions": long})),
            &[super::ON_DEMAND_BETA],
        )
        .expect_err("the over-long instructions must be rejected");

        assert_eq!(error.param.as_deref(), Some("instructions"));

        // Exactly the maximum parses.
        let exactly = "x".repeat(super::MAX_COMPACTION_INSTRUCTIONS_CHARS);

        #[expect(
            clippy::expect_used,
            reason = "the boundary-length instructions are constructed to parse"
        )]
        let parsed = parse_with_betas(
            compaction_body(json!({"type": "summarize", "instructions": exactly})),
            &[super::ON_DEMAND_BETA],
        )
        .expect("the boundary-length instructions parse");

        assert!(matches!(
            compaction_of(&parsed),
            CompactionControl::MessagesOnDemand {
                instructions: Some(_)
            }
        ));

        // A non-`summarize` type is a 400 naming `type`.
        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            compaction_body(json!({"type": "rewrite"})),
            &[super::ON_DEMAND_BETA],
        )
        .expect_err("a non-summarize type must be rejected");

        assert_eq!(error.param.as_deref(), Some("type"));
    }

    /// An unresolved final tool call rejects on-demand compaction: the gateway
    /// never fabricates a tool result to make the history summarizable. A
    /// resolved pair (tool call, then its result) stays valid.
    ///
    /// # Panics
    ///
    /// Panics if an assertion fails — the assertion is the failure report.
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "fixtures serialize and parse values constructed to succeed; the rejection is \
                  the assertion"
    )]
    fn rejects_an_unresolved_final_tool_call_for_on_demand() {
        let mut unresolved = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": "search"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "x"}}
                ]},
                {"role": "user", "content": "summarize this too"},
            ],
        });

        set_member(&mut unresolved, "compaction", json!({"type": "summarize"}));

        let error = parse_inbound_request(
            serde_json::to_vec(&unresolved)
                .expect("the fixture serializes")
                .as_slice(),
            &[],
        )
        .expect_err("the unresolved final tool call is rejected");

        assert_eq!(error.param.as_deref(), Some("messages"));

        // A resolved tool exchange before the pending user turn is valid.
        let mut resolved = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": "search"},
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "search", "input": {"q": "x"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "result"}
                ]},
                {"role": "user", "content": "summarize this too"},
            ],
        });

        set_member(&mut resolved, "compaction", json!({"type": "summarize"}));

        let parsed = parse_inbound_request(
            serde_json::to_vec(&resolved)
                .expect("the fixture serializes")
                .as_slice(),
            &[super::ON_DEMAND_BETA.to_string()],
        )
        .expect("the resolved history summarizes");

        assert!(matches!(
            compaction_of(&parsed),
            CompactionControl::MessagesOnDemand { .. }
        ));
    }
    /// The threshold control parses its edits in order with the documented defaults — absent
    /// trigger keeps the protocol default (recorded as `None`), `pause_after_compaction`
    /// defaults to `false` — while a sub-minimum trigger (naming `trigger`), a non-
    /// `input_tokens` trigger type (naming `trigger.type`), and an unsupported edit type
    /// including the `clear_tool_uses_20250919`/`clear_thinking_20251015` spellings (naming
    /// `type`) are 400s.
    ///
    /// # Panics
    ///
    /// Panics if any parse diverges from the documented control contract.
    #[test]
    fn parses_the_threshold_control() {
        let threshold_body = |edits: Value| {
            let mut body = json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            });

            set_member(&mut body, "context_management", json!({"edits": edits}));

            body
        };

        // Defaults: no trigger member keeps the protocol default, recorded as `None`; the
        // pause flag defaults to `false`.
        #[expect(
            clippy::expect_used,
            reason = "the default edit shape is constructed to parse"
        )]
        let parsed = parse_with_betas(
            threshold_body(json!([{"type": "compact_20260112"}])),
            &[super::THRESHOLD_BETA],
        )
        .expect("the default edit parses");

        assert_eq!(
            compaction_of(&parsed),
            &CompactionControl::MessagesThreshold {
                edits: vec![compaction_control::ThresholdEdit {
                    trigger: None,
                    pause_after_compaction: false,
                    instructions: None,
                }],
            }
        );

        // A stated trigger, pause flag, and whitespace-only instructions parse through.
        #[expect(
            clippy::expect_used,
            reason = "the stated edit shape is constructed to parse"
        )]
        let parsed = parse_with_betas(
            threshold_body(json!([{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 150_000},
                "pause_after_compaction": true,
                "instructions": "   ",
            }])),
            &[super::THRESHOLD_BETA],
        )
        .expect("the stated edit parses");

        assert_eq!(
            compaction_of(&parsed),
            &CompactionControl::MessagesThreshold {
                edits: vec![compaction_control::ThresholdEdit {
                    trigger: Some(150_000),
                    pause_after_compaction: true,
                    instructions: None,
                }],
            }
        );

        // The minimum trigger value parses; one below it is a 400 naming `trigger`, never
        // clamped.
        #[expect(
            clippy::expect_used,
            reason = "the minimum trigger is constructed to parse"
        )]
        let parsed = parse_with_betas(
            threshold_body(json!([{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 50_000},
            }])),
            &[super::THRESHOLD_BETA],
        )
        .expect("the minimum trigger parses");

        assert_eq!(
            compaction_of(&parsed),
            &CompactionControl::MessagesThreshold {
                edits: vec![compaction_control::ThresholdEdit {
                    trigger: Some(50_000),
                    pause_after_compaction: false,
                    instructions: None,
                }],
            }
        );

        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            threshold_body(json!([{
                "type": "compact_20260112",
                "trigger": {"type": "input_tokens", "value": 49_999},
            }])),
            &[super::THRESHOLD_BETA],
        )
        .expect_err("the sub-minimum trigger must be rejected");

        assert_eq!(error.param.as_deref(), Some("trigger"));

        // A non-`input_tokens` trigger type is a 400 naming `trigger.type`.
        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            threshold_body(json!([{
                "type": "compact_20260112",
                "trigger": {"type": "output_tokens", "value": 150_000},
            }])),
            &[super::THRESHOLD_BETA],
        )
        .expect_err("the foreign trigger type must be rejected");

        assert_eq!(error.param.as_deref(), Some("trigger.type"));

        // Unsupported edit types — the clear edits included — are 400s naming `type`.
        for kind in [
            "clear_tool_uses_20250919",
            "clear_thinking_20251015",
            "compact_99999999",
        ] {
            #[expect(
                clippy::expect_used,
                reason = "the rejected body is constructed to fail; an Ok is the failure under test"
            )]
            let error = parse_with_betas(
                threshold_body(json!([{"type": kind}])),
                &[super::THRESHOLD_BETA],
            )
            .expect_err("an unsupported edit type must be rejected");

            assert_eq!(
                error.param.as_deref(),
                Some("type"),
                "wrong param for {kind}"
            );
        }
    }

    /// The invalid-combination rules are 400s naming their fields: the two controls together
    /// (naming `compaction`), the on-demand control with `output_config.format` (naming
    /// `format`), with forced tool use (naming `tool_choice`), or with an empty message list
    /// (naming `messages`), and a threshold edit beside a native compaction block (naming
    /// `context_management`).
    ///
    /// # Panics
    ///
    /// Panics if any combination fails to reject or rejects under another name.
    #[test]
    fn rejects_the_invalid_combinations() {
        let base = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hi"}],
        });

        let with_compaction = |mut body: Value| {
            set_member(&mut body, "compaction", json!({"type": "summarize"}));

            body
        };

        let mut both = base.clone();

        set_member(&mut both, "compaction", json!({"type": "summarize"}));
        set_member(
            &mut both,
            "context_management",
            json!({"edits": [{"type": "compact_20260112"}]}),
        );

        let cases = vec![
            // Both controls together.
            (both, Some("compaction"), super::ON_DEMAND_BETA),
            // A structured-output format beside the on-demand control.
            (
                with_compaction(json!({
                    "model": "m",
                    "max_tokens": 16,
                    "output_config": {
                        "format": {"type": "json_schema", "schema": {"type": "object"}}
                    },
                    "messages": [{"role": "user", "content": "hi"}],
                })),
                Some("format"),
                super::ON_DEMAND_BETA,
            ),
            // Forced tool use beside the on-demand control.
            (
                with_compaction(json!({
                    "model": "m",
                    "max_tokens": 16,
                    "tools": [{"name": "ping", "input_schema": {"type": "object"}}],
                    "tool_choice": {"type": "any"},
                    "messages": [{"role": "user", "content": "hi"}],
                })),
                Some("tool_choice"),
                super::ON_DEMAND_BETA,
            ),
            (
                with_compaction(json!({
                    "model": "m",
                    "max_tokens": 16,
                    "tools": [{"name": "ping", "input_schema": {"type": "object"}}],
                    "tool_choice": {"type": "tool", "name": "ping"},
                    "messages": [{"role": "user", "content": "hi"}],
                })),
                Some("tool_choice"),
                super::ON_DEMAND_BETA,
            ),
            // An empty message list has nothing to summarize.
            (
                with_compaction(json!({"model": "m", "max_tokens": 16, "messages": []})),
                Some("messages"),
                super::ON_DEMAND_BETA,
            ),
        ];

        for (body, param, beta) in cases {
            #[expect(
                clippy::expect_used,
                reason = "each combination is constructed to be rejected"
            )]
            let error = parse_with_betas(body, &[beta])
                .expect_err("the invalid combination must be rejected");

            assert_eq!(error.param.as_deref(), param, "wrong param for {error}");
        }

        // The empty-history rejection names the summarization wording.
        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            with_compaction(json!({"model": "m", "max_tokens": 16, "messages": []})),
            &[super::ON_DEMAND_BETA],
        )
        .expect_err("the empty history must be rejected");

        assert!(error.to_string().contains("nothing to summarize"));

        // Unforced tool choices beside the on-demand control stay valid.
        for choice in ["auto", "none"] {
            #[expect(
                clippy::expect_used,
                reason = "the unforced choice is constructed to parse"
            )]
            let parsed = parse_with_betas(
                with_compaction(json!({
                    "model": "m",
                    "max_tokens": 16,
                    "tools": [{"name": "ping", "input_schema": {"type": "object"}}],
                    "tool_choice": {"type": choice},
                    "messages": [{"role": "user", "content": "hi"}],
                })),
                &[super::ON_DEMAND_BETA],
            )
            .expect("an unforced tool choice parses beside the control");

            assert!(parsed.compaction.is_some());
        }

        // A threshold edit beside a native compaction block is a 400 naming
        // `context_management`; a gateway carrier block is not native and does not trigger
        // the rule.
        let threshold_with_block = |signature: &str| {
            let body = json!({
                "model": "m",
                "max_tokens": 16,
                "context_management": {"edits": [{"type": "compact_20260112"}]},
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": [
                        {"type": "text", "text": "worked"},
                        {"type": "compaction", "content": "summary", "signature": signature},
                    ]},
                ],
            });

            body
        };

        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(
            threshold_with_block("native-signature"),
            &[super::THRESHOLD_BETA],
        )
        .expect_err("a threshold edit beside a native block must be rejected");

        assert_eq!(error.param.as_deref(), Some("context_management"));

        #[expect(
            clippy::expect_used,
            reason = "the gateway-carrier shape is constructed to parse"
        )]
        let parsed = parse_with_betas(
            threshold_with_block("janusgate.compaction.v1.primary.aGVsbG8"),
            &[super::THRESHOLD_BETA],
        )
        .expect("a threshold edit beside a gateway carrier parses");

        assert!(parsed.compaction.is_some());
    }

    /// Compaction content blocks convert into the boundary/state-item pair by the signature
    /// namespace: a namespaced signature marks a gateway carrier (visible content beside the
    /// raw block), any other signature — absent included — a native block whose placement is
    /// leading exactly at the first content block of the first message and historical anywhere
    /// else, whose offset counts the IR messages converted before it, and whose null content
    /// marks the no-op. A second compaction block is a 400 naming `content`.
    ///
    /// # Panics
    ///
    /// Panics if any recognition diverges from the documented contract.
    #[test]
    fn recognizes_gateway_and_native_compaction_blocks() {
        let gateway_signature = "janusgate.compaction.v1.primary.aGVsbG8=";

        // Gateway carrier: boundary beside a gateway state item carrying the raw block.
        let body = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "readable summary",
                     "signature": gateway_signature},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the gateway block shape is constructed to parse"
        )]
        let parsed = parse_with_betas(body, &[]).expect("the gateway block parses");

        #[expect(
            clippy::expect_used,
            reason = "the fixture is constructed to parse; a missing value is the failure"
        )]
        let compaction = parsed
            .compaction
            .as_ref()
            .expect("the gateway block produces compaction facts");

        assert!(compaction.control.is_none());

        let boundary = only_boundary(&parsed);

        let CompactionBoundary::Gateway {
            offset,
            carrier,
            visible_content,
        } = boundary
        else {
            panic_boundary(boundary);
        };

        assert_eq!(*offset, 0);
        assert_eq!(carrier, gateway_signature);
        assert_eq!(visible_content.as_deref(), Some("readable summary"));

        let state_item = only_state_item(&parsed);

        assert!(state_item.is_gateway);
        assert_eq!(state_item.offset, 0);
        assert_eq!(
            state_item.item,
            json!({
                "type": "compaction",
                "content": "readable summary",
                "signature": gateway_signature,
            })
        );

        // A leading native block: first content block of the first message, null content
        // marking the no-op, absent members never fabricated in the state item.
        let body = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "compaction"},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the null block shape is constructed to parse"
        )]
        let parsed =
            parse_with_betas(body, &[super::ON_DEMAND_BETA]).expect("the null no-op block parses");

        #[expect(
            clippy::expect_used,
            reason = "the fixture is constructed to parse; a missing value is the failure"
        )]
        let compaction = parsed
            .compaction
            .as_ref()
            .expect("the null block produces compaction facts");

        assert_eq!(
            compaction.boundaries,
            vec![CompactionBoundary::Native {
                offset: 0,
                artifact: NativeArtifact::MessagesCompactionBlock {
                    is_null_no_op: true,
                    placement: BoundaryPlacement::Leading,
                },
            }]
        );
        let state_item = only_state_item(&parsed);

        assert!(!state_item.is_gateway);
        assert_eq!(state_item.item, json!({"type": "compaction"}));

        // A historical native block sits mid-turn in a later message; its offset counts the
        // IR messages converted before the turn holding it.
        let body = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "user", "content": [
                    {"type": "text", "text": "one"},
                    {"type": "text", "text": "two"},
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "worked"},
                    {"type": "compaction", "content": "summary", "signature": "native-sig"},
                ]},
                {"role": "user", "content": "next"},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the historical block shape is constructed to parse"
        )]
        let parsed =
            parse_with_betas(body, &[super::THRESHOLD_BETA]).expect("the historical block parses");

        #[expect(
            clippy::expect_used,
            reason = "the fixture is constructed to parse; a missing value is the failure"
        )]
        let compaction = parsed
            .compaction
            .as_ref()
            .expect("the historical block produces compaction facts");

        // The user turn's two texts merge into one IR message, so the offset is 1.
        assert_eq!(
            compaction.boundaries,
            vec![CompactionBoundary::Native {
                offset: 1,
                artifact: NativeArtifact::MessagesCompactionBlock {
                    is_null_no_op: false,
                    placement: BoundaryPlacement::Historical,
                },
            }]
        );

        // A second compaction block is a 400 naming `content`.
        let body = json!({
            "model": "m",
            "max_tokens": 16,
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "first", "signature": "sig"},
                ]},
                {"role": "assistant", "content": [
                    {"type": "compaction", "content": "second", "signature": "sig"},
                ]},
            ],
        });

        #[expect(
            clippy::expect_used,
            reason = "the rejected body is constructed to fail; an Ok is the failure under test"
        )]
        let error = parse_with_betas(body, &[super::ON_DEMAND_BETA])
            .expect_err("a second compaction block must be rejected");

        assert_eq!(error.param.as_deref(), Some("content"));
    }

    /// The inbound beta requirements: the on-demand beta covers the on-demand control and a
    /// leading native block, the threshold beta the threshold control and a historical native
    /// block; the exact beta spelling must appear among the caller's `anthropic-beta` entries
    /// and unrelated entries are tolerated.
    ///
    /// # Panics
    ///
    /// Panics if any beta requirement fails to enforce or rejects a satisfied one.
    #[test]
    fn validates_the_inbound_beta_requirements() {
        let on_demand_body = || compaction_body(json!({"type": "summarize"}));

        let leading_block_body = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [
                    {"role": "assistant", "content": [
                        {"type": "compaction", "content": "summary", "signature": "sig"},
                    ]},
                ],
            })
        };

        let threshold_body = || {
            let mut body = json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [{"role": "user", "content": "hi"}],
            });

            set_member(
                &mut body,
                "context_management",
                json!({"edits": [{"type": "compact_20260112"}]}),
            );

            body
        };

        let historical_block_body = || {
            json!({
                "model": "m",
                "max_tokens": 16,
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "content": [
                        {"type": "compaction", "content": "summary", "signature": "sig"},
                    ]},
                ],
            })
        };

        // Missing betas are 400s naming the header.
        for body in [
            on_demand_body(),
            leading_block_body(),
            threshold_body(),
            historical_block_body(),
        ] {
            #[expect(
                clippy::expect_used,
                reason = "each missing-beta body is constructed to be rejected"
            )]
            let error = parse_with_betas(body, &[]).expect_err("the missing beta must be rejected");

            assert_eq!(error.param.as_deref(), Some("anthropic-beta"));
            assert!(error.to_string().contains("anthropic-beta"));
        }

        // Unrelated entries do not satisfy either requirement.
        for body in [on_demand_body(), threshold_body()] {
            #[expect(
                clippy::expect_used,
                reason = "each unrelated-beta body is constructed to be rejected"
            )]
            let error = parse_with_betas(body, &["interleaved-thinking-2025-05-14"])
                .expect_err("an unrelated beta must not satisfy the requirement");

            assert_eq!(error.param.as_deref(), Some("anthropic-beta"));
        }

        // The exact spellings satisfy their requirements, beside unrelated entries.
        #[expect(
            clippy::expect_used,
            reason = "the satisfied on-demand body is constructed to parse"
        )]
        let parsed = parse_with_betas(
            on_demand_body(),
            &["interleaved-thinking-2025-05-14", super::ON_DEMAND_BETA],
        )
        .expect("the on-demand beta satisfies the requirement");

        assert!(parsed.compaction.is_some());

        #[expect(
            clippy::expect_used,
            reason = "the satisfied leading-block body is constructed to parse"
        )]
        let parsed = parse_with_betas(leading_block_body(), &[super::ON_DEMAND_BETA])
            .expect("the on-demand beta satisfies the leading block");

        assert!(parsed.compaction.is_some());

        #[expect(
            clippy::expect_used,
            reason = "the satisfied threshold body is constructed to parse"
        )]
        let parsed = parse_with_betas(threshold_body(), &[super::THRESHOLD_BETA])
            .expect("the threshold beta satisfies the threshold control");

        assert!(parsed.compaction.is_some());

        // A historical block requires the threshold beta, not the on-demand one.
        #[expect(
            clippy::expect_used,
            reason = "the historical body is constructed to be rejected under the wrong beta"
        )]
        let error = parse_with_betas(historical_block_body(), &[super::ON_DEMAND_BETA])
            .expect_err("the on-demand beta must not satisfy the threshold requirement");

        assert_eq!(error.param.as_deref(), Some("anthropic-beta"));
    }
}
