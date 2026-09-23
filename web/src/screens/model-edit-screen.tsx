import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { compositionMessage, isModelIdentifier } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import {
    createHeldRow,
    createScreenRead,
    type FailureCall,
    isSameModelRow,
    isSameProviderRow,
    readAll,
    type ScreenNotice,
} from "/src/screens/read";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button, ButtonLink, Switch } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelIdOverrideFields } from "/src/ui/model-form";

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
    // Reported by the settings form: an open form with unsaved edits holds its
    // row against refreshes, so an operator's input can never silently vanish.
    const [isFormDirty, setIsFormDirty] = createSignal(false);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        reReadOn: () => props.modelId,
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
        },
    });

    const latestModel = createMemo(() => models().find((row) => row.id === props.modelId) ?? null);

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the form holds unsaved edits, and the hold is stated.
    const { row: model, isRowHeld } = createHeldRow(latestModel, isFormDirty, isSameModelRow);

    // The owning provider, kept identity-stable for the same reason: the
    // listing re-parses on every read, so an unpinned lookup would hand the
    // form a fresh object each refresh and remount it over the operator's
    // input even when nothing changed.
    const provider = createMemo((previous: ProviderResponse | null | undefined) => {
        const current = model();
        const fresh =
            current === null
                ? null
                : (providers().find((row) => row.id === current.provider_id) ?? null);

        if (previous === null || previous === undefined || fresh === null) {
            return fresh;
        }

        return isSameProviderRow(previous, fresh) ? previous : fresh;
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={read.notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={read.isReading() && !read.hasRead() && read.notice() === null}>
                <PageHeader
                    title="Edit model"
                    backHref={hrefs.modelDetail(props.modelId)}
                    backLabel="Model"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead() && model() === null}>
                <PageHeader
                    title="Edit model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <EmptyState
                    title="Model not found"
                    description="The gateway holds no model here. It was deleted, or the address was typed wrong — a deleted discovered model returns as new on the next discovery."
                >
                    <ButtonLink
                        href={hrefs.models(null)}
                        tone="primary"
                    >
                        Back to models
                    </ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={model()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={`Edit ${row.public_id}`}
                            backHref={hrefs.modelDetail(row.id)}
                            backLabel="Model"
                        />

                        <Show when={isRowHeld()}>
                            <Notice
                                tone="warn"
                                title="This model changed on the gateway while you have unsaved edits."
                                detail="Save writes your edits over the gateway's version; Revert picks the gateway's version up."
                            />
                        </Show>

                        <ModelSettingsForm
                            api={props.api}
                            provider={provider()}
                            initialModel={row}
                            onNotice={read.showNotice}
                            onFailure={read.reportFailure}
                            onReload={read.reload}
                            onDirtyChange={setIsFormDirty}
                        />
                        <DeleteCard
                            api={props.api}
                            model={row}
                            onFailure={read.reportFailure}
                        />
                    </>
                )}
            </Show>
        </div>
    );
}

function ModelSettingsForm(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly initialModel: ModelResponse;
    readonly onNotice: (notice: ScreenNotice | null) => void;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
    readonly onReload: () => void;
    readonly onDirtyChange: (isDirty: boolean) => void;
}): JSX.Element {
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the write's answer, because the public ID is recomposed
    // inside that write.
    const [current, setCurrent] = createSignal(props.initialModel);
    const [isEnabled, setIsEnabled] = createSignal(current().is_enabled);
    const [hasIdOverride, setHasIdOverride] = createSignal(current().id_override !== null);
    const [idOverride, setIdOverride] = createSignal(current().id_override ?? "");
    const [isSaving, setIsSaving] = createSignal(false);

    const composedId = (): string => {
        const prefix = props.provider?.model_prefix ?? "";

        return idOverride().trim() === ""
            ? `${prefix}${current().upstream_id}`
            : idOverride().trim();
    };

    const normalizedOverride = (): string | null =>
        !hasIdOverride() || idOverride().trim() === "" ? null : idOverride().trim();

    /** Inline composition error for the override, shown where it was typed. */
    const overrideError = (): string | null => {
        const override = normalizedOverride();
        return override === null || isModelIdentifier(override)
            ? null
            : compositionMessage("Public ID override");
    };

    const isDirty = (): boolean =>
        isEnabled() !== current().is_enabled || normalizedOverride() !== current().id_override;

    // The screen holds the form's row while this dirt stands (README §6.3).
    createEffect(
        () => isDirty(),
        (isDirtyValue) => {
            props.onDirtyChange(isDirtyValue);
        },
    );

    const canSave = (): boolean => isDirty() && overrideError() === null;

    const revert = (): void => {
        setIsEnabled(current().is_enabled);
        setHasIdOverride(current().id_override !== null);
        setIdOverride(current().id_override ?? "");
        // Reverting resolves a held row onto the gateway's current version.
        props.onReload();
    };

    const save = async (): Promise<void> => {
        if (isSaving() || !canSave()) {
            return;
        }

        setIsSaving(true);

        const outcome = await performManagementCall(() =>
            props.api.updateModel(
                toUpdateModelRequest(current(), {
                    id_override: normalizedOverride(),
                    is_enabled: isEnabled(),
                }),
            ),
        );

        setIsSaving(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, {
                action: "update-model",
            });
            return;
        }

        const saved = outcome.result.model;
        // Re-baseline every signal from the write's answer explicitly: the
        // public ID is recomposed inside the write, so the returned model is
        // the form's new truth.
        setCurrent(saved);
        setIsEnabled(saved.is_enabled);
        setHasIdOverride(saved.id_override !== null);
        setIdOverride(saved.id_override ?? "");
        props.onNotice({
            tone: "ok",
            title: "Model saved.",
            detail: `Clients now call it ${saved.public_id}.`,
        });
        props.onReload();
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void save();
    };

    return (
        <Card>
            <CardHeader title="Model settings" />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-5"
                onSubmit={submit}
            >
                <Switch
                    label="Enabled"
                    isOn={isEnabled()}
                    onText="On"
                    offText="Off"
                    isDisabled={isSaving()}
                    onToggle={setIsEnabled}
                />

                <ModelIdOverrideFields
                    hasIdOverride={hasIdOverride()}
                    onHasIdOverrideToggle={setHasIdOverride}
                    idOverride={idOverride()}
                    onIdOverrideInput={setIdOverride}
                    composedId={composedId()}
                    idOverrideError={overrideError()}
                    isDisabled={isSaving()}
                />

                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        type="submit"
                        tone="primary"
                        isDisabled={!canSave()}
                        isBusy={isSaving()}
                    >
                        {isSaving() ? "Saving…" : "Save changes"}
                    </Button>
                    <Button
                        isDisabled={!isDirty() || isSaving()}
                        onClick={revert}
                    >
                        Revert
                    </Button>
                    <Show when={isDirty() && !isSaving()}>
                        <span
                            class="text-[13px] font-medium text-warn"
                            role="status"
                        >
                            Unsaved changes
                        </span>
                    </Show>
                </div>
            </form>
        </Card>
    );
}

/** Deletion, administrator-only: discovery never deletes a model. */
function DeleteCard(props: {
    readonly api: ManagementApi;
    readonly model: ModelResponse;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
}): JSX.Element {
    const confirmDelete = async (): Promise<boolean> => {
        const outcome = await performManagementCall(() =>
            props.api.deleteModel({
                model_id: props.model.id,
            }),
        );

        if (outcome.status === "aborted") {
            return false;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, {
                action: "delete-model",
            });
            return false;
        }

        navigate(hrefs.models(props.model.provider_id));
        return true;
    };

    return (
        <ConfirmDeleteCard
            title="Delete this model"
            description="Deleting removes the model at once. Clients calling its public ID start getting 404s."
            question={`Delete ${props.model.public_id}? A deleted discovered model returns as new on the next discovery — claiming it back from the Create model page keeps it serving under the same ID.`}
            onConfirm={confirmDelete}
        />
    );
}
