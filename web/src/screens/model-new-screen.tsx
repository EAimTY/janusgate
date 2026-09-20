import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, For, Show } from "solid-js";
import { toCreateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderResponse, ThinkingLevel } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { Button, ButtonLink, SelectField, Switch, TextField } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ThinkingLevels } from "/src/ui/thinking-levels";

/**
 * One creation page for a manually added model. The provider is picked once —
 * the row belongs to it — and the three overrides match the model page field
 * for field. Creating lands on the new model's page, like a provider creation
 * does. A manual row is indistinguishable from a discovered one afterwards: a
 * later sync keeps its overrides when the upstream ID reappears and drops the
 * row when it does not.
 */
export function ModelNewScreen(props: {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const outcome = await performManagementCall(() => props.api.listProviders(call.signal));

        if (outcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause);

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setFailure(null);
        setProviders(outcome.result.providers);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="New model"
                description="A model row sync would not produce. Pick its provider, name its upstream ID, set its overrides."
                backHref={hrefs.models(props.providerFilter)}
                backLabel="Models"
            />

            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={isReading() && !hasRead() && failure() === null}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title="No providers yet"
                            description="A model belongs to a provider, and there is none to hold it. Add a provider first — its sync may already produce this model."
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                New provider
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <ModelCreateForm
                        api={props.api}
                        providers={providers()}
                        initialProviderId={props.providerFilter}
                    />
                </Show>
            </Show>
        </div>
    );
}

function ModelCreateForm(props: {
    readonly api: ManagementApi;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly initialProviderId: number | null;
}): JSX.Element {
    const initialSelection = (): string => {
        if (
            props.initialProviderId !== null &&
            props.providers.some((row) => row.id === props.initialProviderId)
        ) {
            return String(props.initialProviderId);
        }

        return props.providers.length === 1 && props.providers[0] !== undefined
            ? String(props.providers[0].id)
            : "";
    };

    const [providerId, setProviderId] = createSignal(initialSelection());
    const [upstreamId, setUpstreamId] = createSignal("");
    const [idOverride, setIdOverride] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [levels, setLevels] = createSignal<ReadonlyArray<ThinkingLevel> | null>(null);
    const [isCreating, setIsCreating] = createSignal(false);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    const selectedProvider = createMemo(
        () => props.providers.find((row) => String(row.id) === providerId()) ?? null,
    );

    const trimmedUpstream = (): string => upstreamId().trim();

    const normalizedOverride = (): string | null =>
        idOverride().trim() === "" ? null : idOverride().trim();

    const composedId = (): string => {
        const provider = selectedProvider();

        if (provider === null) {
            return normalizedOverride() ?? trimmedUpstream();
        }

        return normalizedOverride() ?? `${provider.model_prefix}${trimmedUpstream()}`;
    };

    const canCreate = (): boolean =>
        selectedProvider() !== null && trimmedUpstream() !== "" && !isCreating();

    const create = async (): Promise<void> => {
        const provider = selectedProvider();

        if (provider === null || trimmedUpstream() === "" || isCreating()) {
            return;
        }

        setFailure(null);
        setIsCreating(true);

        const selectedLevels = levels();

        const outcome = await performManagementCall(() =>
            props.api.createModel(
                toCreateModelRequest(provider.id, trimmedUpstream(), {
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

        setIsCreating(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, "create-model");

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        navigate(
            hrefs.modelDetail(outcome.result.model.provider_id, outcome.result.model.upstream_id),
        );
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void create();
    };

    const providerOptions = createMemo(() => [
        {
            value: "",
            label: "Choose a provider",
        },
        ...props.providers.map((row) => ({
            value: String(row.id),
            label: `${row.name} (#${row.id})`,
        })),
    ]);

    return (
        <>
            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Notice
                tone="warn"
                title="A manual row lives or dies by the next sync."
                detail="Sync keeps its overrides when this upstream ID reappears, and drops the row when it does not. An upstream reporting no models keeps every curated row."
            />

            <Card>
                <CardHeader
                    title="Identity"
                    description="Which provider holds the row, and the exact upstream identifier it serves."
                />
                <div class="mt-4 flex max-w-2xl flex-col gap-4">
                    <SelectField
                        label="Provider"
                        value={providerId()}
                        options={providerOptions()}
                        onChange={setProviderId}
                        isDisabled={isCreating()}
                        hint="The row's owner. A changed prefix later recomposes every unoverridden public ID on it."
                    />
                    <TextField
                        label="Upstream ID"
                        value={upstreamId()}
                        onInput={setUpstreamId}
                        placeholder="gpt-4o"
                        isDisabled={isCreating()}
                        hint="The exact identifier sent upstream. Verbatim, like a discovered row — no mapping, no alias."
                    />
                </div>
            </Card>

            <Card>
                <CardHeader
                    title="Overrides"
                    description="The three things a model page owns. Everything else about the model comes from upstream."
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
                        isDisabled={isCreating()}
                        hint={
                            trimmedUpstream() === ""
                                ? "Clients call the composed ID once the upstream ID is named."
                                : `Clients will call it ${composedId()}. Renaming breaks clients on the old ID — there are no aliases.`
                        }
                    />

                    <Switch
                        label="Serving"
                        hint="A model serves only while both its own switch and its provider's are on."
                        isOn={isEnabled()}
                        onText="On"
                        offText="Off"
                        isDisabled={isCreating()}
                        onToggle={setIsEnabled}
                    />

                    <ThinkingLevels
                        allowedLevels={levels()}
                        onChange={setLevels}
                        isDisabled={isCreating()}
                    />

                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!canCreate()}
                            isBusy={isCreating()}
                        >
                            {isCreating() ? "Creating…" : "Create model"}
                        </Button>
                    </div>
                </form>
            </Card>

            <Show when={props.providers.length > 1}>
                <p class="text-[12.5px] text-ink-faint">
                    <For each={props.providers}>
                        {(row, index) => (
                            <>
                                {index() > 0 ? " · " : ""}
                                {row.name}
                            </>
                        )}
                    </For>
                    {` — ${props.providers.length} providers hold rows.`}
                </p>
            </Show>
        </>
    );
}
