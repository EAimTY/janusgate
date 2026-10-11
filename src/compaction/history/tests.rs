//! History-materialization tests: interaction-unit grouping, retention selection under the
//! budgets, mandatory-turn and modality retention, opaque-state failures, instruction identity
//! preservation, prefix-identity facts, native continuation, and the sealed checkpoint
//! round-trip through the authenticating boundary consumer.
//!
//! Sealing scenarios hold the shared test configuration's lock: the sealers read the live
//! configuration's keys, and every unsealing here runs through the real boundary consumer.

#![expect(
    clippy::missing_panics_doc,
    reason = "the tests' assertions panic; a # Panics section on every test would restate that"
)]

use super::{Budgets, CheckpointInstructions, RestoredBoundary, seal_gateway_checkpoint};
use crate::{
    compaction::error::CompactionError,
    compaction_model::{
        boundary::{ActiveBoundary, BoundaryPreparation},
        envelope,
        envelope::{
            EnvelopePayload, EnvelopeV1, GatewaySummaryPayload, NativeReplayWindow,
            NativeWindowPayload, PrefixIdentity, PreservedInstruction, ReplacementMode,
            RetainedUnit,
        },
        identity::{InstructionRole, ItemIdentity},
        mechanism::Mechanism,
        sealing,
    },
    format::{
        BoundaryPlacement, CompactionBoundary, NativeArtifact, ParsedCompaction,
        ParsedInstructionItem, UpstreamFormat,
    },
    ir::{IrImage, IrMessage, IrPart, IrRequest, IrRole, StateFamily, StateItem},
    provider::{
        DiscoveryState, Model, ModelRouting, Provider, StoredCredentials,
        definition::{DefinitionPayload, ProviderDefinition},
    },
    test_support,
};
use serde_json::Value;
use std::{error::Error as StdError, num::NonZero};

/// The default test budgets: a summary budget of 1000 and the retention target under test.
fn budgets(retention: u64) -> Budgets {
    Budgets {
        summary: 1_000,
        retention,
    }
}

/// One text message of the given role.
fn text(role: IrRole, text: &str) -> IrMessage {
    IrMessage {
        role,
        parts: vec![IrPart::Text {
            text: String::from(text),
        }],
    }
}

/// One user turn carrying an inline image beside no text.
fn image_turn() -> IrMessage {
    IrMessage {
        role: IrRole::User,
        parts: vec![IrPart::Image {
            image: IrImage::Inline {
                mime_type: String::from("image/png"),
                data_base64: String::from("aXY="),
            },
        }],
    }
}

/// One assistant turn invoking one tool, answered by one tool result.
fn tool_exchange(call_id: &str) -> Vec<IrMessage> {
    vec![
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::ToolCall {
                call_id: String::from(call_id),
                name: String::from("lookup"),
                arguments: String::from("{}"),
                thought_signature: None,
            }],
        },
        IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: String::from(call_id),
                content: String::from("result"),
                is_error: false,
            }],
        },
    ]
}

/// A three-unit history of single-message user turns, each a 4-byte text (two estimated tokens).
fn three_user_units() -> IrRequest {
    IrRequest {
        messages: vec![
            text(IrRole::User, "aaaa"),
            text(IrRole::User, "aaaa"),
            text(IrRole::User, "aaaa"),
        ],
        ..IrRequest::default()
    }
}

/// The owned parsed instruction items of the given `(role, text)` pairs.
fn instruction_items(items: &[(InstructionRole, &str)]) -> Vec<ParsedInstructionItem> {
    items
        .iter()
        .map(|(role, text)| ParsedInstructionItem {
            role: *role,
            text: String::from(*text),
        })
        .collect()
}

/// The first opaque native Messages state item at one message offset.
fn native_state(offset: usize) -> StateItem {
    StateItem {
        offset,
        item: serde_json::json!({"type": "compaction", "content": null}),
        is_gateway: false,
        is_compaction: true,
        family: StateFamily::Messages,
    }
}

/// Renders a planning failure's stable code prefix.
fn failure_code(failure: &CompactionError) -> String {
    failure
        .to_string()
        .split(':')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The failure of a restore or preparation that must not succeed — the match form the outcome
/// tuple's non-`Debug` continuity half requires.
fn restore_failure<'a>(
    outcome: Result<(Option<RestoredBoundary>, CheckpointInstructions<'a>), CompactionError>,
    context: &str,
) -> Result<CompactionError, Box<dyn StdError>> {
    match outcome {
        Ok(_) => Err(format!("{context}: the restore unexpectedly succeeded").into()),

        Err(failure) => Ok(failure),
    }
}

/// A history groups into indivisible units at user messages and tool-call follow-ups, and a
/// compact-only plan under a zero retention budget summarizes every unit: the older prefix is
/// the whole history and no unit is retained.
#[test]
fn compact_only_planning_summarizes_whole_units() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: [
            vec![text(IrRole::User, "u1"), text(IrRole::Assistant, "a1")],
            vec![text(IrRole::User, "u2")],
            tool_exchange("call-1"),
            vec![text(IrRole::User, "u3")],
        ]
        .concat(),
        ..IrRequest::default()
    };

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(0),
    )?;

    let summary_roles: Vec<_> = plan
        .summary_input_messages
        .iter()
        .map(|message| message.role)
        .collect();

    assert_eq!(
        summary_roles,
        [
            IrRole::User,
            IrRole::Assistant,
            IrRole::User,
            IrRole::Assistant,
            IrRole::Tool,
            IrRole::User
        ],
        "the older prefix is the whole history, tool exchanges included whole"
    );

    assert!(
        plan.retained_units.is_empty(),
        "a zero retention budget retains nothing compact-only"
    );

    Ok(())
}

/// Retention takes recent whole units backward under the soft budget and never splits a unit:
/// a four-token budget keeps the last two two-token units and summarizes only the first.
#[test]
fn retention_keeps_recent_whole_units_under_the_budget() -> Result<(), Box<dyn StdError>> {
    let ir = three_user_units();

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(4),
    )?;

    assert_eq!(
        plan.summary_input_messages,
        vec![text(IrRole::User, "aaaa")],
        "only the oldest unit is summarized"
    );

    assert_eq!(
        plan.retained_units.len(),
        2,
        "the recent two units stay whole"
    );

    Ok(())
}

/// Compact-and-continue retains the pending user turn's unit as mandatory state even when the
/// retention budget is exhausted, summarizing only the older units.
#[test]
fn compact_and_continue_retains_the_pending_turn() -> Result<(), Box<dyn StdError>> {
    let ir = three_user_units();

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        true,
        budgets(0),
    )?;

    assert_eq!(
        plan.summary_input_messages,
        vec![text(IrRole::User, "aaaa"), text(IrRole::User, "aaaa")],
        "the units before the pending turn are the summarized prefix"
    );

    assert_eq!(
        plan.retained_units,
        vec![vec![text(IrRole::User, "aaaa")]],
        "the pending user turn's unit is mandatory retained state"
    );

    Ok(())
}

/// Image-carrying units are retained from the earliest one onward — the estimate never invents
/// an image size — and an image in the first unit leaves no summarizable prefix, which is the
/// documented budget failure.
#[test]
fn image_units_are_never_truncated() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![
            text(IrRole::User, "aaaa"),
            image_turn(),
            text(IrRole::User, "aaaa"),
        ],
        ..IrRequest::default()
    };

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(0),
    )?;

    assert_eq!(
        plan.summary_input_messages,
        vec![text(IrRole::User, "aaaa")],
        "the text unit before the image summarizes; the image suffix stays"
    );

    assert_eq!(plan.retained_units.len(), 2);

    let leading_image = IrRequest {
        messages: vec![image_turn(), text(IrRole::User, "aaaa")],
        ..IrRequest::default()
    };

    let failure = match super::plan_gateway_checkpoint(
        &leading_image,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(0),
    ) {
        Ok(_) => return Err("a leading image unit leaves no older prefix".into()),

        Err(failure) => failure,
    };

    assert_eq!(failure_code(&failure), "compaction_budget_exceeded");

    Ok(())
}

/// An exhausted retention budget with no mandatory state retains everything, which is the
/// documented no-older-prefix budget failure rather than a no-op success.
#[test]
fn nothing_left_to_summarize_is_a_budget_failure() -> Result<(), Box<dyn StdError>> {
    let ir = three_user_units();

    let failure = match super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(1_000_000),
    ) {
        Ok(_) => return Err("a fully retained history has no summarizable prefix".into()),

        Err(failure) => failure,
    };

    assert_eq!(failure_code(&failure), "compaction_budget_exceeded");

    Ok(())
}

/// A history opening with an orphan tool result is structurally invalid: the unusable-history
/// failure rejects it without discarding content.
#[test]
fn an_orphan_tool_result_is_unusable_history() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![IrMessage {
            role: IrRole::Tool,
            parts: vec![IrPart::ToolOutput {
                call_id: String::from("call-1"),
                content: String::from("result"),
                is_error: false,
            }],
        }],
        ..IrRequest::default()
    };

    let failure = match super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(0),
    ) {
        Ok(_) => return Err("an orphan tool result cannot group into a unit".into()),

        Err(failure) => failure,
    };

    assert_eq!(failure_code(&failure), "compaction_unusable_history");

    Ok(())
}

/// Opaque native state inside the summarized prefix fails a gateway checkpoint as inaccessible
/// state — it cannot be converted into text — while the native checkpoint plan, which the native
/// producer will consume, accepts the same history.
#[test]
fn opaque_state_in_the_summarized_prefix_is_inaccessible_to_summaries()
-> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: three_user_units().messages,
        state_items: vec![native_state(0)],
        ..IrRequest::default()
    };

    let gateway_failure = match super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(4),
    ) {
        Ok(_) => {
            return Err("the summarized prefix carries state a summary cannot represent".into());
        }

        Err(failure) => failure,
    };

    assert_eq!(
        failure_code(&gateway_failure),
        "compaction_inaccessible_state"
    );

    let native_plan =
        super::plan_native_checkpoint(&ir, &CheckpointInstructions::Parsed(&[]), budgets(4))?;

    assert_eq!(native_plan.summary_input_messages.len(), 1);

    Ok(())
}

/// State items inside the retained region ride the plan at offsets recomputed against the
/// restored window layout — one summary message precedes the retained units — and gateway
/// carriers never ride at all.
#[test]
fn retained_state_offsets_recompute_against_the_restored_layout() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![
            text(IrRole::User, "aaaa"),
            text(IrRole::User, "aaaa"),
            text(IrRole::Assistant, "aaaa"),
        ],
        state_items: vec![
            StateItem {
                offset: 0,
                item: serde_json::json!({"gateway": true}),
                is_gateway: true,
                is_compaction: true,
                family: StateFamily::Responses,
            },
            native_state(1),
        ],
        ..IrRequest::default()
    };

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(4),
    )?;

    let [retained] = plan.retained_state_items.as_slice() else {
        return Err("exactly the retained region's state item rides the plan".into());
    };

    assert_eq!(
        retained,
        &StateItem {
            offset: 1,
            item: serde_json::json!({"type": "compaction", "content": null}),
            is_gateway: false,
            is_compaction: true,
            family: StateFamily::Messages,
        },
        "the retained item lands one message behind its original offset, carriers dropped"
    );

    Ok(())
}

/// A parsed instruction set preserves every item under its occurrence-indexed identity, and the
/// checkpoint's coverage identities are exactly those preserved identities.
#[test]
fn parsed_instructions_preserve_their_identities() -> Result<(), Box<dyn StdError>> {
    let ir = three_user_units();

    let instructions = instruction_items(&[
        (InstructionRole::System, "be precise"),
        (InstructionRole::Developer, "never guess"),
    ]);

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&instructions),
        false,
        budgets(4),
    )?;

    let expected = ItemIdentity::for_instruction_window([
        (InstructionRole::System, "be precise"),
        (InstructionRole::Developer, "never guess"),
    ]);

    assert_eq!(plan.preserved_instructions.len(), 2);

    for (instruction, identity) in plan.preserved_instructions.iter().zip(&expected) {
        assert!(instruction.identity.matches(identity));
    }

    assert_eq!(plan.overlap_identities.len(), 2);

    Ok(())
}

/// A restored checkpoint's instruction continuity rides its sealed identities unchanged into
/// the new plan, so a re-compaction's coverage stays matchable by later replays.
#[test]
fn restored_instructions_pass_through_with_their_identities() -> Result<(), Box<dyn StdError>> {
    let ir = three_user_units();

    let identities = ItemIdentity::for_instruction_window([(InstructionRole::System, "sealed")]);

    let [identity] = identities.as_slice() else {
        return Err("a one-item window yields exactly one identity".into());
    };

    let identity = identity.clone();

    let restored = vec![crate::compaction_model::envelope::PreservedInstruction {
        identity: identity.clone(),
        role: InstructionRole::System,
        text: String::from("sealed"),
    }];

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Restored(restored.clone()),
        false,
        budgets(4),
    )?;

    assert_eq!(plan.preserved_instructions, restored);

    assert_eq!(plan.overlap_identities, vec![identity]);

    Ok(())
}

/// The produced prefix identity digests the exact system instruction and the canonical tool
/// JSON, with absent facts staying absent.
#[test]
fn produced_prefix_identity_digests_the_binding_surface() -> Result<(), Box<dyn StdError>> {
    let identity = super::produced_prefix_identity(&IrRequest::default());

    assert_eq!(identity.instructions_digest, None);

    assert_eq!(identity.tools_digest, None);

    let ir = IrRequest {
        instructions: Some(String::from("be precise")),
        tools: vec![crate::ir::IrTool {
            name: String::from("lookup"),
            description: None,
            parameters: Some(serde_json::json!({})),
        }],
        ..IrRequest::default()
    };

    let identity = super::produced_prefix_identity(&ir);

    assert_eq!(
        identity.instructions_digest,
        Some(crate::compaction_model::identity::hex_sha256(b"be precise"))
    );

    assert_eq!(
        identity.tools_digest,
        Some(crate::compaction_model::identity::hex_sha256(
            &serde_json::to_vec(&ir.tools)?
        ))
    );

    Ok(())
}

/// A freshly produced native window restores for its immediate continuation: the produced
/// retained messages replace the request's history and the window's items ride as the
/// producer's own compaction state ahead of the payload's retained state.
#[test]
fn native_continuation_restores_the_produced_window() -> Result<(), Box<dyn StdError>> {
    let ir = IrRequest {
        messages: vec![text(IrRole::User, "u1"), text(IrRole::User, "u2")],
        ..IrRequest::default()
    };

    let retained = text(IrRole::User, "retained");

    let payload = crate::compaction_model::envelope::NativeWindowPayload {
        producer_instance_id: String::from("provider:1"),
        upstream_model_id: String::from("upstream-model"),
        mechanism: crate::compaction_model::mechanism::Mechanism::ResponsesStandaloneCompact,
        native_format: crate::format::UpstreamFormat::OpenAiResponse,
        required_prefix_identity: None,
        retained_messages: vec![retained.clone()],
        retained_state_items: vec![native_state(0)],
        window: crate::compaction_model::envelope::NativeReplayWindow::ResponsesWindow {
            items: vec![serde_json::json!({"type": "compaction", "encrypted_content": "enc"})],
        },
    };

    let continuation = super::native_continuation(&ir, &payload)?;

    assert_eq!(
        continuation.messages,
        vec![retained],
        "the window replaces the history"
    );

    let [window_item, retained_item] = continuation.state_items.as_slice() else {
        return Err(
            "the continuation carries the window's items beside the payload's retained state"
                .into(),
        );
    };

    assert!(
        window_item.is_compaction
            && window_item.offset == 0
            && window_item.family == StateFamily::Responses,
        "the window's own items ride first as compaction state"
    );

    assert_eq!(retained_item, &native_state(0));

    Ok(())
}

/// The sealed gateway checkpoint round-trips through the boundary consumer: the carrier unseals
/// to the sealed summary, retained units, and replacement mode, and the Messages visible-content
/// binding authenticates only its own content.
#[test]
fn sealed_checkpoints_round_trip_through_the_boundary() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();
    test_support::install_configuration();

    let ir = three_user_units();

    let instructions = instruction_items(&[(InstructionRole::System, "be precise")]);

    let plan = super::plan_gateway_checkpoint(
        &ir,
        &CheckpointInstructions::Parsed(&instructions),
        false,
        budgets(4),
    )?;

    let summary_text = String::from("The sealed summary.");

    let (envelope, carrier) = seal_gateway_checkpoint(
        &plan,
        summary_text.clone(),
        crate::compaction_model::envelope::ReplacementMode::CanonicalWindow,
        Some(&summary_text),
    )?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Gateway {
            offset: 0,
            carrier: carrier.clone(),
            visible_content: Some(summary_text.clone()),
        }],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let Some(ActiveBoundary::Gateway {
        envelope: unsealed, ..
    }) = preparation.active()
    else {
        return Err("the sealed carrier authenticates as the active gateway boundary".into());
    };

    assert_eq!(unsealed.artifact_id, envelope.artifact_id);

    let EnvelopePayload::GatewaySummary(payload) = &unsealed.payload else {
        return Err("the sealed checkpoint carries a gateway summary payload".into());
    };

    assert_eq!(payload.summary_text, summary_text);

    assert_eq!(payload.retained_units.len(), plan.retained_units.len());

    assert_eq!(payload.overlap_identities, plan.overlap_identities);

    let tampered = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Gateway {
            offset: 0,
            carrier,
            visible_content: Some(String::from("tampered")),
        }],
        instruction_items: Vec::new(),
    };

    assert!(
        BoundaryPreparation::authenticate(&tampered).is_err(),
        "a carrier cannot be consumed with content other than what it authenticated"
    );

    Ok(())
}

/// Plans over equal histories under equal budgets and instructions are equal, and the plan
/// distinguishes a compact-only from a compact-and-continue shape.
#[test]
fn plans_over_equal_histories_are_equal() -> Result<(), Box<dyn StdError>> {
    let first = super::plan_gateway_checkpoint(
        &three_user_units(),
        &CheckpointInstructions::Parsed(&[]),
        true,
        budgets(0),
    )?;

    let second = super::plan_gateway_checkpoint(
        &three_user_units(),
        &CheckpointInstructions::Parsed(&[]),
        true,
        budgets(0),
    )?;

    assert_eq!(first, second);

    let differing = super::plan_gateway_checkpoint(
        &three_user_units(),
        &CheckpointInstructions::Parsed(&[]),
        false,
        budgets(0),
    )?;

    assert_ne!(first, differing);

    Ok(())
}

/// The gateway boundary of one carrier at one message offset, unbound to visible content.
fn gateway_boundary(offset: usize, carrier: String) -> CompactionBoundary {
    CompactionBoundary::Gateway {
        offset,
        carrier,
        visible_content: None,
    }
}

/// The gateway summary envelope carrying the given text, retained units, preserved
/// instructions, and coverage identities.
fn summary_envelope(
    summary_text: &str,
    retained_units: Vec<Vec<IrMessage>>,
    preserved_instructions: Vec<PreservedInstruction>,
    overlap_identities: Vec<ItemIdentity>,
) -> EnvelopeV1 {
    EnvelopeV1::new(
        String::from("cmp_restore_fixture"),
        ReplacementMode::HistoricalCheckpoint,
        EnvelopePayload::GatewaySummary(GatewaySummaryPayload {
            summary_text: String::from(summary_text),
            preserved_instructions,
            retained_units: retained_units
                .into_iter()
                .map(|messages| RetainedUnit { messages })
                .collect(),
            retained_state_items: Vec::new(),
            overlap_identities,
        }),
        None,
    )
}

/// The wrapped native window envelope with the given provenance, retained region, and replay
/// window.
fn native_window_envelope(
    producer_instance: &str,
    native_format: UpstreamFormat,
    required_prefix: Option<PrefixIdentity>,
    retained_messages: Vec<IrMessage>,
    retained_state_items: Vec<StateItem>,
    window: NativeReplayWindow,
) -> EnvelopeV1 {
    EnvelopeV1::new(
        String::from("cmp_native_fixture"),
        ReplacementMode::CanonicalWindow,
        EnvelopePayload::NativeWindow(NativeWindowPayload {
            producer_instance_id: String::from(producer_instance),
            upstream_model_id: String::from("upstream-model"),
            mechanism: Mechanism::ResponsesStandaloneCompact,
            native_format,
            required_prefix_identity: required_prefix,
            retained_messages,
            retained_state_items,
            window,
        }),
        None,
    )
}

/// Seals the envelope into its carrier under the installed shared test configuration.
fn sealed_carrier(envelope: &EnvelopeV1) -> Result<String, Box<dyn StdError>> {
    Ok(sealing::seal_with_live_config(envelope, None)?)
}

/// The routing of one provider/model speaking the given format with the given compaction section
/// JSON — `Value::Null` declares no section. No database, upstream, or context: the routing only
/// feeds `prepare_for_target`'s compatibility and export decisions.
fn routing_with(
    format: UpstreamFormat,
    compaction: Value,
) -> Result<ModelRouting, Box<dyn StdError>> {
    let definition = DefinitionPayload {
        format,
        inference: serde_json::from_value(serde_json::json!({
            "url": [{"type": "literal", "value": "http://unused.test/v1/messages"}],
            "upstream_streaming": "match_client"
        }))?,
        credential: serde_json::from_value(serde_json::json!({"type": "api_key"}))?,
        provisioning: None,
        discovery: None,
        quota: None,
        compaction: serde_json::from_value(compaction)?,
    };

    Ok(ModelRouting {
        model_route_id: 1,
        public_id: String::from("public-model"),
        provider: Provider {
            id: 7,
            name: String::from("fixture"),
            model_prefix: String::new(),
            base_url: None,
            credentials: StoredCredentials::ApiKey {
                api_key: String::from("test-key"),
            },
            is_enabled: true,
            quota_generation: 0,
            quota_revision: 0,
            definition: ProviderDefinition {
                name: String::from("fixture-definition"),
                version: NonZero::new(1).ok_or("the fixture version is nonzero")?,
                payload: definition,
                created_at: 1_000,
            },
        },
        model: Model {
            id: 9,
            upstream_id: String::from("upstream-model"),
            is_enabled: true,
            discovery_state: DiscoveryState::Discovered,
            thinking_levels: None,
            max_input_tokens: None,
            max_output_tokens: None,
            thinking_levels_override: None,
            max_input_tokens_override: None,
            max_output_tokens_override: None,
            compaction_support: None,
            compaction_support_override: None,
        },
    })
}

/// Restoring a gateway summary supersedes the covered conversational prefix wholesale, restores
/// the summary and retained units ahead of the fresh suffix, removes the covered instruction by
/// its proven identity while a newly injected instruction survives, and rebuilds the instruction
/// context preserved-first — the replay continuity the returned items carry.
#[test]
fn gateway_summary_restore_supersedes_the_prefix_and_keeps_new_instructions()
-> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let sealed_window =
        ItemIdentity::for_instruction_window([(InstructionRole::System, "old rule")]);

    let [sealed_system] = sealed_window.as_slice() else {
        return Err("a one-item window yields exactly one identity".into());
    };

    let sealed_system = sealed_system.clone();

    let carrier = sealed_carrier(&summary_envelope(
        "The covered work.",
        vec![vec![text(IrRole::Assistant, "kept answer")]],
        vec![PreservedInstruction {
            identity: sealed_system.clone(),
            role: InstructionRole::System,
            text: String::from("old rule"),
        }],
        vec![sealed_system.clone()],
    ))?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(2, carrier)],
        instruction_items: instruction_items(&[
            (InstructionRole::System, "old rule"),
            (InstructionRole::Developer, "new rule"),
        ]),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        instructions: Some(String::from("stale")),
        messages: vec![
            text(IrRole::User, "covered one"),
            text(IrRole::Assistant, "covered two"),
            text(IrRole::User, "fresh turn"),
        ],
        // The parser records the carrier's own state item beside every gateway boundary.
        state_items: vec![StateItem {
            offset: 2,
            item: serde_json::json!({"type": "compaction"}),
            is_gateway: true,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    let (restored, instructions) = super::restore(
        &mut ir,
        &parsed,
        &preparation,
        UpstreamFormat::OpenAiResponse,
    )?;

    assert!(
        matches!(restored, Some(RestoredBoundary::Gateway { .. })),
        "the decoded summary checkpoint is the restored boundary"
    );

    assert_eq!(
        ir.messages,
        vec![
            envelope::summary_message("The covered work."),
            text(IrRole::Assistant, "kept answer"),
            text(IrRole::User, "fresh turn"),
        ],
        "the covered prefix is superseded; summary, retained unit, and fresh suffix restore"
    );

    assert_eq!(
        ir.instructions.as_deref(),
        Some("old rule\n\nnew rule"),
        "the covered instruction is removed by its proven identity and the new one survives"
    );

    let CheckpointInstructions::Restored(restored_items) = instructions else {
        return Err("a restored summary hands its instruction continuity forward".into());
    };

    let [sealed_item, fresh_item] = restored_items.as_slice() else {
        return Err("the continuity is the sealed item beside the surviving current one".into());
    };

    assert_eq!(
        sealed_item.identity, sealed_system,
        "the sealed identity rides unchanged"
    );

    let current_window = ItemIdentity::for_instruction_window([
        (InstructionRole::System, "old rule"),
        (InstructionRole::Developer, "new rule"),
    ]);

    let [_, fresh_identity] = current_window.as_slice() else {
        return Err("the two-item window yields exactly two identities".into());
    };

    assert!(
        fresh_item.identity.matches(fresh_identity),
        "the surviving instruction carries its fresh replay identity"
    );

    Ok(())
}

/// Same-offset state splits at the boundary's own slot: state the boundary's encounter order
/// precedes is covered with the prefix, and the state that follows rides the fresh suffix at its
/// offset recomputed behind the restored window.
#[test]
fn same_offset_state_splits_at_the_boundary_slot() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let carrier = sealed_carrier(&summary_envelope(
        "The covered work.",
        Vec::new(),
        Vec::new(),
        Vec::new(),
    ))?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, carrier)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
        state_items: vec![
            StateItem {
                offset: 1,
                item: serde_json::json!({"opaque": "covered"}),
                is_gateway: false,
                is_compaction: false,
                family: StateFamily::Messages,
            },
            StateItem {
                offset: 1,
                item: serde_json::json!({"type": "compaction"}),
                is_gateway: true,
                is_compaction: true,
                family: StateFamily::Responses,
            },
            native_state(1),
            StateItem {
                offset: 2,
                item: serde_json::json!({"opaque": "suffix"}),
                is_gateway: false,
                is_compaction: false,
                family: StateFamily::Messages,
            },
        ],
        ..IrRequest::default()
    };

    super::restore(
        &mut ir,
        &parsed,
        &preparation,
        UpstreamFormat::OpenAiResponse,
    )?;

    assert_eq!(
        ir.messages,
        vec![
            envelope::summary_message("The covered work."),
            text(IrRole::User, "fresh"),
        ],
        "the summary restores ahead of the fresh suffix"
    );

    assert_eq!(
        ir.state_items,
        vec![
            native_state(1),
            StateItem {
                offset: 2,
                item: serde_json::json!({"opaque": "suffix"}),
                is_gateway: false,
                is_compaction: false,
                family: StateFamily::Messages,
            },
        ],
        "the covered same-offset state is superseded; the fresh state recomputes behind the summary"
    );

    assert_eq!(
        ir.instructions, None,
        "an instruction-free checkpoint restores no instruction context"
    );

    Ok(())
}

/// A checkpoint whose boundary sits beyond the request's history is structurally impossible: the
/// invalid-gateway-state failure rejects it instead of fabricating an empty covered prefix.
#[test]
fn a_boundary_beyond_the_history_is_invalid_gateway_state() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let carrier = sealed_carrier(&summary_envelope("S", Vec::new(), Vec::new(), Vec::new()))?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(5, carrier)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "only")],
        // The parser records the carrier's own state item beside every gateway boundary.
        state_items: vec![StateItem {
            offset: 5,
            item: serde_json::json!({"type": "compaction"}),
            is_gateway: true,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    let failure = restore_failure(
        super::restore(
            &mut ir,
            &parsed,
            &preparation,
            UpstreamFormat::OpenAiResponse,
        ),
        "a boundary beyond the history is structurally impossible",
    )?;

    assert_eq!(failure_code(&failure), "compaction_invalid_gateway_state");

    Ok(())
}

/// A wrapped native window replays only against its own wire format and its producer's recorded
/// system prefix: another format and a changed instruction context are inaccessible state, while
/// the matching request restores the exact window, the payload's retained region, and the fresh
/// suffix at its recomputed offsets.
#[test]
fn wrapped_windows_replay_only_in_their_format_and_prefix() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let foreign_format = native_window_envelope(
        "provider:7",
        UpstreamFormat::AnthropicMessages,
        None,
        Vec::new(),
        Vec::new(),
        NativeReplayWindow::ResponsesWindow {
            items: vec![serde_json::json!({"type": "compaction"})],
        },
    );

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, sealed_carrier(&foreign_format)?)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let carrier_state = StateItem {
        offset: 1,
        item: serde_json::json!({"type": "compaction"}),
        is_gateway: true,
        is_compaction: true,
        family: StateFamily::Responses,
    };

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
        state_items: vec![carrier_state.clone()],
        ..IrRequest::default()
    };

    let failure = restore_failure(
        super::restore(
            &mut ir,
            &parsed,
            &preparation,
            UpstreamFormat::OpenAiResponse,
        ),
        "the window speaks another wire format than the target",
    )?;

    assert_eq!(failure_code(&failure), "compaction_inaccessible_state");

    let requirement = super::produced_prefix_identity(&IrRequest {
        instructions: Some(String::from("the sealed prompt")),
        ..IrRequest::default()
    });

    let bound = native_window_envelope(
        "provider:7",
        UpstreamFormat::OpenAiResponse,
        Some(requirement),
        vec![text(IrRole::Assistant, "kept")],
        vec![native_state(0)],
        NativeReplayWindow::ResponsesWindow {
            items: vec![serde_json::json!({
                "type": "compaction",
                "encrypted_content": "native-ciphertext"
            })],
        },
    );

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, sealed_carrier(&bound)?)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    ir.instructions = Some(String::from("a changed prompt"));

    let failure = restore_failure(
        super::restore(
            &mut ir,
            &parsed,
            &preparation,
            UpstreamFormat::OpenAiResponse,
        ),
        "the request's context no longer matches the recorded prefix",
    )?;

    assert_eq!(failure_code(&failure), "compaction_inaccessible_state");

    ir.instructions = Some(String::from("the sealed prompt"));

    ir.state_items = vec![carrier_state, native_state(2)];

    let (restored, instructions) = super::restore(
        &mut ir,
        &parsed,
        &preparation,
        UpstreamFormat::OpenAiResponse,
    )?;

    assert!(matches!(restored, Some(RestoredBoundary::Gateway { .. })));

    assert!(matches!(instructions, CheckpointInstructions::Parsed(_)));

    assert_eq!(
        ir.messages,
        vec![text(IrRole::Assistant, "kept"), text(IrRole::User, "fresh")],
        "the retained messages replace the covered prefix and the fresh suffix follows"
    );

    assert_eq!(
        ir.state_items,
        vec![
            StateItem {
                offset: 0,
                item: serde_json::json!({
                    "type": "compaction",
                    "encrypted_content": "native-ciphertext"
                }),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Responses,
            },
            native_state(0),
            native_state(2),
        ],
        "the window rides as state; the retained region and the recomputed fresh suffix follow"
    );

    Ok(())
}

/// A readable native block exports as an ordinary summary only when the definition enables the
/// downgrade: enabled, the readable content becomes the summary message ahead of the fresh
/// suffix and its state; disabled, the same foreign window stays inaccessible state.
#[test]
fn readable_native_state_exports_only_when_enabled() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let routing = routing_with(
        UpstreamFormat::OpenAiResponse,
        serde_json::json!({"can_export_native_summary": true}),
    )?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![CompactionBoundary::Native {
            offset: 1,
            artifact: NativeArtifact::MessagesCompactionBlock {
                is_null_no_op: false,
                placement: BoundaryPlacement::Historical,
            },
        }],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
        state_items: vec![
            StateItem {
                offset: 1,
                item: serde_json::json!({"type": "compaction", "content": "Readable summary."}),
                is_gateway: false,
                is_compaction: true,
                family: StateFamily::Messages,
            },
            StateItem {
                offset: 2,
                item: serde_json::json!({"opaque": "suffix"}),
                is_gateway: false,
                is_compaction: false,
                family: StateFamily::Messages,
            },
        ],
        ..IrRequest::default()
    };

    let (restored, instructions) =
        super::prepare_for_target(&mut ir, &parsed, &routing, &preparation)?;

    assert!(
        restored.is_none(),
        "an exported block leaves no native boundary to restore"
    );

    assert!(matches!(instructions, CheckpointInstructions::Parsed(_)));

    assert_eq!(
        ir.messages,
        vec![
            envelope::summary_message("Readable summary."),
            text(IrRole::User, "fresh"),
        ],
        "the readable block becomes the summary message ahead of the fresh suffix"
    );

    let [suffix] = ir.state_items.as_slice() else {
        return Err("exactly the fresh suffix state survives the export".into());
    };

    assert_eq!(
        suffix.offset, 2,
        "the fresh state rides at its recomputed offset"
    );

    let foreign = native_window_envelope(
        "provider:999",
        UpstreamFormat::AnthropicMessages,
        None,
        Vec::new(),
        Vec::new(),
        NativeReplayWindow::AnthropicOnDemandBlock {
            block: serde_json::json!({"type": "compaction", "content": "Readable."}),
        },
    );

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, sealed_carrier(&foreign)?)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
        state_items: vec![StateItem {
            offset: 1,
            item: serde_json::json!({"type": "compaction"}),
            is_gateway: true,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    let unreadable = routing_with(UpstreamFormat::OpenAiResponse, Value::Null)?;

    let failure = restore_failure(
        super::prepare_for_target(&mut ir, &parsed, &unreadable, &preparation),
        "without the documented opt-in the foreign window stays inaccessible",
    )?;

    assert_eq!(failure_code(&failure), "compaction_inaccessible_state");

    Ok(())
}

/// The export downgrade never carries hidden state: a foreign window whose block is ciphertext
/// stays inaccessible even when export is enabled, the payload's retained messages ride the
/// exported window, and a fresh suffix carrying signed reasoning refuses the export.
#[test]
fn export_refuses_hidden_state_and_signed_reasoning() -> Result<(), Box<dyn StdError>> {
    let _lock = test_support::configuration_lock();

    test_support::install_configuration();

    let routing = routing_with(
        UpstreamFormat::OpenAiResponse,
        serde_json::json!({"can_export_native_summary": true}),
    )?;

    let carrier_of = |block: Value, retained: Vec<IrMessage>| {
        sealed_carrier(&native_window_envelope(
            "provider:999",
            UpstreamFormat::AnthropicMessages,
            None,
            retained,
            Vec::new(),
            NativeReplayWindow::AnthropicOnDemandBlock { block },
        ))
    };

    let ciphertext = carrier_of(
        serde_json::json!({"type": "compaction", "content": "Readable.", "encrypted_content": "cipher"}),
        vec![text(IrRole::Assistant, "kept")],
    )?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, ciphertext)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    let mut ir = IrRequest {
        messages: vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")],
        state_items: vec![StateItem {
            offset: 1,
            item: serde_json::json!({"type": "compaction"}),
            is_gateway: true,
            is_compaction: true,
            family: StateFamily::Responses,
        }],
        ..IrRequest::default()
    };

    let failure = restore_failure(
        super::prepare_for_target(&mut ir, &parsed, &routing, &preparation),
        "ciphertext cannot be exported as text",
    )?;

    assert_eq!(failure_code(&failure), "compaction_inaccessible_state");

    let readable = carrier_of(
        serde_json::json!({"type": "compaction", "content": "Readable.", "signature": "sig"}),
        vec![text(IrRole::Assistant, "kept")],
    )?;

    let parsed = ParsedCompaction {
        control: None,
        boundaries: vec![gateway_boundary(1, readable)],
        instruction_items: Vec::new(),
    };

    let preparation = BoundaryPreparation::authenticate(&parsed)?;

    ir.messages = vec![
        text(IrRole::User, "covered"),
        IrMessage {
            role: IrRole::Assistant,
            parts: vec![IrPart::Reasoning {
                text: String::from("signed thinking"),
                signature: Some(String::from("sig")),
                opaque: serde_json::Value::Null,
            }],
        },
    ];

    let failure = restore_failure(
        super::prepare_for_target(&mut ir, &parsed, &routing, &preparation),
        "the export cannot preserve the signed reasoning it would discard",
    )?;

    assert_eq!(failure_code(&failure), "compaction_inaccessible_state");

    ir.messages = vec![text(IrRole::User, "covered"), text(IrRole::User, "fresh")];

    let (restored, _) = super::prepare_for_target(&mut ir, &parsed, &routing, &preparation)?;

    assert!(restored.is_none());

    assert_eq!(
        ir.messages,
        vec![
            envelope::summary_message("Readable."),
            text(IrRole::Assistant, "kept"),
            text(IrRole::User, "fresh"),
        ],
        "the exported window is the summary, the payload's retained messages, and the fresh suffix"
    );

    Ok(())
}
