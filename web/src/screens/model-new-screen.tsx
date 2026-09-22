import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { toCreateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { Button, ButtonLink, SelectField, Switch, TextField } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelAliasFields } from "/src/ui/model-form";

/**
 * One creation page for a manually added model. When opened from a
 * provider's page the provider is fixed — there is nothing to pick — and
 * otherwise it is picked once, because the model belongs to it. The
 * settings match the edit page field for field. Creating lands on the new
 * model's page, like a provider creation does. The same shape claims back a
 * model the upstream no longer reports: naming its provider and upstream ID
 * resumes service under the requested state instead of inserting a new row.
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

    const fixedProvider = createMemo(() =>
        props.providerFilter === null
            ? null
            : (providers().find((row) => row.id === props.providerFilter) ?? null),
    );

    const backHref = (): string =>
        fixedProvider() === null
            ? hrefs.models(props.providerFilter)
            : hrefs.providerDetail(fixedProvider()?.id ?? 0);
    const backLabel = (): string => (fixedProvider() === null ? "Models" : "Provider");

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Create model"
                description="Something discovery would not produce — or a model the upstream no longer reports, to claim back."
                backHref={backHref()}
                backLabel={backLabel()}
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
                            description="A model belongs to a provider, and there is none to hold it. Add a provider first — discovery may already produce this model."
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                Create provider
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <ModelCreateForm
                        api={props.api}
                        providers={providers()}
                        initialProviderId={props.providerFilter}
                        fixedProvider={fixedProvider()}
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
    readonly fixedProvider: ProviderResponse | null;
}): JSX.Element {
    const initialSelection = (): string => {
        if (props.fixedProvider !== null) {
            return String(props.fixedProvider.id);
        }

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
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [hasAlias, setHasAlias] = createSignal(false);
    const [idOverride, setIdOverride] = createSignal("");
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
        !hasAlias() || idOverride().trim() === "" ? null : idOverride().trim();

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

        const outcome = await performManagementCall(() =>
            props.api.createModel(
                toCreateModelRequest(provider.id, trimmedUpstream(), {
                    id_override: normalizedOverride(),
                    is_enabled: isEnabled(),
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

        navigate(hrefs.modelDetail(outcome.result.model.id));
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
            label: row.name,
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
                title="A model added by hand is never removed automatically — and it claims back what discovery lost."
                detail="Naming a provider and upstream ID the upstream no longer reports claims that model back under the settings below."
            />

            <Card>
                <CardHeader title="Identity" />
                <div class="mt-4 flex max-w-2xl flex-col gap-4">
                    <Show
                        when={props.fixedProvider}
                        fallback={
                            <SelectField
                                label="Provider"
                                value={providerId()}
                                options={providerOptions()}
                                onChange={setProviderId}
                                isDisabled={isCreating()}
                            />
                        }
                    >
                        {(fixed) => (
                            <div>
                                <p class="block text-[13px] font-medium text-ink">Provider</p>
                                <p class="tnum mt-1.5">{fixed().name}</p>
                            </div>
                        )}
                    </Show>
                    <TextField
                        label="Upstream ID"
                        value={upstreamId()}
                        onInput={setUpstreamId}
                        placeholder="gpt-4o"
                        isDisabled={isCreating()}
                        hint="Sent upstream verbatim."
                    />
                </div>
            </Card>

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
                        isDisabled={isCreating()}
                        onToggle={setIsEnabled}
                    />

                    <ModelAliasFields
                        hasAlias={hasAlias()}
                        onHasAliasToggle={setHasAlias}
                        alias={idOverride()}
                        onAliasInput={setIdOverride}
                        composedId={composedId()}
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
        </>
    );
}
