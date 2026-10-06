import type { JSX } from "@solidjs/web";
import { createMemo, createSignal } from "solid-js";
import { readProviderCatalog } from "/src/features/inventory/read";
import { isSameModelRow, isSameProviderRow } from "/src/features/inventory/row-equality";
import {
    CompactionSupportOverrideFields,
    createCapabilityOverrideDraft,
    isSameCompactionOverride,
    isSameLevelSet,
    ThinkingLevelsOverrideFields,
    TokenCapOverrideFields,
} from "/src/features/models/model-form";
import { VanishedModelNotice } from "/src/features/models/vanished-model-notice";
import { translation } from "/src/i18n";
import { deleteModelMessage, editIdentifierMessage } from "/src/i18n/rich";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { hrefs, navigate } from "/src/router";
import { createDeleteAction, createEditForm } from "/src/screen-state/edit-form";
import { createScreenRead, readOne, type ScreenRead } from "/src/screen-state/read";
import { createEditRow, createPinnedRow } from "/src/screen-state/row-state";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

/** Edits physical inventory settings without changing public route identities. */
export function ModelEditScreen(props: {
    readonly api: ManagementApi;
    readonly modelId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);

    const latestModel = createMemo(() => models().find((row) => row.id === props.modelId) ?? null);

    // The held row with its save-settling latch: an open form with unsaved
    // edits holds its row against refreshes, so an operator's input can never
    // silently vanish, and a succeeded save holds it until its own reload
    // lands (README §6.3).
    const edit = createEditRow(latestModel, isSameModelRow);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const catalog = await readOne(signal, readProviderCatalog(props.api, null));

            setProviders(catalog.providers);
            setModels(catalog.models);
            edit.reportReloaded();
        },
    });

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the hold stands — and the hold is stated.
    const { row: model, isRowHeld } = edit;

    // The owning provider, kept identity-stable for the same reason: the
    // listing re-parses on every read, so an unpinned lookup would hand the
    // form a fresh object each refresh and remount it over the operator's
    // input even when nothing changed.
    const latestOwningProvider = createMemo(() => {
        const current = model();

        return current === null
            ? null
            : (providers().find((row) => row.id === current.provider_id) ?? null);
    });
    const provider = createPinnedRow(latestOwningProvider, isSameProviderRow);

    return (
        <ScreenFrame
            read={read}
            row={model}
            firstReadHeader={
                <PageHeader
                    title={translation()(($) => $.screens.modelEditScreen.editModel)}
                    backHref={hrefs.modelDetail(props.modelId)}
                    backLabel={translation()(($) => $.additional.modelEditScreen.model)}
                />
            }
            notFound={
                <RowNotFound
                    kind="model"
                    headerTitle={translation()(($) => $.additional.modelEditScreen.editModel)}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={editIdentifierMessage(row.upstream_id)}
                        backHref={hrefs.modelDetail(row.id)}
                        backLabel={translation()(($) => $.additional.modelEditScreen.model)}
                    />

                    {/* The suspension the detail page states, restated here
                        through the shared notice: this page is reachable by
                        deep link, and it is where an "enable it again" edit
                        lands — a save of the enabled switch must not read as
                        service restored while the upstream still withholds
                        the row. */}
                    <VanishedModelNotice
                        isVanished={row.discovery_state === "vanished"}
                        providerId={row.provider_id}
                        upstreamId={row.upstream_id}
                    />

                    <HoldNotice
                        isHeld={isRowHeld()}
                        subject="model"
                    />

                    <ModelSettingsForm
                        api={props.api}
                        provider={provider()}
                        initialModel={row}
                        read={read}
                        onDirtyChange={edit.reportFormDirty}
                        onSaved={edit.reportSaved}
                    />
                    <DeleteCard
                        api={props.api}
                        model={row}
                        read={read}
                    />
                </>
            )}
        </ScreenFrame>
    );
}

function ModelSettingsForm(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly initialModel: ModelResponse;
    readonly read: ScreenRead;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    // A save re-baselines the capability settings onto the returned row.
    // The initial fields seed from the props directly, so
    // the setup reads stay out of the `current` signal's tracking contract.
    const [current, setCurrent] = createSignal(props.initialModel);
    const [isEnabled, setIsEnabled] = createSignal(props.initialModel.is_enabled);
    const draft = createCapabilityOverrideDraft(props.initialModel);

    const isDirty = (): boolean =>
        isEnabled() !== current().is_enabled ||
        !isSameLevelSet(draft.normalizedLevels(), current().thinking_levels_override) ||
        draft.normalizedMaxInput() !== current().max_input_tokens_override ||
        draft.normalizedMaxOutput() !== current().max_output_tokens_override ||
        !isSameCompactionOverride(
            draft.normalizedCompaction(),
            current().compaction_support_override,
        );

    const canSave = (): boolean =>
        isDirty() && draft.maxInputError() === null && draft.maxOutputError() === null;

    const revert = (): void => {
        setIsEnabled(current().is_enabled);
        draft.apply(current());
        // Reverting resolves a held row onto the gateway's current version.
        props.read.reload();
    };

    const form = createEditForm({
        read: props.read,
        action: "update-model",
        isDirty,
        canSave,
        perform: () =>
            props.api.updateModel(
                toUpdateModelRequest(current(), {
                    is_enabled: isEnabled(),
                    thinking_levels_override: draft.normalizedLevels(),
                    max_input_tokens_override: draft.normalizedMaxInput(),
                    max_output_tokens_override: draft.normalizedMaxOutput(),
                    compaction_support_override: draft.normalizedCompaction(),
                }),
            ),
        onSucceeded: (envelope) => {
            // The returned inventory row is the form's new baseline.
            setCurrent(envelope.model);
            setIsEnabled(envelope.model.is_enabled);
            draft.apply(envelope.model);

            return {
                tone: "ok",
                get title() {
                    return translation()(($) => $.notices.modelSaved);
                },
                detail: null,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader title={translation()(($) => $.screens.modelEditScreen.modelSettings)} />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-5"
                onSubmit={form.submit}
            >
                <Switch
                    label={translation()(($) => $.screens.modelEditScreen.enabled)}
                    isOn={isEnabled()}
                    isDisabled={form.isSaving()}
                    onToggle={setIsEnabled}
                />

                <ThinkingLevelsOverrideFields
                    hasOverride={draft.hasLevelsOverride()}
                    onHasOverrideToggle={draft.setHasLevelsOverride}
                    selected={draft.selectedLevels()}
                    onSelectedChange={draft.setSelectedLevels}
                    discovered={current().thinking_levels}
                    isDisabled={form.isSaving()}
                />

                <TokenCapOverrideFields
                    switchLabel={translation()(($) => $.remainingText.overrideMaxInputTokens)}
                    fieldLabel={translation()(($) => $.remainingText.maxInputTokensOverride)}
                    hasOverride={draft.hasMaxInputOverride()}
                    onHasOverrideToggle={draft.setHasMaxInputOverride}
                    value={draft.maxInputText()}
                    onValueInput={draft.setMaxInputText}
                    discovered={current().max_input_tokens}
                    valueError={draft.maxInputError()}
                    isDisabled={form.isSaving()}
                />

                <TokenCapOverrideFields
                    switchLabel={translation()(($) => $.remainingText.overrideMaxOutputTokens)}
                    fieldLabel={translation()(($) => $.remainingText.maxOutputTokensOverride)}
                    hasOverride={draft.hasMaxOutputOverride()}
                    onHasOverrideToggle={draft.setHasMaxOutputOverride}
                    value={draft.maxOutputText()}
                    onValueInput={draft.setMaxOutputText}
                    discovered={current().max_output_tokens}
                    valueError={draft.maxOutputError()}
                    isDisabled={form.isSaving()}
                />

                <CompactionSupportOverrideFields
                    hasOverride={draft.hasCompactionOverride()}
                    onHasOverrideToggle={draft.setHasCompactionOverride}
                    selected={draft.selectedCompaction()}
                    onSelectedChange={draft.setSelectedCompaction}
                    discovered={current().compaction_support}
                    isDisabled={form.isSaving()}
                />

                <DirtyFormFooter
                    canSave={canSave()}
                    isSaving={form.isSaving()}
                    isDirty={isDirty()}
                    onRevert={revert}
                />
            </form>
        </Card>
    );
}

/** Deletion, administrator-only: discovery never deletes a model. */
function DeleteCard(props: {
    readonly api: ManagementApi;
    readonly model: ModelResponse;
    readonly read: ScreenRead;
}): JSX.Element {
    const confirmDelete = createDeleteAction({
        read: props.read,
        action: "delete-model",
        perform: () =>
            props.api.deleteModel({
                model_id: props.model.id,
            }),
        onDeleted: () => navigate(hrefs.models(props.model.provider_id)),
    });

    return (
        <ConfirmDeleteCard
            title={translation()(($) => $.screens.modelEditScreen.deleteThisModel)}
            description={translation()(
                ($) =>
                    $.screens.modelEditScreen.deletingRemovesThisPhysicalModelAndItsDirectMessage,
            )}
            question={deleteModelMessage(props.model.upstream_id)}
            onConfirm={confirmDelete}
        />
    );
}
