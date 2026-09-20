import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, For, Show } from "solid-js";
import { describeValidity } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { isOAuthProviderRow, providerTypeLabel } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { AppLink } from "/src/ui/app-link";
import { ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeTone, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { PageHeader } from "/src/ui/layout";

/** The one readable statement of a provider's credential state. */
export function credentialBadge(provider: ProviderResponse): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    if (!isOAuthProviderRow(provider)) {
        return {
            tone: "muted",
            label: "API key",
        };
    }

    if (provider.expires_at === null) {
        return {
            tone: "warn",
            label: "Stale",
        };
    }

    const validity = describeValidity(provider.expires_at, Math.floor(Date.now() / 1000));

    return validity.isExpired
        ? {
              tone: "warn",
              label: "Expired",
          }
        : {
              tone: "ok",
              label: "Valid",
          };
}

/**
 * The provider list: one row per upstream account. Rows carry identity,
 * type, model count, and the two states — nothing else, and no model's
 * details. Everything about one provider lives on its own page.
 */
export function ProvidersScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [search, setSearch] = createSignal("");
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

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setFailure(null);
        setProviders(providersOutcome.status === "ok" ? providersOutcome.result.providers : []);
        setModels(modelsOutcome.status === "ok" ? modelsOutcome.result.models : []);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const modelCounts = createMemo(() => {
        const counts = new Map<number, number>();

        for (const model of models()) {
            counts.set(model.provider_id, (counts.get(model.provider_id) ?? 0) + 1);
        }

        return counts;
    });

    const visibleProviders = createMemo(() => {
        const needle = search().trim().toLowerCase();

        if (needle === "") {
            return providers();
        }

        return providers().filter(
            (provider) =>
                provider.name.toLowerCase().includes(needle) ||
                providerTypeLabelOf(provider).toLowerCase().includes(needle),
        );
    });

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Providers"
                description="Upstream accounts. Open one to edit it, sync its models, or fix its credentials."
            >
                <ButtonLink
                    href={hrefs.providerNew()}
                    tone="primary"
                    glyph="plus"
                >
                    New provider
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
                <div class="flex flex-col gap-2">
                    <For
                        each={[
                            0,
                            1,
                            2,
                        ]}
                    >
                        {() => <Skeleton class="h-16 w-full rounded-xl" />}
                    </For>
                </div>
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="plug"
                            title="No providers yet"
                            description="A provider is one upstream account — a Codex OAuth or Antigravity OAuth login, or a key on any OpenAI-compatible API."
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
                    <div class="relative">
                        <Glyph
                            name="search"
                            class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-ink-faint"
                        />
                        <input
                            type="search"
                            aria-label="Search providers"
                            placeholder="Search by name or type"
                            value={search()}
                            onInput={(event) => setSearch(event.currentTarget.value)}
                            class="w-full rounded-lg border border-rule-strong bg-surface py-2 pr-3 pl-9 text-ink placeholder:text-ink-faint transition-colors duration-150 ease-out hover:border-ink-faint focus:border-accent focus:outline-none sm:max-w-xs"
                        />
                    </div>

                    <Show
                        when={visibleProviders().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={`No providers match "${search().trim()}".`}
                            />
                        }
                    >
                        <ul class="flex flex-col gap-2">
                            <For each={visibleProviders()}>
                                {(provider) => (
                                    <ProviderRow
                                        provider={provider}
                                        modelCount={modelCounts().get(provider.id) ?? 0}
                                    />
                                )}
                            </For>
                        </ul>
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

function providerTypeLabelOf(provider: ProviderResponse): string {
    return providerTypeLabel(provider.type);
}

function ProviderRow(props: {
    readonly provider: ProviderResponse;
    readonly modelCount: number;
}): JSX.Element {
    const credential = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => credentialBadge(props.provider);

    return (
        <li>
            <AppLink
                href={hrefs.providerDetail(props.provider.id)}
                class="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3.5 transition-colors duration-150 ease-out hover:border-ink-faint"
            >
                <span class="min-w-0 flex-1 basis-48">
                    <span class="block truncate text-[14.5px] font-semibold">
                        {props.provider.name}
                    </span>
                    <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                        {`#${props.provider.id} · ${providerTypeLabel(props.provider.type)} · ${props.modelCount} ${props.modelCount === 1 ? "model" : "models"}`}
                    </span>
                </span>
                <span class="flex items-center gap-2">
                    <Badge
                        tone={props.provider.is_enabled ? "ok" : "muted"}
                        label={props.provider.is_enabled ? "Serving" : "Off"}
                    />
                    <Badge
                        tone={credential().tone}
                        label={credential().label}
                    />
                </span>
                <Glyph
                    name="chev-right"
                    class="text-ink-faint transition-transform duration-150 ease-out group-hover:translate-x-0.5 group-hover:text-ink-dim"
                />
            </AppLink>
        </li>
    );
}
