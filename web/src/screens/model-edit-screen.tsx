import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { compositionMessage, isModelIdentifier } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { Button, ButtonLink, Switch } from "/src/ui/controls";
import { EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelIdOverrideFields } from "/src/ui/model-form";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

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
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() => props.api.listModels({}, call.signal)),
        ]);

        if (providersOutcome.status === "aborted" || modelsOutcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        // A successful read keeps the notice: a save reports through it just
        // before this reload runs, and only a new failure replaces it.
        setProviders(providersOutcome.status === "ok" ? providersOutcome.result.providers : []);
        setModels(modelsOutcome.status === "ok" ? modelsOutcome.result.models : []);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        props.modelId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const model = createMemo(() => models().find((row) => row.id === props.modelId) ?? null);

    const provider = createMemo(() => {
        const current = model();
        return current === null
            ? null
            : (providers().find((row) => row.id === current.provider_id) ?? null);
    });

    const reportFailure = (cause: unknown, action: "update-model" | "delete-model"): void => {
        const diagnosis = describeCallFailure(cause, action);

        setNotice({
            tone: "bad",
            title: diagnosis.headline,
            detail: diagnosis.detail,
            fix: diagnosis.fix,
        });
    };

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={isReading() && !hasRead() && notice() === null}>
                <PageHeader
                    title="Edit model"
                    backHref={hrefs.modelDetail(props.modelId)}
                    backLabel="Model"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && model() === null}>
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
                when={(() => {
                    const current = model();
                    return current === null
                        ? null
                        : {
                              model: current,
                              provider: provider(),
                          };
                })()}
                keyed
            >
                {(both) => (
                    <>
                        <PageHeader
                            title={`Edit ${both.model.public_id}`}
                            backHref={hrefs.modelDetail(both.model.id)}
                            backLabel="Model"
                        />

                        <ModelSettingsForm
                            api={props.api}
                            provider={both.provider}
                            initialModel={both.model}
                            onNotice={setNotice}
                            onFailure={reportFailure}
                            onReload={() => void read()}
                        />
                        <DeleteCard
                            api={props.api}
                            model={both.model}
                            onFailure={reportFailure}
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
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "update-model") => void;
    readonly onReload: () => void;
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

    const canSave = (): boolean => isDirty() && overrideError() === null;

    const revert = (): void => {
        setIsEnabled(current().is_enabled);
        setHasIdOverride(current().id_override !== null);
        setIdOverride(current().id_override ?? "");
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
            props.onFailure(outcome.cause, "update-model");
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
    readonly onFailure: (cause: unknown, action: "delete-model") => void;
}): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);

    const remove = async (): Promise<void> => {
        if (isDeleting()) {
            return;
        }

        setIsDeleting(true);

        const outcome = await performManagementCall(() =>
            props.api.deleteModel({
                model_id: props.model.id,
            }),
        );

        setIsDeleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, "delete-model");
            setIsArmed(false);
            return;
        }

        navigate(hrefs.models(props.model.provider_id));
    };

    return (
        <Card class="border-bad/30">
            <CardHeader
                title="Delete this model"
                description="Deleting removes the model at once. Clients calling its public ID start getting 404s."
            />
            <Show
                when={isArmed()}
                fallback={
                    <div class="mt-4">
                        <Button
                            tone="danger"
                            glyph="trash"
                            onClick={() => setIsArmed(true)}
                        >
                            Delete…
                        </Button>
                    </div>
                }
            >
                <div class="mt-4 flex flex-wrap items-center gap-2">
                    <p class="w-full text-[13.5px] text-ink-dim">
                        {`Delete ${props.model.public_id}? A deleted discovered model returns as new on the next discovery — claiming it back from the Create model page keeps it serving under the same ID.`}
                    </p>
                    <Button
                        tone="danger"
                        isBusy={isDeleting()}
                        onClick={() => void remove()}
                    >
                        {isDeleting() ? "Deleting…" : "Delete for good"}
                    </Button>
                    <Button
                        isDisabled={isDeleting()}
                        onClick={() => setIsArmed(false)}
                    >
                        Keep it
                    </Button>
                </div>
            </Show>
        </Card>
    );
}
