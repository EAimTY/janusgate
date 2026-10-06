import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { CREDENTIAL_KIND, UPSTREAM_FORMAT } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { Badge } from "/src/ui/feedback";
import { RowLink } from "/src/ui/row-link";
import type { CredentialBadge } from "./credential-presentation";
import { credentialBadge } from "./credential-presentation";

/**
 * One provider's row: its name, its kind, its model count, and its enabled
 * state. The row is one link to the provider's own page, where its models
 * and usage live and the edit page behind it owns every change.
 */
function ProviderRow(props: {
    readonly provider: ProviderResponse;
    readonly modelCount: number;
}): JSX.Element {
    const credential = (): CredentialBadge | null => credentialBadge(props.provider);

    return (
        <li>
            <RowLink
                href={hrefs.providerDetail(props.provider.id)}
                primary={
                    <span class="block truncate text-[14.5px] font-semibold">
                        {props.provider.name}
                    </span>
                }
                secondary={`${UPSTREAM_FORMAT.label(props.provider.format)} · ${CREDENTIAL_KIND.label(props.provider.type)} · ${translation()(
                    ($) => $.counts.models,
                    {
                        count: props.modelCount,
                        total: formatNumber(props.modelCount),
                    },
                )}`}
                badges={
                    <>
                        <Badge
                            tone={props.provider.is_enabled ? "ok" : "muted"}
                            label={
                                props.provider.is_enabled
                                    ? translation()(($) => $.helpers.providersTable.on)
                                    : translation()(($) => $.helpers.providersTable.off)
                            }
                        />
                        <Show when={credential()}>
                            {(current) => (
                                <Badge
                                    tone={current().tone}
                                    label={current().label}
                                    title={current().title}
                                />
                            )}
                        </Show>
                    </>
                }
            />
        </li>
    );
}

/** A list of provider rows, ordered by name as the gateway returns them. */
export function ProviderList(props: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly modelCounts: ReadonlyMap<number, number>;
}): JSX.Element {
    return (
        <ul class="flex flex-col gap-2">
            <For each={props.providers}>
                {(provider) => (
                    <ProviderRow
                        provider={provider}
                        modelCount={props.modelCounts.get(provider.id) ?? 0}
                    />
                )}
            </For>
        </ul>
    );
}
