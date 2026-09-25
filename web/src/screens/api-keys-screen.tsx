import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { createScreenRead, readAll } from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { AppLink } from "/src/ui/app-link";
import { CopyButton } from "/src/ui/clipboard";
import { ButtonLink, SearchInput } from "/src/ui/controls";
import { Badge, EmptyState, Notice } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { PageHeader } from "/src/ui/layout";
import { SkeletonRows } from "./skeleton-rows";

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
            const [keysResult] = await readAll(signal, {
                run: (cycle) => props.api.listApiKeys(cycle),
            });

            setKeys(keysResult.api_keys);
        },
    });

    const visibleKeys = createMemo(() => {
        const needle = search().trim().toLowerCase();

        if (needle === "") {
            return keys();
        }

        return keys().filter(
            (row) =>
                row.name.toLowerCase().includes(needle) || row.key.toLowerCase().includes(needle),
        );
    });

    const enabledCount = createMemo(() => keys().filter((row) => row.is_enabled).length);

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="API keys"
                description="Serving keys. Clients send one as Authorization: Bearer <key>; open one for its secret and usage."
            >
                <ButtonLink
                    href={hrefs.apiKeyNew()}
                    tone="primary"
                    glyph="plus"
                >
                    Create API key
                </ButtonLink>
            </PageHeader>

            <ReadNotice read={read} />

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
                            title="No API keys yet"
                            description="A fresh gateway serves nothing until the first key exists."
                        >
                            <ButtonLink
                                href={hrefs.apiKeyNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                Create API key
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <SearchInput
                        label="Search API keys"
                        placeholder="Search by name or key"
                        value={search()}
                        onInput={setSearch}
                        class="sm:max-w-xs"
                    />

                    <p
                        class="text-[12.5px] text-ink-faint"
                        role="status"
                    >
                        {`${enabledCount()} of ${keys().length} ${keys().length === 1 ? "key" : "keys"} enabled`}
                    </p>

                    <Show
                        when={visibleKeys().length > 0}
                        fallback={
                            <Notice
                                tone="info"
                                title={`No API keys match "${search().trim()}".`}
                            />
                        }
                    >
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
        <li class="flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3.5">
            <AppLink
                href={hrefs.apiKeyDetail(props.apiKey.id)}
                class="group flex min-w-0 flex-1 basis-48 items-center gap-4"
            >
                <span class="min-w-0 flex-1">
                    <span class="block truncate text-[14.5px] font-semibold">
                        {props.apiKey.name}
                    </span>
                    <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                        {`Created ${formatTimestamp(props.apiKey.created_at)}`}
                    </span>
                    <span class="tnum mt-1 block truncate text-[12.5px] text-ink-dim">
                        {props.apiKey.key}
                    </span>
                </span>
                <span class="flex shrink-0 items-center gap-2">
                    <Badge
                        tone={props.apiKey.is_enabled ? "ok" : "muted"}
                        label={props.apiKey.is_enabled ? "On" : "Off"}
                    />
                    <Glyph
                        name="chev-right"
                        class="text-ink-faint transition-transform duration-150 ease-out group-hover:translate-x-0.5 group-hover:text-ink-dim"
                    />
                </span>
            </AppLink>
            <CopyButton
                value={props.apiKey.key}
                subject="the secret here"
            />
        </li>
    );
}
