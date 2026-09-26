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
    readAll,
    type ScreenRead,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import {
    composedPublicId,
    ModelIdOverrideFields,
    normalizedPublicIdOverride,
} from "/src/ui/model-form";
import { createDeleteAction, createEditForm } from "./edit-form";
import { RowNotFound } from "./row-not-found";

/**
 * One model's edit page: its settings and its deletion. The form opens
 * prefilled, tracks dirt, and saves the complete settings back — saving
 * installs the model the gateway returns, because the public ID is
 * recomposed inside that write. Reading the model — its facts and its usage
 * — lives on the detail page.
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
            const [providersResult, modelsResult] = await readAll(
                signal,
                {
                    run: (cycle) => props.api.listProviders(cycle),
                },
                {
                    run: (cycle) => props.api.listModels({}, cycle),
                },
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);
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

    const isDirty = (): boolean =>
        isEnabled() !== current().is_enabled || normalizedOverride() !== current().id_override;

    const canSave = (): boolean => isDirty() && overrideError() === null;

    const revert = (): void => {
        setIsEnabled(current().is_enabled);
        setHasIdOverride(current().id_override !== null);
        setIdOverride(current().id_override ?? "");
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

            return {
                tone: "ok",
                title: "Model saved.",
                detail: `Clients now call it ${saved.public_id}.`,
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
            question={`Delete ${props.model.public_id}? A deleted discovered model returns as new on the next discovery — claiming it back from the Create model page keeps it serving under the same ID.`}
            onConfirm={confirmDelete}
        />
    );
}
