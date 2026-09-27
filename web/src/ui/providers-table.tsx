import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { isCredentialExpired, nowMicros } from "/src/format";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import {
    credentialKindLabel,
    isOAuthProviderRow,
    upstreamFormatLabel,
} from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { Badge, type BadgeTone } from "/src/ui/feedback";
import { RowLink } from "/src/ui/row-link";

/**
 * A credential badge's words, plus the native tooltip where the badge's one
 * word understates the state it names.
 */
type CredentialBadge = {
    readonly tone: BadgeTone;
    readonly label: string;
    readonly title?: string | undefined;
};

/**
 * The one glance at a credential that can go stale: a fresh OAuth login reads
 * `Valid`, one past its expiry `Expired`, and one with no stored expiry was
 * never fresh — `Stale`, titled with the sentence the provider's page states,
 * since a bare word nowhere explains itself. A static key never expires, and
 * the row's own line already names its kind, so it earns no credential badge
 * and this answers `null` for it.
 */
export function credentialBadge(provider: ProviderResponse): CredentialBadge | null {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return {
            tone: "warn",
            label: "Stale",
            title: "The OAuth credentials were never fresh.",
        };
    }

    return isCredentialExpired(provider.expires_at, nowMicros())
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
                secondary={`${upstreamFormatLabel(props.provider.format)} · ${credentialKindLabel(props.provider.type)} · ${props.modelCount} ${props.modelCount === 1 ? "model" : "models"}`}
                badges={
                    <>
                        <Badge
                            tone={props.provider.is_enabled ? "ok" : "muted"}
                            label={props.provider.is_enabled ? "On" : "Off"}
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
