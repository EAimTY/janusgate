//! Gateway summary inference (README §6.8): the first-party default
//! summarization prompt, the planner that shapes the internal summarizer
//! request, and the acceptance rule the summary must satisfy.
//!
//! The summarizer is an internal dispatch on the same routed provider/model
//! through the normal credential path — never the public endpoint, never a
//! provider switch. Supplied compaction instructions replace the default
//! prompt rather than appending to it; execution constraints stay external to
//! the prompt, so custom instructions never authorize tools, a provider
//! change, or dropped replay state. The model must produce summary text only:
//! tool selection is off for the inference (definitions may ride along only
//! when retained native state requires their exact prefix), output schemas and
//! stop sequences never propagate, and only a complete, nonempty, tool-free
//! summary ending normally is accepted — no second attempt, no larger budget,
//! no partial summary packaged as success.

use super::{error::CompactionError, history::GatewayCheckpointPlan};
use crate::ir::{
    IrAggregation, IrFinishReason, IrMessage, IrPart, IrRequest, IrRole, IrToolChoice,
};

/// The first-party default summarization prompt (README §6.8): its
/// contract is to preserve the objective, the active instructions, the
/// decisions made, the constraints in force, the completed work, the unresolved
/// work, the references, the tool facts, and the next actions, as text only.
const DEFAULT_SUMMARY_PROMPT: &str = "You are summarizing the earlier part of a \
conversation so it can be replaced by your summary. Produce the summary as plain prose text and \
nothing else: no preamble, no headings markup beyond plain lines, no tool calls. Preserve, to the \
extent the conversation supplies them: the user's objective; the instructions currently in force; \
the decisions made so far; the constraints and preferences that must keep being honored; the work \
already completed, with its outcomes; the work still unresolved; the referenced files, links, \
identifiers, and other artifacts; the relevant tool facts — which tools were called, with what, \
and what they returned; and the concrete next actions. Keep every fact needed to continue the \
work without the replaced history, and invent nothing.";

/// The fixed trailing user instruction that closes the summarizer's input.
const SUMMARY_CLOSING_INSTRUCTION: &str = "Produce the summary now.";

/// Builds the internal summarizer request from the restored window and the
/// gateway checkpoint plan (README §6.8).
///
/// The request carries the restored window's serving facts with: the supplied
/// instructions replacing the default prompt (their absence keeping the
/// default); the eligible older prefix as input history plus the fixed closing
/// user instruction; tool selection off; the tool definitions kept only when
/// the retained region carries native state whose replay may require the exact
/// tool prefix, and dropped otherwise; the output bound by the summary budget
/// and the caller's compact-only output cap, whichever is smaller; the response
/// format dropped; and the sampling controls and reasoning effort preserved.
/// The opaque state items never ride the summarizer.
///
/// The request is built field by field from the borrowed window, so the
/// covered history and state are never cloned just to be overwritten.
///
/// Consumes the plan's summary input messages: they become the summarizer's
/// input history, and no later consumer of the plan reads them.
#[must_use]
pub(super) fn plan_summary_request(
    restored: &IrRequest,
    plan: &mut GatewayCheckpointPlan,
    instructions: Option<&str>,
    output_cap: Option<u64>,
) -> IrRequest {
    let mut messages = std::mem::take(&mut plan.summary_input_messages);

    messages.push(IrMessage {
        role: IrRole::User,
        parts: vec![IrPart::Text {
            text: String::from(SUMMARY_CLOSING_INSTRUCTION),
        }],
    });

    IrRequest {
        has_compaction_trigger: restored.has_compaction_trigger,
        model: restored.model.clone(),
        instructions: Some(
            instructions
                .map(str::to_string)
                .unwrap_or_else(|| String::from(DEFAULT_SUMMARY_PROMPT)),
        ),
        messages,
        // Tool selection is off for the inference; the definitions ride along
        // only when the retained native state may require their exact prefix,
        // so suppressing them cannot invalidate the state being summarized.
        tools: if plan.retained_state_items.is_empty() {
            Vec::new()
        } else {
            restored.tools.clone()
        },
        tool_choice: Some(IrToolChoice::None),
        temperature: restored.temperature,
        top_p: restored.top_p,
        max_output_tokens: Some(match output_cap {
            Some(cap) => plan.summary_budget.min(cap),
            None => plan.summary_budget,
        }),
        response_format: None,
        reasoning_effort: restored.reasoning_effort,
        state_items: Vec::new(),
    }
}

/// Validates a completed summary inference and returns its text.
///
/// Only a complete, nonempty summary ending normally without a tool call is
/// usable: the finish must be the plain stop (a truncation, a content filter,
/// or a tool-call finish all disqualify), the text must be nonempty, and the
/// response must carry no tool call the gateway would never execute.
///
/// # Errors
///
/// Returns [`CompactionError::UnusableSummary`] for every disqualifying shape.
pub(super) fn validate_summary(aggregation: &IrAggregation) -> Result<String, CompactionError> {
    let Some(finish) = aggregation.finish else {
        return Err(CompactionError::unusable_summary(
            "the summary inference ended without a finish",
        ));
    };

    if finish != IrFinishReason::Stop {
        return Err(CompactionError::unusable_summary(format!(
            "the summary inference ended with finish `{finish:?}` instead of a normal stop"
        )));
    }

    if aggregation.text.trim().is_empty() {
        return Err(CompactionError::unusable_summary(
            "the summary inference produced no text",
        ));
    }

    if !aggregation.tool_calls.is_empty() {
        return Err(CompactionError::unusable_summary(
            "the summary inference requested a tool call, which the gateway never executes",
        ));
    }

    Ok(aggregation.text.clone())
}
