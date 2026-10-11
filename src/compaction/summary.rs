//! Gateway summary inference (`docs/design/compaction.md`): the first-party default
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

/// The first-party default summarization prompt (`docs/design/compaction.md`): its
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
/// gateway checkpoint plan (`docs/design/compaction.md`).
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::missing_panics_doc,
        reason = "the tests' assertions panic; a # Panics section on every test would \
                  restate that"
    )]

    use super::{plan_summary_request, validate_summary};
    use crate::{
        compaction::history::{self, Budgets, CheckpointInstructions},
        ir::{
            IrAggregatedToolCall, IrAggregation, IrFinishReason, IrMessage, IrPart, IrRequest,
            IrRole, IrToolChoice, StateFamily, StateItem,
        },
    };
    use std::error::Error as StdError;

    /// The checkpoint plan fixture of the planner tests, built through the real planner: one
    /// unit under a zero retention budget, so the whole history is the older prefix the summary
    /// consumes.
    fn fixture_plan(
        summary_budget: u64,
    ) -> Result<history::GatewayCheckpointPlan, Box<dyn StdError>> {
        let ir = IrRequest {
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: String::from("covered history"),
                }],
            }],
            ..IrRequest::default()
        };

        Ok(history::plan_gateway_checkpoint(
            &ir,
            &CheckpointInstructions::Parsed(&[]),
            false,
            Budgets {
                summary: summary_budget,
                retention: 0,
            },
        )?)
    }

    /// The checkpoint plan fixture carrying retained native state, built through the native
    /// planner over a history whose retained region carries one opaque state item.
    fn fixture_plan_with_retained_state(
        summary_budget: u64,
    ) -> Result<history::GatewayCheckpointPlan, Box<dyn StdError>> {
        let ir = IrRequest {
            messages: vec![
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("covered history"),
                    }],
                },
                IrMessage {
                    role: IrRole::User,
                    parts: vec![IrPart::Text {
                        text: String::from("kept"),
                    }],
                },
                IrMessage {
                    role: IrRole::Assistant,
                    parts: vec![IrPart::Text {
                        text: String::from("kept answer"),
                    }],
                },
            ],
            state_items: vec![StateItem {
                offset: 1,
                item: serde_json::json!({"type": "compaction", "content": null}),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Messages,
            }],
            ..IrRequest::default()
        };

        Ok(history::plan_native_checkpoint(
            &ir,
            &CheckpointInstructions::Parsed(&[]),
            Budgets {
                summary: summary_budget,
                retention: 0,
            },
        )?)
    }

    /// The restored window the planner tests borrow.
    fn fixture_restored() -> IrRequest {
        IrRequest {
            model: String::from("public-model"),
            instructions: Some(String::from("original instructions")),
            messages: vec![IrMessage {
                role: IrRole::User,
                parts: vec![IrPart::Text {
                    text: String::from("covered history"),
                }],
            }],
            tools: vec![crate::ir::IrTool {
                name: String::from("lookup"),
                description: None,
                parameters: None,
            }],
            tool_choice: Some(IrToolChoice::Auto),
            max_output_tokens: Some(999),
            response_format: Some(crate::ir::IrResponseFormat::Json),
            state_items: vec![StateItem {
                offset: 0,
                item: serde_json::json!({"opaque": true}),
                is_gateway: false,
                is_compaction: false,
                family: StateFamily::Responses,
            }],
            ..IrRequest::default()
        }
    }

    /// The summarizer request carries the default prompt when the caller supplied none, closes
    /// with the fixed user instruction, turns tool selection off, drops the definitions without
    /// retained native state, binds the output to the summary budget, and never carries the
    /// window's format restriction, output budget, or opaque state.
    #[test]
    fn the_default_planner_shapes_the_summarizer_request() -> Result<(), Box<dyn StdError>> {
        let mut plan = fixture_plan(100)?;

        let summarizer = plan_summary_request(&fixture_restored(), &mut plan, None, None);

        assert_eq!(
            summarizer.instructions.as_deref(),
            Some(super::DEFAULT_SUMMARY_PROMPT),
            "absent compaction instructions keep the default prompt"
        );

        let [covered, closing] = summarizer.messages.as_slice() else {
            return Err(
                "the summarizer input is the older prefix plus the closing instruction".into(),
            );
        };

        assert_eq!(
            covered.parts,
            vec![IrPart::Text {
                text: String::from("covered history")
            }],
        );

        assert_eq!(closing.role, IrRole::User);

        assert_eq!(
            closing.parts,
            vec![IrPart::Text {
                text: String::from("Produce the summary now.")
            }]
        );

        assert_eq!(summarizer.tool_choice, Some(IrToolChoice::None));

        assert!(
            summarizer.tools.is_empty(),
            "definitions ride only when retained native state may require their prefix"
        );

        assert_eq!(summarizer.max_output_tokens, Some(100));

        assert_eq!(
            summarizer.response_format, None,
            "format restrictions never propagate"
        );

        assert!(
            summarizer.state_items.is_empty(),
            "opaque state never rides the summarizer"
        );

        Ok(())
    }

    /// Supplied compaction instructions replace the default prompt outright, the caller's
    /// compact-only output cap binds the summary output to the smaller of cap and budget, and
    /// retained native state keeps the restored window's tool definitions riding.
    #[test]
    fn supplied_instructions_and_caps_and_retained_state_shape_the_request()
    -> Result<(), Box<dyn StdError>> {
        let mut plan = fixture_plan(100)?;

        let summarizer = plan_summary_request(
            &fixture_restored(),
            &mut plan,
            Some("Summarize briefly."),
            Some(40),
        );

        assert_eq!(
            summarizer.instructions.as_deref(),
            Some("Summarize briefly.")
        );

        assert_eq!(
            summarizer.max_output_tokens,
            Some(40),
            "the smaller cap binds"
        );

        let mut plan = fixture_plan(100)?;

        let summarizer = plan_summary_request(
            &fixture_restored(),
            &mut plan,
            Some("Summarize briefly."),
            Some(400),
        );

        assert_eq!(
            summarizer.max_output_tokens,
            Some(100),
            "the budget binds over a larger cap"
        );

        let mut plan = fixture_plan_with_retained_state(100)?;

        assert!(
            !plan.retained_state_items.is_empty(),
            "the fixture plan carries its retained region's native state"
        );

        let summarizer = plan_summary_request(&fixture_restored(), &mut plan, None, None);

        assert_eq!(
            summarizer.tools.len(),
            1,
            "retained native state keeps the exact tool prefix riding"
        );

        assert!(
            summarizer.messages.iter().all(|message| !message
                .parts
                .iter()
                .any(|part| matches!(part, IrPart::ToolOutput { .. })),),
            "only the plan's input rides; the restored window's messages do not"
        );

        Ok(())
    }

    /// Planning consumes the plan's summary input: the covered prefix becomes the summarizer's
    /// input and no later consumer can re-read it.
    #[test]
    fn planning_consumes_the_summary_input() -> Result<(), Box<dyn StdError>> {
        let mut plan = fixture_plan(100)?;

        assert_eq!(plan.summary_input_messages.len(), 1);

        let _summarizer = plan_summary_request(&fixture_restored(), &mut plan, None, None);

        assert!(plan.summary_input_messages.is_empty());

        Ok(())
    }

    /// Only a complete, nonempty, tool-free summary ending normally validates; every other shape
    /// fails with the stable unusable-summary code, preserving the upstream's own protocol
    /// meanings for refusal and truncation.
    #[test]
    fn validation_accepts_only_complete_usable_summaries() -> Result<(), Box<dyn StdError>> {
        let usable = IrAggregation {
            text: String::from("A complete summary."),
            finish: Some(IrFinishReason::Stop),
            ..IrAggregation::default()
        };

        assert_eq!(
            validate_summary(&usable).unwrap_or_default(),
            "A complete summary."
        );

        for (aggregation, reason) in [
            (
                IrAggregation {
                    text: String::from("   "),
                    finish: Some(IrFinishReason::Stop),
                    ..IrAggregation::default()
                },
                "empty",
            ),
            (IrAggregation::default(), "no finish"),
            (
                IrAggregation {
                    text: String::from("Truncated."),
                    finish: Some(IrFinishReason::Length),
                    ..IrAggregation::default()
                },
                "truncated",
            ),
            (
                IrAggregation {
                    text: String::from("Let me call a tool."),
                    finish: Some(IrFinishReason::ToolCall),
                    ..IrAggregation::default()
                },
                "tool finish",
            ),
            (
                IrAggregation {
                    text: String::from("Text beside a tool call."),
                    finish: Some(IrFinishReason::Stop),
                    tool_calls: vec![IrAggregatedToolCall {
                        call_id: String::from("call"),
                        name: String::from("lookup"),
                        arguments: String::from("{}"),
                        thought_signature: None,
                    }],
                    ..IrAggregation::default()
                },
                "tool call",
            ),
        ] {
            let failure = match validate_summary(&aggregation) {
                Ok(_) => return Err(format!("the {reason} shape is unusable").into()),

                Err(failure) => failure,
            };

            let rendered = failure.to_string();

            assert!(
                rendered.starts_with("compaction_summary_unusable:"),
                "the {reason} shape renders the stable code, got {rendered}"
            );
        }

        Ok(())
    }
}
