import type { JSX } from "@solidjs/web";
import { createSignal } from "solid-js";
import type { AccessInventory } from "/src/access-control";
import { formatTimestamp } from "/src/format";
import { translation } from "/src/i18n";
import { keyEndpointMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import {
    createPinnedRow,
    createScreenRead,
    isSameApiKeyRow,
    readAll,
    readApiKeys,
    readPublishedInventory,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { ApiKeyAccess } from "/src/ui/api-key-access";
import { ApiKeySecretValue } from "/src/ui/api-key-form";
import { ButtonLink } from "/src/ui/controls";
import { Badge } from "/src/ui/feedback";
import { Card, CardHeader, Fact, FactList, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "./row-not-found";

/**
 * One serving key's page: its secret, its facts, and what serving clients
 * did with it. Nothing is changed here — the edit page owns every change.
 * Deleting a key revokes it and removes its model access rules; accounting remains.
 */
export function ApiKeyDetailScreen(props: {
    readonly api: ManagementApi;
    readonly apiKeyId: number;
}): JSX.Element {
    const [inventory, setInventory] = createSignal<AccessInventory | null>(null);
    const [keys, setKeys] = createSignal<ReadonlyArray<ApiKeyResponse>>([]);

    const read = createScreenRead({
        load: async (signal) => {
            const [keysResult, inventoryResult] = await readAll(
                signal,
                readApiKeys(props.api),
                readPublishedInventory(props.api, null),
            );
            setInventory(inventoryResult);

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
                    title={translation()(($) => $.screens.apiKeyDetailScreen.apiKey)}
                    backHref={hrefs.apiKeys()}
                    backLabel={translation()(($) => $.additional.apiKeyDetailScreen.apiKeys)}
                />
            }
            notFound={
                <RowNotFound
                    kind="api_key"
                    headerTitle={translation()(($) => $.additional.apiKeyDetailScreen.apiKey)}
                    createAction={{
                        href: hrefs.apiKeyNew(),
                        label: translation()(($) => $.additional.apiKeyDetailScreen.createAPIKey),
                    }}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={row.name}
                        description={translation()(($) => $.domainMessages.created, {
                            instant: formatTimestamp(row.created_at),
                        })}
                        backHref={hrefs.apiKeys()}
                        backLabel={translation()(($) => $.additional.apiKeyDetailScreen.apiKeys)}
                    >
                        <Badge
                            tone={row.is_enabled ? "ok" : "muted"}
                            label={
                                row.is_enabled
                                    ? translation()(($) => $.additional.apiKeyDetailScreen.enabled)
                                    : translation()(($) => $.additional.apiKeyDetailScreen.disabled)
                            }
                        />
                        <ButtonLink href={hrefs.apiKeyEdit(row.id)}>
                            {translation()(($) => $.screens.apiKeyDetailScreen.editAPIKey)}
                        </ButtonLink>
                    </PageHeader>

                    <Card>
                        <CardHeader
                            title={translation()(($) => $.screens.apiKeyDetailScreen.secret)}
                        />
                        <div class="mt-4">
                            <ApiKeySecretValue secret={row.key} />
                        </div>
                        {/* The first-run journey ends on a copied key, which
                            points nowhere by itself: the surface URLs a client
                            is aimed at live on the Overview's Endpoints card —
                            the fact the management API never states. */}
                        <p class="mt-3 text-[12.5px] text-ink-faint">
                            {keyEndpointMessage(hrefs.home())}
                        </p>
                    </Card>

                    <Card>
                        <CardHeader
                            title={translation()(($) => $.screens.apiKeyDetailScreen.details)}
                        />
                        <FactList>
                            <Fact
                                label={translation()(($) => $.screens.apiKeyDetailScreen.name)}
                                value={row.name}
                            />
                            <Fact
                                label={translation()(($) => $.screens.apiKeyDetailScreen.created)}
                                value={formatTimestamp(row.created_at)}
                                isTabular
                            />
                            {/* The one enabled pair this page speaks: the
                                Details card's Status echoes the header badge's
                                word exactly — the tight list-row "On/Off"
                                never appears on a detail page. */}
                            <Fact
                                label={translation()(($) => $.screens.apiKeyDetailScreen.status)}
                                value={
                                    row.is_enabled
                                        ? translation()(
                                              ($) => $.additional.apiKeyDetailScreen.enabled,
                                          )
                                        : translation()(
                                              ($) => $.additional.apiKeyDetailScreen.disabled,
                                          )
                                }
                            />
                        </FactList>
                    </Card>

                    {inventory() && (
                        <Card>
                            <ApiKeyAccess
                                policy={row.access_control}
                                inventory={
                                    inventory() ?? {
                                        providers: [],
                                        models: [],
                                        pools: [],
                                        modelRoutes: [],
                                    }
                                }
                            />
                            <div class="mt-4">
                                <ButtonLink href={hrefs.apiKeyEdit(row.id)}>
                                    {translation()(($) => $.screens.apiKeyDetailScreen.editAPIKey)}
                                </ButtonLink>
                            </div>
                        </Card>
                    )}
                    <EntityUsageCard
                        api={props.api}
                        filter={{
                            api_key_id: row.id,
                        }}
                        subjectKind="api key"
                    />
                </>
            )}
        </ScreenFrame>
    );
}
