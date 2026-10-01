import type { JSX } from "@solidjs/web";
import { createMemo, createSignal } from "solid-js";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import {
    createEditRow,
    createPinnedRow,
    createScreenRead,
    isSameModelRow,
    isSameProviderRow,
    readOne,
    readProviderCatalog,
    type ScreenRead,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { VanishedModelNotice } from "/src/screens/vanished-model-notice";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import {
    composedPublicId,
    isSameLevelSet,
    ModelIdOverrideFields,
    normalizedPublicIdOverride,
    normalizedThinkingLevelsOverride,
    normalizedTokenCapOverride,
    ThinkingLevelsOverrideFields,
    TokenCapOverrideFields,
    tokenCapOverrideError,
} from "/src/ui/model-form";
import { createDeleteAction, createEditForm } from "./edit-form";
import { RowNotFound } from "./row-not-found";

/**
 * One model's edit page: its settings and its deletion. The form opens
 * prefilled, tracks dirt, and saves the complete settings back — saving
 * installs the model the gateway returns, because the public ID is
 * recomposed inside that write. Reading the model — its facts and its usage
 * — lives on the detail page. A vanished row's suspension is stated under
 * the header with the detail page's two recovery paths, and the save
 * notice's routing line speaks only where service is true — silent for a
 * vanished row, and naming the provider's re-enable where the owning
 * provider is disabled.
 */
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
                    title="Edit model"
                    backHref={hrefs.modelDetail(props.modelId)}
                    backLabel="Model"
                />
            }
            notFound={
                <RowNotFound
                    kind="model"
                    headerTitle="Edit model"
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={`Edit ${row.public_id}`}
                        backHref={hrefs.modelDetail(row.id)}
                        backLabel="Model"
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
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the write's answer, because the public ID is recomposed
    // inside that write. The initial fields seed from the props directly, so
    // the setup reads stay out of the `current` signal's tracking contract.
    const [current, setCurrent] = createSignal(props.initialModel);
    const [isEnabled, setIsEnabled] = createSignal(props.initialModel.is_enabled);
    const [hasIdOverride, setHasIdOverride] = createSignal(props.initialModel.id_override !== null);
    const [idOverride, setIdOverride] = createSignal(props.initialModel.id_override ?? "");
    const [hasLevelsOverride, setHasLevelsOverride] = createSignal(
        props.initialModel.thinking_levels_override !== null,
    );
    const [selectedLevels, setSelectedLevels] = createSignal<ReadonlyArray<string>>(
        props.initialModel.thinking_levels_override ?? [],
    );
    const [hasMaxInputOverride, setHasMaxInputOverride] = createSignal(
        props.initialModel.max_input_tokens_override !== null,
    );
    const [maxInputText, setMaxInputText] = createSignal(
        props.initialModel.max_input_tokens_override?.toString() ?? "",
    );
    const [hasMaxOutputOverride, setHasMaxOutputOverride] = createSignal(
        props.initialModel.max_output_tokens_override !== null,
    );
    const [maxOutputText, setMaxOutputText] = createSignal(
        props.initialModel.max_output_tokens_override?.toString() ?? "",
    );

    // Both derivations run through the shared toggle-aware helpers, so the
    // off-hint always names the ID the write produces — even with stale text
    // left in the override field after the switch goes off.
    const normalizedOverride = (): string | null =>
        normalizedPublicIdOverride(hasIdOverride(), idOverride());

    const composedId = (): string =>
        composedPublicId(
            props.provider?.model_prefix ?? "",
            current().upstream_id,
            normalizedOverride(),
        );

    /** Inline composition error for the override, shown where it was typed. */
    const overrideError = (): string | null =>
        routingTokenError(normalizedOverride(), "Public ID override");

    /** Inline errors for the token-cap overrides, shown where each was typed. */
    const maxInputError = (): string | null =>
        tokenCapOverrideError(hasMaxInputOverride(), maxInputText(), "Max input tokens override");

    const maxOutputError = (): string | null =>
        tokenCapOverrideError(
            hasMaxOutputOverride(),
            maxOutputText(),
            "Max output tokens override",
        );

    const normalizedLevels = (): Array<string> | null =>
        normalizedThinkingLevelsOverride(hasLevelsOverride(), selectedLevels());

    const normalizedMaxInput = (): number | null =>
        normalizedTokenCapOverride(hasMaxInputOverride(), maxInputText());

    const normalizedMaxOutput = (): number | null =>
        normalizedTokenCapOverride(hasMaxOutputOverride(), maxOutputText());

    const isDirty = (): boolean =>
        isEnabled() !== current().is_enabled ||
        normalizedOverride() !== current().id_override ||
        !isSameLevelSet(normalizedLevels(), current().thinking_levels_override) ||
        normalizedMaxInput() !== current().max_input_tokens_override ||
        normalizedMaxOutput() !== current().max_output_tokens_override;

    const canSave = (): boolean =>
        isDirty() &&
        overrideError() === null &&
        maxInputError() === null &&
        maxOutputError() === null;

    const revert = (): void => {
        setIsEnabled(current().is_enabled);
        setHasIdOverride(current().id_override !== null);
        setIdOverride(current().id_override ?? "");
        setHasLevelsOverride(current().thinking_levels_override !== null);
        setSelectedLevels(current().thinking_levels_override ?? []);
        setHasMaxInputOverride(current().max_input_tokens_override !== null);
        setMaxInputText(current().max_input_tokens_override?.toString() ?? "");
        setHasMaxOutputOverride(current().max_output_tokens_override !== null);
        setMaxOutputText(current().max_output_tokens_override?.toString() ?? "");
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
                    id_override: normalizedOverride(),
                    is_enabled: isEnabled(),
                    thinking_levels_override: normalizedLevels(),
                    max_input_tokens_override: normalizedMaxInput(),
                    max_output_tokens_override: normalizedMaxOutput(),
                }),
            ),
        onSucceeded: (envelope) => {
            const saved = envelope.model;
            // Re-baseline every signal from the write's answer explicitly: the
            // public ID is recomposed inside the write, so the returned model is
            // the form's new truth.
            setCurrent(saved);
            setIsEnabled(saved.is_enabled);
            setHasIdOverride(saved.id_override !== null);
            setIdOverride(saved.id_override ?? "");
            setHasLevelsOverride(saved.thinking_levels_override !== null);
            setSelectedLevels(saved.thinking_levels_override ?? []);
            setHasMaxInputOverride(saved.max_input_tokens_override !== null);
            setMaxInputText(saved.max_input_tokens_override?.toString() ?? "");
            setHasMaxOutputOverride(saved.max_output_tokens_override !== null);
            setMaxOutputText(saved.max_output_tokens_override?.toString() ?? "");

            // The routing line speaks only where clients calling the composed
            // ID actually reach the gateway. A vanished row stays suspended
            // whatever its settings say — `models:update` never touches
            // `discovery_state` — and its silence is carried by the standing
            // suspension notice under the header. Under a disabled provider
            // the row answers nothing either, but nothing else on this page
            // states that hold, so the line states it instead of promising
            // service: what waits is the provider's switch, not this save.
            let routingLine: string | null;
            if (saved.discovery_state === "vanished") {
                routingLine = null;
            } else if (props.provider !== null && !props.provider.is_enabled) {
                routingLine = `Clients will call it ${saved.public_id} once the provider is re-enabled.`;
            } else {
                routingLine = `Clients now call it ${saved.public_id}.`;
            }

            return {
                tone: "ok",
                title: "Model saved.",
                detail: routingLine,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader title="Model settings" />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-5"
                onSubmit={form.submit}
            >
                <Switch
                    label="Enabled"
                    isOn={isEnabled()}
                    isDisabled={form.isSaving()}
                    onToggle={setIsEnabled}
                />

                <ModelIdOverrideFields
                    hasIdOverride={hasIdOverride()}
                    onHasIdOverrideToggle={setHasIdOverride}
                    idOverride={idOverride()}
                    onIdOverrideInput={setIdOverride}
                    composedId={composedId()}
                    idOverrideError={overrideError()}
                    isDisabled={form.isSaving()}
                />

                <ThinkingLevelsOverrideFields
                    hasOverride={hasLevelsOverride()}
                    onHasOverrideToggle={setHasLevelsOverride}
                    selected={selectedLevels()}
                    onSelectedChange={setSelectedLevels}
                    discovered={current().thinking_levels}
                    isDisabled={form.isSaving()}
                />

                <TokenCapOverrideFields
                    switchLabel="Override max input tokens"
                    fieldLabel="Max input tokens override"
                    hasOverride={hasMaxInputOverride()}
                    onHasOverrideToggle={setHasMaxInputOverride}
                    value={maxInputText()}
                    onValueInput={setMaxInputText}
                    discovered={current().max_input_tokens}
                    valueError={maxInputError()}
                    isDisabled={form.isSaving()}
                />

                <TokenCapOverrideFields
                    switchLabel="Override max output tokens"
                    fieldLabel="Max output tokens override"
                    hasOverride={hasMaxOutputOverride()}
                    onHasOverrideToggle={setHasMaxOutputOverride}
                    value={maxOutputText()}
                    onValueInput={setMaxOutputText}
                    discovered={current().max_output_tokens}
                    valueError={maxOutputError()}
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
            title="Delete this model"
            description="Deleting removes the model at once. Clients calling its public ID start getting 404s."
            question={`Delete ${props.model.public_id}? Clients calling it 404 until it exists again — a discovered model returns as new on the next discovery (without a custom public ID); a manual one can be re-added from the Create model page as a new row.`}
            onConfirm={confirmDelete}
        />
    );
}
