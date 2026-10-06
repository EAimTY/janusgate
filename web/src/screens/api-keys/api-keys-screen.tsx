import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { readApiKeys } from "/src/features/inventory/read";
import { formatTimestamp } from "/src/format";
import { translation } from "/src/i18n";
import { keyCountMessage, noKeysMatchMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { createScreenRead, readOne } from "/src/screen-state/read";
import { CopyButton } from "/src/ui/clipboard";
import { ButtonLink, listingNeedle, SearchInput } from "/src/ui/controls";
import { Badge, EmptyState, Notice } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { ReadNotice } from "/src/ui/read-notice";
import { RowLink } from "/src/ui/row-link";
import { SkeletonRows } from "/src/ui/skeleton-rows";

/**
 * Every serving key on one page. Rows carry identity, the full secret with a
 * copy action, serving state, and creation time — each row opens the key's
 * detail page, behind which its edit page owns every change. Nothing is
 * edited here.
 */
export function ApiKeysScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [keys, setKeys] = createSignal<ReadonlyArray<ApiKeyResponse>>([]);
    const [search, setSearch] = createSignal("");

    const read = createScreenRead({
        load: async (signal) => {
            const keysResult = await readOne(signal, readApiKeys(props.api));

            setKeys(keysResult.api_keys);
        },
    });

    const visibleKeys = createMemo(() => {
        const needle = listingNeedle(search());

        if (needle === "") {
            return keys();
        }

        return keys().filter(
            (row) =>
                row.name.toLowerCase().includes(needle) || row.key.toLowerCase().includes(needle),
        );
    });

    // The count line's numerator counts the enabled rows among the visible
    // ones while its denominator is always the whole listing — two facts that
    // diverge under a standing search, where the sentence shape alone would
    // read as visible over total, the meaning the models screen's identical
    // "X of Y models" carries. A standing search therefore states both facts
    // ("2 keys shown · 1 of 5 enabled"); with no search the visible rows are
    // the whole listing, and the plain sentence holds.
    const visibleEnabledCount = createMemo(
        () => visibleKeys().filter((row) => row.is_enabled).length,
    );

    const countLine = createMemo(() =>
        keyCountMessage({
            total: keys().length,
            shown: visibleKeys().length,
            enabled: visibleEnabledCount(),
            hasSearch: listingNeedle(search()).length > 0,
        }),
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.apiKeysScreen.apiKeys)}
                description={translation()(
                    ($) =>
                        $.screens.apiKeysScreen
                            .servingKeysOpenAIClientsSendOneAsAuthorizationMessage,
                )}
            >
                <ButtonLink
                    href={hrefs.apiKeyNew()}
                    tone="primary"
                    glyph="plus"
                >
                    {translation()(($) => $.screens.apiKeysScreen.createAPIKey)}
                </ButtonLink>
            </PageHeader>

            <ReadNotice
                read={read}
                retry={read.reload}
            />

            <SkeletonRows
                read={read}
                rowCount={3}
                rowHeight="h-16"
            />

            <Show when={read.hasRead()}>
                <Show
                    when={keys().length > 0}
                    fallback={
                        <EmptyState
                            glyph="key"
                            title={translation()(($) => $.screens.apiKeysScreen.noAPIKeysYet)}
                            description={translation()(
                                ($) =>
                                    $.screens.apiKeysScreen
                                        .aFreshGatewayServesNothingUntilTheFirstMessage,
                            )}
                        >
                            <ButtonLink
                                href={hrefs.apiKeyNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                {translation()(($) => $.screens.apiKeysScreen.createAPIKey)}
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <SearchInput
                        label={translation()(($) => $.screens.apiKeysScreen.searchAPIKeys)}
                        placeholder={translation()(
                            ($) => $.screens.apiKeysScreen.searchByNameOrKey,
                        )}
                        value={search()}
                        onInput={setSearch}
                        class="sm:max-w-xs"
                    />

                    <Show
                        when={visibleKeys().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={noKeysMatchMessage(search().trim())}
                            />
                        }
                    >
                        <p
                            class="text-[12.5px] text-ink-faint"
                            role="status"
                        >
                            {countLine()}
                        </p>
                        <ul class="flex flex-col gap-2">
                            <For each={visibleKeys()}>{(row) => <ApiKeyRow apiKey={row} />}</For>
                        </ul>
                    </Show>
                </Show>
            </Show>
        </div>
    );
}

function ApiKeyRow(props: { readonly apiKey: ApiKeyResponse }): JSX.Element {
    return (
        <RowLink
            href={hrefs.apiKeyDetail(props.apiKey.id)}
            primary={
                <span class="block truncate text-[14.5px] font-semibold">{props.apiKey.name}</span>
            }
            secondary={translation()(($) => $.domainMessages.created, {
                instant: formatTimestamp(props.apiKey.created_at),
            })}
            tertiary={props.apiKey.key}
            badges={
                <Badge
                    tone={props.apiKey.is_enabled ? "ok" : "muted"}
                    label={
                        props.apiKey.is_enabled
                            ? translation()(($) => $.additional.apiKeysScreen.on)
                            : translation()(($) => $.additional.apiKeysScreen.off)
                    }
                />
            }
            trailing={
                <CopyButton
                    value={props.apiKey.key}
                    subject={translation()(($) => $.screens.apiKeysScreen.theSecretHere)}
                />
            }
        />
    );
}
