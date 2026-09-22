import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { formatCreatedAt } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { ApiKeySecretValue } from "/src/ui/api-key-form";
import { ButtonLink } from "/src/ui/controls";
import { Badge, EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

/**
 * One serving key's page: its secret, its facts, and what serving clients
 * did with it. Nothing is changed here — the edit page owns every change.
 * Deleting a key revokes it; unlike a provider, nothing cascades.
 */
export function ApiKeyDetailScreen(props: {
    readonly api: ManagementApi;
    readonly apiKeyId: number;
}): JSX.Element {
    const [keys, setKeys] = createSignal<ReadonlyArray<ApiKeyResponse>>([]);
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

        const outcome = await performManagementCall(() => props.api.listApiKeys(call.signal));

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
        setKeys(outcome.result.api_keys);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        props.apiKeyId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const apiKey = createMemo(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
    );

    return (
        <div class="rise-in flex flex-col gap-5">
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
                <PageHeader
                    title="API key"
                    backHref={hrefs.apiKeys()}
                    backLabel="API keys"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && apiKey() === null}>
                <PageHeader
                    title="API key"
                    backHref={hrefs.apiKeys()}
                    backLabel="API keys"
                />
                <EmptyState
                    title="API key not found"
                    description="The gateway holds no API key here. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back."
                >
                    <ButtonLink
                        href={hrefs.apiKeys()}
                        tone="primary"
                    >
                        Back to API keys
                    </ButtonLink>
                    <ButtonLink href={hrefs.apiKeyNew()}>Create API key</ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={apiKey()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={row.name}
                            description={`Created ${formatCreatedAt(row.created_at)}`}
                            backHref={hrefs.apiKeys()}
                            backLabel="API keys"
                        >
                            <Badge
                                tone={row.is_enabled ? "ok" : "muted"}
                                label={row.is_enabled ? "On" : "Off"}
                            />
                            <ButtonLink href={hrefs.apiKeyEdit(row.id)}>Edit API key</ButtonLink>
                        </PageHeader>

                        <Card>
                            <CardHeader title="Secret" />
                            <div class="mt-4">
                                <ApiKeySecretValue secret={row.key} />
                            </div>
                        </Card>

                        <Card>
                            <CardHeader title="Details" />
                            <dl class="mt-4 grid gap-x-8 gap-y-3 sm:grid-cols-2">
                                <div>
                                    <dt class="text-[12.5px] font-medium text-ink-dim">Name</dt>
                                    <dd class="mt-1">{row.name}</dd>
                                </div>
                                <div>
                                    <dt class="text-[12.5px] font-medium text-ink-dim">Created</dt>
                                    <dd class="tnum mt-1">{formatCreatedAt(row.created_at)}</dd>
                                </div>
                                <div>
                                    <dt class="text-[12.5px] font-medium text-ink-dim">Enabled</dt>
                                    <dd class="mt-1">{row.is_enabled ? "On" : "Off"}</dd>
                                </div>
                            </dl>
                        </Card>

                        <EntityUsageCard
                            api={props.api}
                            filter={{
                                api_key_id: row.id,
                            }}
                            subjectKind="API key"
                        />
                    </>
                )}
            </Show>
        </div>
    );
}
