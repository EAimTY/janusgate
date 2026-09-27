import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import {
    createPinnedRow,
    createScreenRead,
    isSameApiKeyRow,
    readApiKeys,
    readOne,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { ApiKeySecretValue } from "/src/ui/api-key-form";
import { ButtonLink } from "/src/ui/controls";
import { Badge } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "./row-not-found";

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

    const read = createScreenRead({
        load: async (signal) => {
            const keysResult = await readOne(signal, readApiKeys(props.api));

            setKeys(keysResult.api_keys);
        },
    });

    // The row keeps its previous answer when a re-read returns the same
    // content (`createPinnedRow`), so a refresh never flips object identity
    // and never remounts the cards (or restarts the usage read) over
    // unchanged data.
    const apiKey = createPinnedRow(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
        isSameApiKeyRow,
    );

    return (
        <ScreenFrame
            read={read}
            row={apiKey}
            firstReadHeader={
                <PageHeader
                    title="API key"
                    backHref={hrefs.apiKeys()}
                    backLabel="API keys"
                />
            }
            notFound={
                <RowNotFound
                    kind="api_key"
                    headerTitle="API key"
                    createAction={{
                        href: hrefs.apiKeyNew(),
                        label: "Create API key",
                    }}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={row.name}
                        description={`Created ${formatTimestamp(row.created_at)}`}
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
                                <dd class="tnum mt-1">{formatTimestamp(row.created_at)}</dd>
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
        </ScreenFrame>
    );
}
