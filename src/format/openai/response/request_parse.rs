//! Client-request parsing for the Responses codec: the outward `POST /responses` body deserialized
//! into the IR request.

use super::compaction;
use crate::{
    compaction::{identity::InstructionRole, sealing},
    format::{
        self, CompactionBoundary, CompactionControl, NativeArtifact, ParsedCompaction,
        ParsedInstructionItem, ParsedRequest, StringOrArray, compaction_control::AutomaticEntry,
    },
    ir::{
        InvalidRequest, IrImage, IrMessage, IrPart, IrReasoningLevel, IrRequest, IrResponseFormat,
        IrRole, IrTool, IrToolChoice, StateItem,
    },
};
use serde::{Deserialize, Serialize, de::IgnoredAny};
use serde_json::Value;

/// Strictly parsed Responses request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    /// Public model name to route to.
    model: String,

    /// Conversation input: a plain string or an ordered item array.
    input: StringOrArray<ItemIn>,

    /// System instructions.
    #[serde(default)]
    instructions: Option<String>,

    /// Function tools advertised to the model.
    #[serde(default)]
    tools: Option<Vec<ToolIn>>,

    /// Tool selection restriction.
    #[serde(default)]
    tool_choice: Option<ToolChoiceIn>,

    /// Sampling temperature.
    #[serde(default)]
    temperature: Option<f64>,

    /// Nucleus sampling threshold.
    #[serde(default)]
    top_p: Option<f64>,

    /// Output token budget.
    #[serde(default)]
    max_output_tokens: Option<u64>,

    /// Reasoning configuration.
    #[serde(default)]
    reasoning: Option<ReasoningIn>,

    /// Responses automatic compaction controls: the caller's ordered
    /// `context_management` entries, each validated into
    /// [`CompactionControl::ResponsesAutomatic`].
    #[serde(default)]
    context_management: Option<Vec<ContextManagementEntryIn>>,

    /// Whether the client wants a streamed response.
    #[serde(default)]
    stream: Option<bool>,

    /// Storage request; the gateway is stateless, so only `false` is legal.
    #[serde(default)]
    store: Option<bool>,

    /// Text output configuration.
    #[serde(default)]
    text: Option<TextIn>,

    // The remaining fields are stateless-idempotent envelope controls that typed clients — the
    // codex CLI serializes every one of them on each request — always carry, but that a stateless
    // gateway can never honor downstream; each is accepted and ignored, like the item-level
    // carve-outs above (README §6.6).
    //
    // `stream_options` has no Responses-specific schema, and `client_metadata` and
    // `access_programs` are client-declared free-form values, so the three are ignored in whatever
    // JSON shape they arrive.
    /// Parallel tool-call switch; the downstream provider decides parallelism, so it is accepted
    /// and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    parallel_tool_calls: Option<bool>,

    /// Extra output content the client asks to have included (codex asks for
    /// `reasoning.encrypted_content`); the gateway renders its own output shapes, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    include: Option<Vec<String>>,

    /// Upstream prompt-cache placement key; cache placement is a downstream concern, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    prompt_cache_key: Option<String>,

    /// Streaming options; usage is always included, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    stream_options: IgnoredAny,

    /// Requested service tier; the gateway has no tier control, so it is accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    service_tier: Option<String>,

    /// Free-form client metadata; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    client_metadata: IgnoredAny,

    /// Client-declared access programs; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    access_programs: IgnoredAny,
}

/// Input item, tagged by `type`.
///
/// The shapes are strict: a member outside the item's own shape is rejected. The exception is a
/// `reasoning` item, whose payload is accepted in whatever shape it arrives — its replayable
/// members are typed so a reasoning item carrying `encrypted_content` is preserved verbatim as an
/// opaque state item, and every member beside the typed ones is ignored.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum ItemIn {
    /// Conversation message.
    Message {
        /// Gateway-rendered item identifier; accepted and ignored, because codex replays parsed
        /// output items verbatim each turn (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Authoring role.
        role: String,

        /// Text or typed content.
        content: StringOrArray<PartIn>,

        /// Gateway-rendered item phase; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        phase: Option<IgnoredAny>,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,
    },

    /// A past tool invocation.
    FunctionCall {
        /// Gateway-rendered item identifier; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Invocation identifier.
        call_id: String,

        /// Function name.
        name: String,

        /// JSON-encoded arguments.
        #[serde(default)]
        arguments: String,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,

        /// Thought signature to reattach across turns.
        #[serde(default)]
        thought_signature: Option<String>,
    },

    /// A durable configuration control. The gateway cannot apply arbitrary
    /// configuration semantics downstream, and discarding a durable control to
    /// make a different endpoint accept the request is forbidden — so the item
    /// parses only to an explicit rejection naming it.
    ConfigurationUpdate {
        /// The control's payload; parsed only to carry the rejection.
        #[expect(dead_code, reason = "parsed only to be rejected explicitly")]
        control: IgnoredAny,
    },

    /// A tool result.
    FunctionCallOutput {
        /// Gateway-rendered item identifier; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        id: Option<IgnoredAny>,

        /// Identifier of the invocation being answered.
        call_id: String,

        /// Result content: a plain string or an array of text parts.
        output: StringOrArray<OutputPartIn>,

        /// Gateway-rendered item status; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        status: Option<IgnoredAny>,
    },

    /// A replayed reasoning item: its replayable members are typed, and a
    /// member outside them is ignored (the item is accepted in whatever shape
    /// it arrives). An item carrying `encrypted_content` becomes an opaque
    /// state item replayed at its history position.
    Reasoning(ReasoningItem),

    /// A replayed compaction item: opaque state — a gateway carrier or native
    /// compaction state, decided by the carrier namespace of its
    /// `encrypted_content`.
    #[serde(rename = "compaction", alias = "compaction_summary")]
    Compaction(CompactionItemIn),

    /// The explicit compaction trigger control item: a request to compact, not
    /// conversation content. Only the exact `compaction_trigger` spelling is
    /// accepted.
    CompactionTrigger {},
}

/// A replayed reasoning item's typed replayable members.
///
/// Deserialization accepts the item in whatever shape it arrives (README §6.6); serialization
/// re-renders the typed members with their presence preserved — an absent optional member stays
/// absent — so an item carrying `encrypted_content` replays verbatim as an opaque state item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct ReasoningItem {
    /// Gateway- or provider-minted item identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) id: Option<String>,

    /// The reasoning summary parts, of whatever vendor shape.
    pub(super) summary: Vec<Value>,

    /// The reasoning content parts, of whatever vendor shape, when the item
    /// carries them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) content: Option<Vec<Value>>,

    /// The opaque encrypted replay payload whose presence makes the item an
    /// opaque state item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) encrypted_content: Option<String>,
}

/// Optional native replay text whose wire absence differs from an explicitly supplied null.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum OptionalReplayText {
    /// No member occurred on the client wire.
    #[default]
    #[serde(skip)]
    Missing,
    /// Exact accepted string or an explicitly supplied null.
    Present(Option<String>),
}

impl OptionalReplayText {
    /// Whether serialization must omit this absent member entirely.
    pub(super) const fn is_absent(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

/// A replayed compaction item's members.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompactionItemIn {
    /// Gateway- or provider-minted item identifier, replayed verbatim.
    #[serde(default, skip_serializing_if = "OptionalReplayText::is_absent")]
    pub(super) id: OptionalReplayText,

    /// The opaque replay state: a namespaced gateway carrier or native
    /// ciphertext.
    pub(super) encrypted_content: String,
    /// Authoring provenance retained across native replay.
    #[serde(default, skip_serializing_if = "OptionalReplayText::is_absent")]
    pub(super) created_by: OptionalReplayText,
}

/// One `context_management` entry of the request envelope.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ContextManagementEntryIn {
    /// Entry kind; only `compaction` is supported, every other kind is
    /// rejected by the conversion below.
    #[serde(rename = "type")]
    pub(super) entry_type: String,

    /// The compaction threshold in input tokens, when the caller stated one.
    #[serde(default)]
    pub(super) compact_threshold: Option<u64>,
}

impl ItemIn {
    /// Wraps one plain-text user turn: the item a bare-string `input` carries.
    pub(super) fn user_text(text: String) -> Self {
        Self::Message {
            id: None,
            role: "user".to_string(),
            content: StringOrArray::Text(text),
            phase: None,
            status: None,
        }
    }
}

/// Content part, tagged by `type`; a member outside the part's own shape is rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum PartIn {
    /// Text segment on the input side.
    InputText {
        /// The text content.
        text: String,
    },

    /// Text segment on the assistant side.
    OutputText {
        /// The text content.
        text: String,

        /// Gateway-rendered annotations; accepted and ignored, because the gateway's own rendered
        /// `output_text` parts carry the member and replaying an aggregated response verbatim must
        /// parse (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        annotations: Option<IgnoredAny>,

        /// Gateway-rendered log probabilities; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        logprobs: Option<IgnoredAny>,
    },

    /// Image segment.
    InputImage {
        /// Remote or data URL of the image.
        image_url: String,

        /// OpenAI image detail hint; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        detail: Option<IgnoredAny>,
    },
}

/// Content part of a `function_call_output` output array, tagged by `type`.
///
/// Only `input_text` is supported: the IR tool output has no media channel. The image and file
/// kinds are modelled so the rejection can name the offending kind, and every further kind lands in
/// [`OutputPartIn::Other`], which the conversion rejects the same way.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum OutputPartIn {
    /// Text segment.
    InputText {
        /// The text content.
        text: String,
    },

    /// Image segment, which the IR tool output cannot carry.
    InputImage {
        /// Remote or data URL of the image.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        image_url: Option<IgnoredAny>,

        /// OpenAI image detail hint; accepted and ignored (README §6.6).
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        detail: Option<IgnoredAny>,
    },

    /// File segment, which the IR tool output cannot carry.
    InputFile {
        /// Upstream file identifier.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        file_id: Option<IgnoredAny>,

        /// Inline file data.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        file_data: Option<IgnoredAny>,

        /// File name.
        #[serde(default)]
        #[expect(dead_code, reason = "rejected by the conversion below")]
        filename: Option<IgnoredAny>,
    },

    /// Any other part kind, of which the tag value the enum machinery consumed leaves no trace.
    #[serde(other)]
    Other,
}

impl OutputPartIn {
    /// The part's wire type name, for the rejection that names the offending kind; [`None`] for a
    /// kind [`OutputPartIn::Other`] swallowed.
    fn type_name(&self) -> Option<&'static str> {
        match self {
            Self::InputText { .. } => Some("input_text"),

            Self::InputImage { .. } => Some("input_image"),

            Self::InputFile { .. } => Some("input_file"),

            Self::Other => None,
        }
    }
}

/// Advertised function tool.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolIn {
    /// Tool kind; only `function` is supported.
    #[serde(rename = "type")]
    tool_type: String,

    /// Function name.
    name: String,

    /// Human-readable description.
    #[serde(default)]
    description: Option<String>,

    /// JSON Schema of accepted arguments.
    #[serde(default)]
    parameters: Option<Value>,

    /// OpenAI strictness flag; accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    strict: Option<bool>,
}

/// Tool choice: a named mode or a forced function.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ToolChoiceIn {
    /// Named mode (`auto`, `none`, `required`).
    Mode(String),

    /// Forced function.
    Function {
        /// Tool kind; must be `function`.
        #[serde(rename = "type")]
        tool_type: String,

        /// Required function name.
        name: String,
    },
}

/// Reasoning configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReasoningIn {
    /// Reasoning effort string, opaque to the gateway: the string type is the whole constraint,
    /// and the value is carried verbatim into the upstream's own parameter (README §6.4).
    #[serde(default)]
    effort: Option<String>,

    /// Summary verbosity; only `auto` is accepted.
    #[serde(default)]
    summary: Option<String>,
}

/// Text output configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TextIn {
    /// Response format restriction.
    #[serde(default)]
    format: Option<FormatIn>,

    /// Output verbosity hint (codex `TextControls`); the IR has no verbosity channel, so it is
    /// accepted and ignored.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
    verbosity: Option<String>,
}

/// Response format, tagged by `type`; a member outside the format's own shape is rejected, and the
/// fieldless formats are empty struct variants rather than unit variants because only struct
/// variants reject unknown members.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum FormatIn {
    /// Free-form text.
    Text {},

    /// Any valid JSON value.
    JsonObject {},

    /// JSON conforming to a named schema.
    JsonSchema {
        /// Schema name.
        name: String,

        /// The JSON Schema itself.
        schema: Value,

        /// OpenAI strictness flag; accepted and ignored.
        #[serde(default)]
        #[expect(dead_code, reason = "accepted and ignored per README §6.6")]
        strict: Option<bool>,
    },
}

/// The conversion of one parsed request's input items into IR: the instruction
/// texts, the conversation messages, the opaque state items with their offsets,
/// the ordered compaction boundaries, and whether a trigger control was
/// present.
pub(super) struct InputConversion {
    /// System/developer message texts, in encounter order.
    pub(super) instruction_parts: Vec<String>,

    /// The same items typed for the remote-compaction identity pass: role and
    /// text per instruction item, in encounter order.
    pub(super) instruction_items: Vec<ParsedInstructionItem>,

    /// Conversation messages in order.
    pub(super) messages: Vec<IrMessage>,

    /// Opaque state items recognized in the history, in history order, each at
    /// its message offset.
    pub(super) state_items: Vec<StateItem>,

    /// Compaction state markers in history order.
    pub(super) boundaries: Vec<CompactionBoundary>,

    /// Whether a `compaction_trigger` control item was present.
    pub(super) has_trigger: bool,
}

/// Converts parsed input items into IR: messages and instruction parts as the
/// ordinary path always has, plus the remote-compaction state vocabulary —
/// reasoning items carrying `encrypted_content` become opaque native state
/// items, and compaction items split on the carrier namespace into gateway
/// boundaries and native boundaries, each state item riding both
/// `state_items` and its boundary at the same message offset.
///
/// The explicit trigger control is validated here: a trigger must be the one
/// and last input item, so any item after a trigger, and any second trigger,
/// is a 400 (README §6.8).
///
/// # Errors
///
/// Returns [`InvalidRequest`] for unknown roles, non-text instruction or tool
/// content, malformed arguments or image references, and misplaced or repeated
/// trigger items.
pub(super) fn convert_input(items: Vec<ItemIn>) -> Result<InputConversion, InvalidRequest> {
    let mut conversion = InputConversion {
        instruction_parts: Vec::new(),
        instruction_items: Vec::new(),
        messages: Vec::new(),
        state_items: Vec::new(),
        boundaries: Vec::new(),
        has_trigger: false,
    };

    for item in items {
        let is_trigger = matches!(item, ItemIn::CompactionTrigger { .. });

        if conversion.has_trigger && !is_trigger {
            return Err(InvalidRequest::field(
                "input",
                "compaction_trigger must be the last input item",
            ));
        }

        match item {
            ItemIn::Message { role, content, .. } => match role.as_str() {
                "system" | "developer" => {
                    let text = message_content_text(&content)?;

                    conversion.instruction_items.push(ParsedInstructionItem {
                        role: if role == "system" {
                            InstructionRole::System
                        } else {
                            InstructionRole::Developer
                        },
                        text: text.clone(),
                    });

                    conversion.instruction_parts.push(text);
                }

                "user" => conversion.messages.push(IrMessage {
                    role: IrRole::User,
                    parts: message_content_parts(&content)?,
                }),

                "assistant" => conversion.messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts: message_content_parts(&content)?,
                }),

                other => {
                    return Err(InvalidRequest::field(
                        "role",
                        format!("Unknown input message role `{other}`"),
                    ));
                }
            },

            ItemIn::FunctionCall {
                call_id,
                name,
                arguments,
                thought_signature,
                ..
            } => {
                format::arguments_as_object(&arguments)?;

                conversion.messages.push(IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::ToolCall {
                        call_id,
                        name,
                        arguments,
                        thought_signature,
                    }],
                });
            }

            ItemIn::FunctionCallOutput {
                call_id, output, ..
            } => {
                conversion.messages.push(IrMessage {
                    role: IrRole::Tool,
                    parts: vec![IrPart::ToolOutput {
                        call_id,
                        content: tool_output_text(&output)?,
                        is_error: false,
                    }],
                });
            }

            ItemIn::Reasoning(reasoning) => {
                // A reasoning item without encrypted content carries no
                // replayable state and is dropped, exactly as before the
                // compaction feature; one with it is opaque native state
                // replayed verbatim at its position.
                if reasoning.encrypted_content.is_some() {
                    conversion.state_items.push(StateItem {
                        offset: conversion.messages.len(),
                        item: compaction::reasoning_item_json(&reasoning),
                        is_gateway: false,
                    });
                }
            }

            ItemIn::Compaction(compaction_item) => {
                let offset = conversion.messages.len();

                let item = compaction::replayed_compaction_item_json(
                    compaction_item.id,
                    compaction_item.encrypted_content.clone(),
                    compaction_item.created_by,
                );

                if sealing::is_gateway_marker(&compaction_item.encrypted_content) {
                    conversion.state_items.push(StateItem {
                        offset,
                        item,
                        is_gateway: true,
                    });

                    conversion.boundaries.push(CompactionBoundary::Gateway {
                        offset,
                        carrier: compaction_item.encrypted_content,
                        visible_content: None,
                    });
                } else {
                    conversion.state_items.push(StateItem {
                        offset,
                        item,
                        is_gateway: false,
                    });

                    conversion.boundaries.push(CompactionBoundary::Native {
                        offset,
                        artifact: NativeArtifact::ResponsesCompactionItem,
                    });
                }
            }

            ItemIn::CompactionTrigger {} => {
                if conversion.has_trigger {
                    return Err(InvalidRequest::field(
                        "input",
                        "at most one compaction_trigger item is allowed",
                    ));
                }

                conversion.has_trigger = true;
            }

            ItemIn::ConfigurationUpdate { .. } => {
                return Err(InvalidRequest::field(
                    "input",
                    "configuration_update items carry durable configuration controls the \
                     gateway cannot apply or preserve across compaction; stateless history \
                     replay of such controls is not supported",
                ));
            }
        }
    }

    Ok(conversion)
}

/// Validates the `context_management` entries and maps them into the parsed
/// automatic control.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `type` for any entry kind other than
/// `compaction`.
fn context_management_entries(
    entries: &[ContextManagementEntryIn],
) -> Result<Vec<AutomaticEntry>, InvalidRequest> {
    let mut converted = Vec::with_capacity(entries.len());

    for entry in entries {
        if entry.entry_type != "compaction" {
            return Err(InvalidRequest::field(
                "type",
                format!(
                    "Unsupported context management entry type `{}`",
                    entry.entry_type
                ),
            ));
        }

        converted.push(AutomaticEntry {
            compact_threshold: entry.compact_threshold,
        });
    }

    Ok(converted)
}

/// Parses a Responses request body into IR.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed JSON, unknown or unsupported fields, invalid parameter
/// ranges, invalid image references, and ambiguous or misplaced compaction controls.
pub(super) fn parse_request(body: &[u8]) -> Result<ParsedRequest, InvalidRequest> {
    let request: RequestBody = format::parse_client_request(body)?;

    if request.store == Some(true) {
        return Err(InvalidRequest::field(
            "store",
            "The gateway is stateless; store must be false",
        ));
    }

    if let Some(reasoning) = &request.reasoning
        && let Some(summary) = &reasoning.summary
        && summary != "auto"
    {
        return Err(InvalidRequest::field(
            "summary",
            format!("Unsupported reasoning summary `{summary}`"),
        ));
    }

    format::validate_sampling_ranges(request.temperature, request.top_p)?;

    let context_management = match &request.context_management {
        None => None,
        Some(entries) => Some(context_management_entries(entries)?),
    };

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

    // One explicit compaction control per request: a trigger beside the
    // automatic controls is ambiguous, and ambiguity is rejected instead of
    // silently interpreted (README §6.8).
    if converted.has_trigger && context_management.is_some() {
        return Err(InvalidRequest::request(
            "compaction_trigger and context_management are ambiguous; state one compaction \
             control",
        ));
    }

    let control = if converted.has_trigger {
        Some(CompactionControl::ResponsesTrigger)
    } else if let Some(entries) = context_management {
        Some(CompactionControl::ResponsesAutomatic { entries })
    } else {
        None
    };

    let compaction =
        if control.is_none() && converted.boundaries.is_empty() && converted.state_items.is_empty()
        {
            None
        } else {
            Some(ParsedCompaction {
                control,
                boundaries: converted.boundaries,
                instruction_items: converted.instruction_items,
            })
        };

    let instructions = build_instructions(request.instructions, converted.instruction_parts);

    let mut tools: Vec<IrTool> = Vec::new();

    if let Some(tools_in) = &request.tools {
        for tool in tools_in {
            format::reject_non_function_type(&tool.tool_type, "tool")?;

            tools.push(IrTool {
                name: tool.name.clone(),
                description: tool.description.clone(),
                parameters: tool.parameters.clone(),
            });
        }
    }

    let tool_choice = match &request.tool_choice {
        None => None,

        Some(choice) => {
            format::reject_tool_choice_without_tools(tools.is_empty())?;

            Some(match choice {
                ToolChoiceIn::Mode(mode) => format::openai::parse_tool_choice_mode(mode)?,

                ToolChoiceIn::Function { tool_type, name } => {
                    format::reject_non_function_type(tool_type, "tool_choice")?;

                    IrToolChoice::Function { name: name.clone() }
                }
            })
        }
    };

    let response_format = match request.text.as_ref().and_then(|text| text.format.as_ref()) {
        None => None,
        Some(FormatIn::Text {}) => Some(IrResponseFormat::Text),
        Some(FormatIn::JsonObject {}) => Some(IrResponseFormat::Json),

        Some(FormatIn::JsonSchema {
            name,
            schema,
            strict: _,
        }) => Some(IrResponseFormat::JsonSchema {
            name: name.clone(),
            schema: schema.clone(),
        }),
    };

    // The closed vocabulary is enforced here: an unknown effort string is a 400 naming the field,
    // and the parsed enum renders verbatim into the upstream's own parameter (README §6.4).
    // `summary` keeps its own validation above.
    let reasoning_effort = request
        .reasoning
        .and_then(|reasoning| reasoning.effort)
        .map(|effort| IrReasoningLevel::parse(&effort, "reasoning"))
        .transpose()?;

    Ok(ParsedRequest {
        affinity: crate::format::affinity::Sources::from_wire_messages(
            crate::format::UpstreamFormat::OpenAiResponse,
            &converted.messages,
        ),
        ir: IrRequest {
            state_items: converted.state_items,
            model: request.model,
            instructions,
            messages: converted.messages,
            tools,
            tool_choice,
            temperature: request.temperature,
            top_p: request.top_p,
            max_output_tokens: request.max_output_tokens,
            response_format,
            reasoning_effort,
        },
        is_streaming: request.stream.unwrap_or(false),
        should_include_usage: false,
        compaction,
    })
}

/// Combines the `instructions` field with system/developer message texts in encounter order.
pub(super) fn build_instructions(direct: Option<String>, pushed: Vec<String>) -> Option<String> {
    let mut combined = Vec::with_capacity(pushed.len() + 1);

    if let Some(direct) = direct {
        combined.push(direct);
    }

    combined.extend(pushed);

    if combined.is_empty() {
        None
    } else {
        Some(combined.join("\n\n"))
    }
}

/// Extracts plain text from system/developer message content.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming the offending content part when the message carries non-text
/// parts: instructions have no image channel, so silently dropping parts would lose request
/// content.
fn message_content_text(content: &StringOrArray<PartIn>) -> Result<String, InvalidRequest> {
    match content {
        StringOrArray::Text(text) => Ok(text.clone()),

        StringOrArray::Array(parts) => {
            let texts = parts
                .iter()
                .map(|part| match part {
                    PartIn::InputText { text } | PartIn::OutputText { text, .. } => {
                        Ok(text.as_str())
                    }

                    PartIn::InputImage { .. } => Err(InvalidRequest::field(
                        "content",
                        "System and developer message content parts must be text; \
                         `input_image` is not supported",
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;

            // The separator only ever separates text the accumulator already holds, so a
            // leading run of empty parts carries none: skip it, then join the rest with the
            // blank-line separator.
            let combined = texts
                .iter()
                .skip_while(|text| text.is_empty())
                .copied()
                .collect::<Vec<&str>>()
                .join("\n\n");

            Ok(combined)
        }
    }
}

/// Converts message content into IR parts.
///
/// # Errors
///
/// Returns [`InvalidRequest`] for malformed image references.
fn message_content_parts(content: &StringOrArray<PartIn>) -> Result<Vec<IrPart>, InvalidRequest> {
    match content {
        StringOrArray::Text(text) => Ok(vec![IrPart::Text { text: text.clone() }]),

        StringOrArray::Array(parts) => {
            let mut converted = Vec::with_capacity(parts.len());

            for part in parts {
                converted.push(match part {
                    PartIn::InputText { text } | PartIn::OutputText { text, .. } => {
                        IrPart::Text { text: text.clone() }
                    }

                    PartIn::InputImage { image_url, .. } => IrPart::Image {
                        image: IrImage::from_url(image_url)?,
                    },
                });
            }

            Ok(converted)
        }
    }
}

/// Extracts plain text from a `function_call_output` item's `output`. Text parts join with the same
/// blank-line separator the Chat Completions codec uses for its text-parts content, so the array
/// form flattens to exactly what the equivalent plain-string form carries.
///
/// # Errors
///
/// Returns [`InvalidRequest`] naming `output` for a recognized non-text part: the IR tool output
/// has no media channel, so silently dropping such a part would lose request content.
fn tool_output_text(output: &StringOrArray<OutputPartIn>) -> Result<String, InvalidRequest> {
    let parts = match output {
        StringOrArray::Text(text) => return Ok(text.clone()),

        StringOrArray::Array(parts) => parts,
    };

    let texts = parts
        .iter()
        .map(|part| match part {
            OutputPartIn::InputText { text } => Ok(text.as_str()),

            other => {
                let message = match other.type_name() {
                    Some(part_type) => format!(
                        "function_call_output output parts must be text; \
                         `{part_type}` is not supported"
                    ),

                    None => "function_call_output output parts must be text".to_string(),
                };

                Err(InvalidRequest::field("output", message))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;

    // The separator only ever separates text the accumulator already holds, so a leading run
    // of empty parts carries none: skip it, then join the rest with the blank-line separator.
    let combined = texts
        .iter()
        .skip_while(|text| text.is_empty())
        .copied()
        .collect::<Vec<&str>>()
        .join("\n\n");

    Ok(combined)
}

#[cfg(test)]
mod tests {
    use super::parse_request;
    use crate::{
        format::{CompactionControl, ParsedRequest},
        ir::{InvalidRequest, IrPart, IrResponseFormat, IrRole},
    };
    use serde_json::Value;

    /// Parses one request body given as JSON.
    fn parse(body: Value) -> Result<ParsedRequest, InvalidRequest> {
        parse_request(body.to_string().as_bytes())
    }

    /// A `message` input item carrying the gateway-rendered `id` and `status` fields parses: the
    /// gateway's own assistant message output items carry both, so replaying a response verbatim as
    /// the next request's input — the codex replay loop — must parse (README §6.6 history-replay
    /// carve-outs).
    ///
    /// # Panics
    ///
    /// Panics if the parsed request diverges from the documented IR.
    #[test]
    fn message_input_item_with_status_parses() {
        #[expect(
            clippy::expect_used,
            reason = "the replayed assistant message is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "hello",
                            "annotations": [],
                            "logprobs": [],
                        },
                    ],
                },
            ],
        }))
        .expect("a replayed assistant message output item must parse");

        assert_eq!(parsed.ir.messages.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees two messages"
        )]
        let assistant = &parsed.ir.messages[1];

        assert_eq!(assistant.role, IrRole::Assistant);

        assert_eq!(
            assistant.parts,
            vec![IrPart::Text {
                text: "hello".to_string(),
            }],
        );
    }

    /// An `input_image` part carrying the official `detail` hint parses to the same IR as the part
    /// without it: typed SDK clients send `detail` on every `input_image`, and the gateway cannot
    /// honor it downstream, so it is accepted and ignored like `strict` (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn input_image_detail_is_accepted_and_ignored() {
        #[expect(
            clippy::expect_used,
            reason = "the plain image body is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/i.png",
                    },
                ]},
            ],
        }))
        .expect("a plain input_image part must parse");

        #[expect(
            clippy::expect_used,
            reason = "the detailed image body is constructed to parse"
        )]
        let detailed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {
                        "type": "input_image",
                        "image_url": "https://example.com/i.png",
                        "detail": "high",
                    },
                ]},
            ],
        }))
        .expect("an input_image part carrying `detail` must parse");

        assert_eq!(detailed.ir.messages, plain.ir.messages);
    }

    /// A `function_call_output` item accepts the official plain-string form and the content-parts
    /// array form, and the text-parts array flattens to exactly the string form's tool output with
    /// the blank-line separator (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn function_call_output_string_and_text_parts_agree() {
        #[expect(
            clippy::expect_used,
            reason = "the string-form output is constructed to parse"
        )]
        let string_form = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call_output", "call_id": "call_1", "output": "x\n\ny"},
            ],
        }))
        .expect("a string output must parse");

        #[expect(
            clippy::expect_used,
            reason = "the parts-form output is constructed to parse"
        )]
        let parts_form = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "function_call_output", "call_id": "call_1", "output": [
                    {"type": "input_text", "text": "x"},
                    {"type": "input_text", "text": "y"},
                ]},
            ],
        }))
        .expect("a text-parts output must parse");

        assert_eq!(parts_form.ir.messages, string_form.ir.messages);

        #[expect(
            clippy::indexing_slicing,
            reason = "the parsed body carries exactly one message"
        )]
        let parts = &parts_form.ir.messages[0].parts;

        assert_eq!(
            *parts,
            vec![IrPart::ToolOutput {
                call_id: "call_1".to_string(),
                content: "x\n\ny".to_string(),
                is_error: false,
            }],
        );
    }

    /// System and developer message content-parts arrays flatten with the blank-line separator:
    /// parts join in order, an empty part between texts keeps both its separators, and a leading
    /// run of empty parts carries none, so the array form flattens to exactly what the equivalent
    /// plain-string form carries (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed instructions diverge from the documented flattening.
    #[test]
    fn system_developer_text_part_arrays_flatten_to_the_string_form() {
        #[expect(
            clippy::expect_used,
            reason = "the system parts form is constructed to parse"
        )]
        let system = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "system", "content": [
                    {"type": "input_text", "text": "a"},
                    {"type": "input_text", "text": ""},
                    {"type": "input_text", "text": "b"},
                ]},
            ],
        }))
        .expect("system content parts must parse");

        assert_eq!(system.ir.instructions.as_deref(), Some("a\n\n\n\nb"));

        #[expect(
            clippy::expect_used,
            reason = "the developer parts form is constructed to parse"
        )]
        let developer = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "developer", "content": [
                    {"type": "output_text", "text": ""},
                    {"type": "output_text", "text": "c"},
                ]},
            ],
        }))
        .expect("developer content parts must parse");

        assert_eq!(developer.ir.instructions.as_deref(), Some("c"));
    }

    /// The full codex CLI request envelope parses: with `wire_api = "responses"` the CLI always
    /// serializes `parallel_tool_calls`, `include`, `prompt_cache_key`, `text.verbosity`,
    /// `service_tier`, `client_metadata`, and `access_programs` beside the supported fields, and
    /// rejecting any of them would 400 the client's first request before the item-level carve-outs
    /// are ever reached (README §6.6). The supported fields inside the envelope — streaming,
    /// reasoning effort, and `text.format` — keep their semantics.
    ///
    /// # Panics
    ///
    /// Panics if the parsed envelope diverges from the documented IR.
    #[test]
    fn codex_envelope_parses() {
        #[expect(
            clippy::expect_used,
            reason = "the codex CLI envelope is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "instructions": "be terse",
            "input": [{"type": "message", "role": "user", "content": "hi"}],
            "tools": [{"type": "function", "name": "shell"}],
            "tool_choice": "auto",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high", "summary": "auto"},
            "store": false,
            "stream": true,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "cache-key",
            "text": {"format": {"type": "text"}, "verbosity": "medium"},
            "service_tier": "auto",
            "client_metadata": {"originator": "codex_cli_rs"},
            "access_programs": ["codex"],
            "stream_options": {"include_usage": true},
        }))
        .expect("the codex CLI request envelope must parse");

        assert!(parsed.is_streaming);

        assert_eq!(
            parsed.ir.reasoning_effort,
            Some(crate::ir::IrReasoningLevel::High)
        );

        assert_eq!(parsed.ir.response_format, Some(IrResponseFormat::Text));
    }

    /// The envelope control fields are accepted and ignored: the same request with and without them
    /// parses to identical IR, so their presence never changes routing or the upstream render
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the parsed requests diverge from the documented IR.
    #[test]
    fn envelope_control_fields_are_accepted_and_ignored() {
        #[expect(
            clippy::expect_used,
            reason = "the plain request is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "text": {"format": {"type": "json_object"}},
        }))
        .expect("the plain request must parse");

        #[expect(
            clippy::expect_used,
            reason = "the decorated request is constructed to parse"
        )]
        let decorated = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "parallel_tool_calls": false,
            "include": ["reasoning.encrypted_content"],
            "prompt_cache_key": "cache-key",
            "stream_options": {"include_usage": true},
            "service_tier": "default",
            "client_metadata": {"originator": "test"},
            "access_programs": ["codex"],
            "text": {"format": {"type": "json_object"}, "verbosity": "low"},
        }))
        .expect("the decorated request must parse");

        assert_eq!(decorated.ir.messages, plain.ir.messages);

        assert_eq!(decorated.ir.response_format, plain.ir.response_format);

        assert_eq!(decorated.is_streaming, plain.is_streaming);
    }

    /// A non-text part in a `function_call_output` output array is rejected naming `output`: the IR
    /// tool output has no media channel, so image and file parts cannot be carried (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn function_call_output_non_text_parts_are_rejected() {
        for part_type in ["input_image", "input_file"] {
            let part = if part_type == "input_image" {
                serde_json::json!({
                    "type": "input_image",
                    "image_url": "https://example.com/i.png",
                })
            } else {
                serde_json::json!({
                    "type": "input_file",
                    "file_id": "file_1",
                })
            };

            #[expect(
                clippy::expect_used,
                reason = "each non-text part is constructed to be rejected"
            )]
            let error = parse(serde_json::json!({
                "model": "m",
                "input": [
                    {"type": "function_call_output", "call_id": "call_1", "output": [
                        {"type": "input_text", "text": "before"},
                        part,
                    ]},
                ],
            }))
            .expect_err("a non-text part in a function_call_output output must be rejected");

            assert_eq!(error.param.as_deref(), Some("output"));

            assert_eq!(
                error.to_string(),
                format!(
                    "function_call_output output parts must be text; \
                     `{part_type}` is not supported"
                ),
            );
        }
    }

    /// Unknown members are rejected wherever the protocol defines a shape: an item member outside
    /// the item's own shape, a content-part member outside the part's own shape, a member beside a
    /// fieldless response format, and a member beside the request envelope's own fields. serde's
    /// rejection names the offending member, which is the contract README §6.6 documents.
    ///
    /// # Panics
    ///
    /// Panics if a rejection lands outside its documented wording.
    #[test]
    fn unknown_members_are_rejected() {
        let cases = [
            (
                serde_json::json!({
                    "model": "m",
                    "input": [
                        {"type": "message", "role": "user", "content": "hi", "bogus": 1},
                    ],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "input": [{
                        "type": "message",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "hi", "bogus": 1}],
                    }],
                }),
                "bogus",
            ),
            (
                serde_json::json!({
                    "model": "m",
                    "input": "hi",
                    "text": {"format": {"type": "text", "bogus": 1}},
                }),
                "bogus",
            ),
            (
                serde_json::json!({"model": "m", "input": "hi", "bogus": 1}),
                "bogus",
            ),
        ];

        for (body, field) in cases {
            #[expect(
                clippy::expect_used,
                reason = "each unknown member is constructed to be rejected"
            )]
            let error = parse(body).expect_err("an unknown member must be rejected");

            assert!(
                error
                    .to_string()
                    .contains(&format!("unknown field `{field}`")),
                "unexpected rejection: {error}"
            );
        }
    }

    /// A `reasoning` input item carries no replayable state, so its payload is accepted in whatever
    /// shape it arrives — unknown members included — while every other item shape stays strict
    /// (README §6.6).
    ///
    /// # Panics
    ///
    /// Panics if the reasoning item is rejected or not ignored.
    #[test]
    fn reasoning_items_are_accepted_in_any_shape() {
        #[expect(
            clippy::expect_used,
            reason = "the reasoning payload is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "thought"}],
                    "encrypted_content": "ciphertext",
                },
            ],
        }))
        .expect("a reasoning item payload must parse in any shape");

        assert_eq!(parsed.ir.messages.len(), 1);
    }

    /// `reasoning.effort` parses into the closed seven-value vocabulary (README §6.4): known
    /// values parse, unknown strings are a 400 naming the field, and a non-string value is a 400
    /// malformed request. `summary` keeps its own validation.
    ///
    /// # Panics
    ///
    /// Panics if the parsed effort or rejection diverges from the contract.
    #[test]
    fn reasoning_effort_parses_the_closed_vocabulary() {
        use crate::ir::IrReasoningLevel;

        for (level, effort) in IrReasoningLevel::VOCABULARY {
            #[expect(
                clippy::expect_used,
                reason = "a vocabulary string is constructed to parse"
            )]
            let parsed = parse(serde_json::json!({
                "model": "m",
                "input": "hi",
                "reasoning": {"effort": effort},
            }))
            .expect("a vocabulary effort string must parse");

            assert_eq!(parsed.ir.reasoning_effort, Some(level));
        }

        #[expect(
            clippy::expect_used,
            reason = "the unknown value is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "reasoning": {"effort": "ultra"},
        }))
        .expect_err("an unknown effort must be rejected");

        assert_eq!(error.param.as_deref(), Some("reasoning"));

        #[expect(
            clippy::expect_used,
            reason = "the non-string value is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "reasoning": {"effort": 5},
        }))
        .expect_err("a non-string effort must be rejected as malformed");

        assert!(
            error.to_string().contains("invalid type"),
            "a non-string effort fails on its type alone: {error}"
        );
    }

    /// A terminal `compaction_trigger` item parses into the explicit trigger
    /// control: exactly one trigger, last in the input (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the parsed control diverges from the contract.
    #[test]
    fn a_terminal_trigger_parses_into_the_trigger_control() {
        #[expect(
            clippy::expect_used,
            reason = "the trigger request is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "compaction_trigger"},
            ],
        }))
        .expect("a terminal trigger parses");

        #[expect(
            clippy::expect_used,
            reason = "a trigger request is constructed to carry compaction facts"
        )]
        let compaction = parsed
            .compaction
            .expect("a trigger request carries compaction facts");

        assert!(matches!(
            compaction.control,
            Some(CompactionControl::ResponsesTrigger)
        ));

        assert!(compaction.boundaries.is_empty());

        // The trigger is a control item, not conversation content: it becomes
        // no message and no state item.
        assert_eq!(parsed.ir.messages.len(), 1);

        assert!(parsed.ir.state_items.is_empty());
    }

    /// A trigger anywhere but the last position is rejected, and so is a
    /// second trigger: the synthesized compact-only contract accepts one
    /// terminal trigger and rejects ambiguity instead of silently
    /// interpreting it (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if a misplaced or repeated trigger parses.
    #[test]
    fn misplaced_and_repeated_triggers_are_rejected() {
        #[expect(
            clippy::expect_used,
            reason = "the misplaced trigger is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "compaction_trigger"},
                {"type": "message", "role": "user", "content": "hi"},
            ],
        }))
        .expect_err("a trigger followed by an item must be rejected");

        assert_eq!(error.param.as_deref(), Some("input"));

        #[expect(
            clippy::expect_used,
            reason = "the repeated trigger is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "compaction_trigger"},
                {"type": "compaction_trigger"},
            ],
        }))
        .expect_err("a second trigger must be rejected");

        assert_eq!(error.param.as_deref(), Some("input"));
    }

    /// `context_management` parses into the automatic control with its
    /// thresholds in order, an entry of any other kind is rejected naming
    /// `type`, and a trigger beside the automatic controls is rejected as
    /// ambiguous (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the parsed controls or rejections diverge from the contract.
    #[test]
    fn context_management_parses_and_rejects_ambiguity() {
        #[expect(
            clippy::expect_used,
            reason = "the automatic request is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "context_management": [
                {"type": "compaction"},
                {"type": "compaction", "compact_threshold": 50_000},
            ],
        }))
        .expect("context_management entries parse");

        #[expect(
            clippy::expect_used,
            reason = "an automatic request is constructed to carry compaction facts"
        )]
        let compaction = parsed
            .compaction
            .expect("an automatic request carries compaction facts");

        #[expect(
            clippy::panic,
            reason = "the parsed control is constructed to be the automatic control"
        )]
        let Some(CompactionControl::ResponsesAutomatic { entries }) = compaction.control else {
            panic!("context_management parses into the automatic control");
        };

        assert_eq!(entries.len(), 2);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees two entries"
        )]
        {
            assert_eq!(entries[0].compact_threshold, None);

            assert_eq!(entries[1].compact_threshold, Some(50_000));
        }

        #[expect(
            clippy::expect_used,
            reason = "the foreign entry kind is constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
            "context_management": [{"type": "clear_context"}],
        }))
        .expect_err("a foreign entry kind must be rejected");

        assert_eq!(error.param.as_deref(), Some("type"));

        #[expect(
            clippy::expect_used,
            reason = "the mixed controls are constructed to be rejected"
        )]
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {"type": "compaction_trigger"},
            ],
            "context_management": [{"type": "compaction"}],
        }))
        .expect_err("a trigger beside context_management must be rejected");

        assert!(
            error.to_string().contains("ambiguous"),
            "unexpected rejection: {error}"
        );
    }

    /// A native compaction item in ordinary history becomes a native boundary
    /// and an opaque state item without any control — a pure replay request —
    /// while a request carrying neither state nor controls keeps `compaction`
    /// `None` (README §6.8).
    ///
    /// # Panics
    ///
    /// Panics if the parsed compaction facts diverge from the contract.
    #[test]
    fn replay_only_history_carries_boundaries_without_a_control() {
        #[expect(
            clippy::expect_used,
            reason = "the replay request is constructed to parse"
        )]
        let parsed = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "message", "role": "user", "content": "hi"},
                {
                    "type": "compaction",
                    "id": "cmp_1",
                    "encrypted_content": "native-state",
                },
            ],
        }))
        .expect("a replay request with a native compaction item parses");

        #[expect(
            clippy::expect_used,
            reason = "a replay request is constructed to carry its boundaries"
        )]
        let compaction = parsed
            .compaction
            .expect("a replay request carries its boundaries");

        assert!(compaction.control.is_none());

        assert_eq!(compaction.boundaries.len(), 1);

        assert_eq!(parsed.ir.state_items.len(), 1);

        #[expect(
            clippy::indexing_slicing,
            reason = "the preceding length assertion guarantees one state item"
        )]
        {
            assert_eq!(parsed.ir.state_items[0].is_gateway, false);

            assert_eq!(
                parsed.ir.state_items[0].item.get("type"),
                Some(&serde_json::json!("compaction")),
            );
        }

        #[expect(
            clippy::expect_used,
            reason = "the plain request is constructed to parse"
        )]
        let plain = parse(serde_json::json!({
            "model": "m",
            "input": "hi",
        }))
        .expect("a plain request parses");

        assert!(plain.compaction.is_none());
    }

    /// A `configuration_update` control item is rejected explicitly, never
    /// silently discarded into prose or forwarded to another endpoint.
    ///
    /// # Panics
    ///
    /// Panics if the rejection does not carry the item name.
    #[test]
    #[expect(
        clippy::expect_used,
        reason = "the tested body is constructed to be rejected; the rejection is the assertion"
    )]
    fn rejects_a_configuration_update_control_item() {
        let error = parse(serde_json::json!({
            "model": "m",
            "input": [
                {"type": "configuration_update", "control": null}
            ],
        }))
        .expect_err("the control item is rejected");

        assert_eq!(error.param.as_deref(), Some("input"));
    }
}
