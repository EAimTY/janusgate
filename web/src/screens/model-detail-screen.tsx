import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { describeLevels } from "/src/format";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ProviderResponse,
    ThinkingLevel,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { AppLink } from "/src/ui/app-link";
import { Button, ButtonLink, Switch, TextField } from "/src/ui/controls";
import { Badge, EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { modelServingBadge } from "/src/ui/models-table";
import { ThinkingLevels } from "/src/ui/thinking-levels";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

/**
 * One model's page: the facts the gateway derives, and the three overrides
 * the operator owns — public ID, serving switch, thinking levels. The form
 * opens prefilled, tracks dirt, and saves the row's complete state back.
 * Saving installs the row the gateway returns, because the public ID is
 * recomposed inside that write.
 */
export function ModelDetailScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
    readonly upstreamId: string;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [failureNotice, setFailureNotice] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() =>
                props.api.listModels(
                    {
                        provider_id: props.providerId,
                    },
                    call.signal,
                ),
            ),
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
        props.providerId;
        props.upstreamId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const provider = createMemo(
        () => providers().find((row) => row.id === props.providerId) ?? null,
    );

    const model = createMemo(
        () => models().find((row) => row.upstream_id === props.upstreamId) ?? null,
    );

    const loaded = createMemo(() => {
        const row = provider();
        const current = model();
        return row !== null && current !== null
            ? {
                  provider: row,
                  model: current,
              }
            : null;
    });

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

            <Show when={hasRead() && (provider() === null || model() === null)}>
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
                <EmptyState
                    title={provider() === null ? "Provider not found" : "Model not found"}
                    description={
                        provider() === null
                            ? `The gateway holds no provider #${props.providerId}, so this model address is dead.`
                            : "The gateway holds no such model on this provider. It was removed by a sync, or the address was typed wrong."
                    }
                >
                    <ButtonLink
                        href={hrefs.models(null)}
                        tone="primary"
                    >
                        Back to models
                    </ButtonLink>
                    <Show when={provider() !== null}>
                        <ButtonLink href={hrefs.providerDetail(props.providerId)}>
                            Open the provider
                        </ButtonLink>
                    </Show>
                </EmptyState>
            </Show>

            <Show
                when={loaded()}
                keyed
            >
                {(both) => (
                    <ModelEditor
                        api={props.api}
                        provider={both.provider}
                        initialModel={both.model}
                    />
                )}
            </Show>
        </div>
    );
}

function ModelEditor(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly initialModel: ModelResponse;
}): JSX.Element {
    // The row's truth as the gateway last returned it; a save re-baselines
    // it onto the write's answer, because the public ID is recomposed
    // inside that write.
    const [current, setCurrent] = createSignal(props.initialModel);
    const [idOverride, setIdOverride] = createSignal(current().id_override ?? "");
    const [isEnabled, setIsEnabled] = createSignal(current().is_enabled);
    const [levels, setLevels] = createSignal<ReadonlyArray<ThinkingLevel> | null>(
        current().thinking_levels_override,
    );
    const [isSaving, setIsSaving] = createSignal(false);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);

    const composedId = (): string =>
        idOverride().trim() === ""
            ? `${props.provider.model_prefix}${current().upstream_id}`
            : idOverride().trim();

    const normalizedOverride = (): string | null =>
        idOverride().trim() === "" ? null : idOverride().trim();

    const levelsEqual = (
        left: ReadonlyArray<ThinkingLevel> | null,
        right: ReadonlyArray<ThinkingLevel> | null,
    ): boolean => {
        if (left === null || right === null) {
            return left === right;
        }

        return left.length === right.length && left.every((level) => right.includes(level));
    };

    const isDirty = (): boolean =>
        normalizedOverride() !== current().id_override ||
        isEnabled() !== current().is_enabled ||
        !levelsEqual(levels(), current().thinking_levels_override);

    const revert = (): void => {
        setIdOverride(current().id_override ?? "");
        setIsEnabled(current().is_enabled);
        setLevels(current().thinking_levels_override);
    };

    const save = async (): Promise<void> => {
        if (isSaving()) {
            return;
        }

        setIsSaving(true);
        setNotice(null);

        const selectedLevels = levels();

        const outcome = await performManagementCall(() =>
            props.api.updateModel(
                toUpdateModelRequest(current(), {
                    id_override: normalizedOverride(),
                    is_enabled: isEnabled(),
                    thinking_levels_override:
                        selectedLevels === null
                            ? null
                            : [
                                  ...selectedLevels,
                              ],
                }),
            ),
        );

        setIsSaving(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, "update-model");

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        const saved = outcome.result.model;
        // Re-baseline every signal from the write's answer explicitly: the
        // public ID is recomposed inside the write, so the returned row is
        // the editor's new truth. Each signal is set from `saved` rather
        // than re-read, so this holds however updates flush.
        setCurrent(saved);
        setIdOverride(saved.id_override ?? "");
        setIsEnabled(saved.is_enabled);
        setLevels(saved.thinking_levels_override);
        setNotice({
            tone: "ok",
            title: "Model saved.",
            detail: `Clients now call it ${saved.id}.`,
        });
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void save();
    };

    const serving = (): {
        readonly tone: "ok" | "warn" | "bad" | "muted" | "info";
        readonly label: string;
    } => modelServingBadge(current(), props.provider);

    return (
        <>
            <PageHeader
                title={current().id}
                description={`A model of ${props.provider.name}.`}
                backHref={hrefs.models(props.provider.id)}
                backLabel="Models"
            >
                <Badge
                    tone={serving().tone}
                    label={serving().label}
                />
            </PageHeader>

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

            <Card>
                <CardHeader title="Facts" />
                <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Public ID</dt>
                        <dd class="tnum mt-1 break-all">{current().id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Upstream ID</dt>
                        <dd class="tnum mt-1 break-all">{current().upstream_id}</dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Provider</dt>
                        <dd class="mt-1">
                            <AppLink
                                href={hrefs.providerDetail(props.provider.id)}
                                class="font-medium text-accent hover:text-accent-strong"
                            >
                                {props.provider.name}
                            </AppLink>
                        </dd>
                    </div>
                    <div>
                        <dt class="text-[12.5px] font-medium text-ink-dim">Thinking levels</dt>
                        <dd class="mt-1 text-ink-dim">
                            {describeLevels(current().thinking_levels_override)}
                        </dd>
                    </div>
                </dl>
            </Card>

            <Card>
                <CardHeader
                    title="Overrides"
                    description="The three things this page owns. Everything else about the model comes from upstream."
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
                        hint="A model serves only while both its own switch and its provider's are on."
                        isOn={isEnabled()}
                        onText="On"
                        offText="Off"
                        isDisabled={isSaving()}
                        onToggle={setIsEnabled}
                    />

                    <ThinkingLevels
                        allowedLevels={levels()}
                        onChange={setLevels}
                        isDisabled={isSaving()}
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
