import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { discoveryStateLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { AppLink } from "/src/ui/app-link";
import { Button, ButtonLink, Switch, TextField } from "/src/ui/controls";
import { Badge, EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { modelDiscoveryBadge, modelServingBadge } from "/src/ui/models-table";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

/**
 * One model's page: the facts the gateway derives, the two writable columns
 * the operator owns — public ID override, serving switch — and deletion.
 * The row is addressed by its never-recycled row identifier; the form opens
 * prefilled, tracks dirt, and saves the complete column state back. Saving
 * installs the row the gateway returns, because the public ID is recomposed
 * inside that write.
 */
export function ModelDetailScreen(props: {
    readonly api: ManagementApi;
    readonly modelId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [failureNotice, setFailureNotice] = createSignal<ScreenNotice | null>(null);
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

            setFailureNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setFailureNotice(null);
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
            <Show when={failureNotice()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={isReading() && !hasRead() && failureNotice() === null}>
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && model() === null}>
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <EmptyState
                    title="Model not found"
                    description={`The gateway holds no model #${props.modelId}. It was deleted, or the address was typed wrong — a deleted discovered row returns under a fresh identity on the next sync, never under this one.`}
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
                        <ModelEditor
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

function ModelEditor(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly initialModel: ModelResponse;
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "update-model") => void;
    readonly onReload: () => void;
}): JSX.Element {
    // The row's truth as the gateway last returned it; a save re-baselines
    // it onto the write's answer, because the public ID is recomposed
    // inside that write.
    const [current, setCurrent] = createSignal(props.initialModel);
    const [idOverride, setIdOverride] = createSignal(current().id_override ?? "");
    const [isEnabled, setIsEnabled] = createSignal(current().is_enabled);
    const [isSaving, setIsSaving] = createSignal(false);

    const composedId = (): string => {
        const prefix = props.provider?.model_prefix ?? "";

        return idOverride().trim() === ""
            ? `${prefix}${current().upstream_id}`
            : idOverride().trim();
    };

    const normalizedOverride = (): string | null =>
        idOverride().trim() === "" ? null : idOverride().trim();

    const isDirty = (): boolean =>
        normalizedOverride() !== current().id_override || isEnabled() !== current().is_enabled;

    const revert = (): void => {
        setIdOverride(current().id_override ?? "");
        setIsEnabled(current().is_enabled);
    };

    const save = async (): Promise<void> => {
        if (isSaving()) {
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
        // public ID is recomposed inside the write, so the returned row is
        // the editor's new truth.
        setCurrent(saved);
        setIdOverride(saved.id_override ?? "");
        setIsEnabled(saved.is_enabled);
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

    const serving = (): {
        readonly tone: "ok" | "warn" | "bad" | "muted" | "info";
        readonly label: string;
    } => modelServingBadge(current(), props.provider ?? undefined);

    const discovery = (): {
        readonly tone: "ok" | "warn" | "bad" | "muted" | "info";
        readonly label: string;
    } => modelDiscoveryBadge(current());

    return (
        <>
            <PageHeader
                title={current().public_id}
                description={`Row #${current().id} · ${current().upstream_id} on ${current().provider}.`}
                backHref={hrefs.models(current().provider_id)}
                backLabel="Models"
            >
                <Badge
                    tone={serving().tone}
                    label={serving().label}
                />
                <Badge
                    tone={discovery().tone}
                    label={discovery().label}
                />
            </PageHeader>

            <Show when={current().discovery_state === "vanished"}>
                <Notice
                    tone="warn"
                    title="Upstream no longer reports this model."
                    detail="The row keeps its curated columns but serves nothing. Sync again when the upstream lists it, or claim it back as a manual row from the creation page with the same provider and upstream ID."
                    fix={null}
                />
            </Show>

            <Card>
                <CardHeader title="Facts" />
                <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Public ID</dt>
                        <dd class="tnum mt-1 break-all">{current().public_id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Upstream ID</dt>
                        <dd class="tnum mt-1 break-all">{current().upstream_id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Provider</dt>
                        <dd class="mt-1">
                            {props.provider === null ? (
                                <span class="text-ink-dim">{current().provider}</span>
                            ) : (
                                <AppLink
                                    href={hrefs.providerDetail(props.provider.id)}
                                    class="font-medium text-accent hover:text-accent-strong"
                                >
                                    {props.provider.name}
                                </AppLink>
                            )}
                        </dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Discovery</dt>
                        <dd class="mt-1 text-ink-dim">
                            {discoveryStateLabel(current().discovery_state)}
                        </dd>
                    </div>
                </dl>
            </Card>

            <Card>
                <CardHeader
                    title="Writable columns"
                    description="The two things this page owns. Discovery state is never edited here — this page cannot restore a vanished row."
                />
                <form
                    novalidate={true}
                    class="mt-4 flex max-w-2xl flex-col gap-5"
                    onSubmit={submit}
                >
                    <TextField
                        label="Public ID override"
                        value={idOverride()}
                        onInput={setIdOverride}
                        placeholder="No override"
                        isDisabled={isSaving()}
                        hint={`Clients call it ${composedId()}. Renaming breaks clients on the old ID — there are no aliases.`}
                    />

                    <Switch
                        label="Serving"
                        hint="A model serves only while its own switch and its provider's are on, and the upstream still reports it."
                        isOn={isEnabled()}
                        onText="On"
                        offText="Off"
                        isDisabled={isSaving()}
                        onToggle={setIsEnabled}
                    />

                    <div class="flex flex-wrap items-center gap-2">
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!isDirty()}
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
        </>
    );
}

/** Deletion, administrator-only: discovery never deletes a row. */
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
                description="Deleting removes the row at once. Clients calling its public ID start getting 404s. Usage already recorded keeps attributing by this row's number, which is never handed to a later row."
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
                        {`Delete ${props.model.public_id}? A deleted discovered row returns as a new row with a fresh identity on the next sync — the manual claim on the creation page is the only way to keep working under this number.`}
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
