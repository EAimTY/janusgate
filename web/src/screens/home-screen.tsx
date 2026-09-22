import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, For, Show } from "solid-js";
import { credentialExpiryLine, isCredentialExpired, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { isModelEffectivelyServing } from "/src/management-api/serving";
import { isOAuthProviderRow, providerTypeLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton, Stat } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

type HomeData = {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly apiKeys: ReadonlyArray<ApiKeyResponse>;
};

/**
 * The overview: the gateway's health at a glance. Counts the operator can
 * trust, the providers that need a human, and the next step for a fresh
 * gateway. This page reports gateway state, not traffic — request-level
 * figures live on the Usage pages.
 */
export function HomeScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [data, setData] = createSignal<HomeData>({
        providers: [],
        models: [],
        apiKeys: [],
    });
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

        const [providersOutcome, modelsOutcome, apiKeysOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() => props.api.listModels({}, call.signal)),
            performManagementCall(() => props.api.listApiKeys(call.signal)),
        ]);

        if (
            providersOutcome.status === "aborted" ||
            modelsOutcome.status === "aborted" ||
            apiKeysOutcome.status === "aborted"
        ) {
            return;
        }

        setIsReading(false);

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : apiKeysOutcome.status === "failed"
                    ? apiKeysOutcome
                    : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setFailure(null);
        setData({
            providers: providersOutcome.status === "ok" ? providersOutcome.result.providers : [],
            models: modelsOutcome.status === "ok" ? modelsOutcome.result.models : [],
            apiKeys: apiKeysOutcome.status === "ok" ? apiKeysOutcome.result.api_keys : [],
        });
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const providersById = createMemo(
        () =>
            new Map<number, ProviderResponse>(
                data().providers.map((row) => [
                    row.id,
                    row,
                ]),
            ),
    );

    const servingModels = createMemo(
        () =>
            data().models.filter((model) =>
                isModelEffectivelyServing(model, providersById().get(model.provider_id)),
            ).length,
    );

    const enabledApiKeys = createMemo(() => data().apiKeys.filter((row) => row.is_enabled).length);

    const attentionProviders = createMemo(() =>
        data().providers.filter((provider) => credentialAttention(provider) !== null),
    );

    const emptyProviders = createMemo(() => {
        const modelCounts = new Map<number, number>();

        for (const model of data().models) {
            modelCounts.set(model.provider_id, (modelCounts.get(model.provider_id) ?? 0) + 1);
        }

        return data().providers.filter((provider) => (modelCounts.get(provider.id) ?? 0) === 0);
    });

    // Exactly what the list below counts: every item the section can show —
    // credential problems, providers without models, the missing first key —
    // so the headline number and the list can never contradict each other.
    const attentionCount = createMemo(
        () =>
            attentionProviders().length +
            emptyProviders().length +
            (data().apiKeys.length === 0 ? 1 : 0),
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Overview"
                description="The health of this gateway at a glance."
            >
                <ButtonLink
                    href={hrefs.providerNew()}
                    tone="primary"
                    glyph="plus"
                >
                    Create provider
                </ButtonLink>
            </PageHeader>

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
                <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
                    <For
                        each={[
                            0,
                            1,
                            2,
                            3,
                            4,
                        ]}
                    >
                        {() => (
                            <Card>
                                <Skeleton class="h-7 w-16" />
                                <Skeleton class="mt-3 h-4 w-24" />
                            </Card>
                        )}
                    </For>
                </div>
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={data().providers.length > 0}
                    fallback={<GettingStarted />}
                >
                    <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-5">
                        <Card>
                            <Stat
                                value={String(data().providers.length)}
                                label="Providers"
                                hint="Upstream accounts"
                            />
                        </Card>
                        <Card>
                            <Stat
                                value={String(data().models.length)}
                                label="Models"
                                hint="Discovered or added by hand"
                            />
                        </Card>
                        <Card>
                            <Stat
                                value={`${servingModels()} / ${data().models.length}`}
                                label="Serving"
                                hint="Provider on, model on, reported"
                            />
                        </Card>
                        <Card>
                            <Stat
                                value={String(data().apiKeys.length)}
                                label="API keys"
                                hint={`${enabledApiKeys()} enabled`}
                            />
                        </Card>
                        <Card>
                            <Stat
                                value={String(attentionCount())}
                                label="Need attention"
                                hint="Listed below"
                            />
                        </Card>
                    </div>

                    <Show
                        when={attentionCount() > 0}
                        fallback={
                            <Notice
                                tone="ok"
                                title="Everything looks healthy."
                                detail={null}
                                fix={null}
                            />
                        }
                    >
                        <Card>
                            <CardHeader title="Needs attention" />
                            <ul class="mt-4 flex flex-col divide-y divide-rule">
                                <Show when={data().apiKeys.length === 0}>
                                    <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
                                        <Glyph
                                            name="key"
                                            class="shrink-0 text-ink-faint"
                                        />
                                        <p class="min-w-0 flex-1 text-[13.5px]">
                                            <span class="font-medium">No API keys yet.</span>
                                            <span class="text-ink-dim">
                                                {
                                                    " Serving clients authenticate with one — without it, the gateway serves nothing."
                                                }
                                            </span>
                                        </p>
                                        <ButtonLink href={hrefs.apiKeyNew()}>
                                            Create API key
                                        </ButtonLink>
                                    </li>
                                </Show>
                                <For each={attentionProviders()}>
                                    {(provider) => <AttentionRow provider={provider} />}
                                </For>
                                <For each={emptyProviders()}>
                                    {(provider) => (
                                        <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
                                            <Glyph
                                                name="stack"
                                                class="shrink-0 text-ink-faint"
                                            />
                                            <p class="min-w-0 flex-1 text-[13.5px]">
                                                <span class="font-medium">{provider.name}</span>
                                                <span class="text-ink-dim">
                                                    {" has no models yet."}
                                                </span>
                                            </p>
                                            <ButtonLink href={hrefs.providerDetail(provider.id)}>
                                                Open and discover
                                            </ButtonLink>
                                        </li>
                                    )}
                                </For>
                            </ul>
                        </Card>
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

/** Why an OAuth provider needs a human: expired, or never fresh. */
function credentialAttention(provider: ProviderResponse): {
    readonly label: string;
    readonly detail: string;
} | null {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return {
            label: "Credential not fresh",
            detail: "No expiry is stored for this provider.",
        };
    }

    if (!isCredentialExpired(provider.expires_at, nowMicros())) {
        return null;
    }

    return {
        label: "Credential expired",
        detail: credentialExpiryLine(provider.expires_at, nowMicros()),
    };
}

function AttentionRow(props: { readonly provider: ProviderResponse }): JSX.Element {
    const attention = (): {
        readonly label: string;
        readonly detail: string;
    } =>
        credentialAttention(props.provider) ?? {
            label: "Needs attention",
            detail: "",
        };

    return (
        <li class="flex flex-wrap items-center gap-x-3 gap-y-2 py-3">
            <Badge
                tone="warn"
                label={attention().label}
            />
            <p class="min-w-0 flex-1 text-[13.5px]">
                <span class="font-medium">{props.provider.name}</span>
                <span class="text-ink-dim">{` · ${providerTypeLabel(props.provider.type)} · ${attention().detail}`}</span>
            </p>
            <ButtonLink href={hrefs.providerDetail(props.provider.id)}>Reconnect</ButtonLink>
        </li>
    );
}

/** A fresh gateway: the five steps from here to a working endpoint. */
function GettingStarted(): JSX.Element {
    return (
        <EmptyState
            glyph="plug"
            title="No providers yet"
            description="A provider is one upstream account. Its models arrive when you discover them."
        >
            <ButtonLink
                href={hrefs.providerNew()}
                tone="primary"
                glyph="plus"
            >
                Add your first provider
            </ButtonLink>
            <ol class="mt-2 flex w-full list-none flex-col gap-2 text-[13.5px]">
                <GettingStartedStep
                    index={1}
                    isDone
                    title="Connected to the gateway"
                />
                <GettingStartedStep
                    index={2}
                    title="Add a provider"
                />
                <GettingStartedStep
                    index={3}
                    title="Discover its models from upstream"
                />
                <GettingStartedStep
                    index={4}
                    title="Enable the provider"
                />
                <GettingStartedStep
                    index={5}
                    title="Issue an API key for serving clients"
                />
            </ol>
        </EmptyState>
    );
}

function GettingStartedStep(props: {
    readonly index: number;
    readonly title: string;
    readonly isDone?: boolean | undefined;
}): JSX.Element {
    return (
        <li class="flex items-center gap-3">
            <span
                class={
                    props.isDone === true
                        ? "grid size-5 shrink-0 place-items-center rounded-full bg-ok-soft text-ok"
                        : "grid size-5 shrink-0 place-items-center rounded-full bg-muted text-[12px] font-semibold text-ink-dim"
                }
            >
                {props.isDone === true ? <Glyph name="check" /> : props.index}
            </span>
            <span class={props.isDone === true ? "text-ink-dim line-through" : ""}>
                {props.title}
            </span>
        </li>
    );
}
