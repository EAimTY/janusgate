import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import { isCredentialExpired, nowMicros } from "/src/format";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import {
    credentialKindLabel,
    isOAuthProviderRow,
    upstreamFormatLabel,
} from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { AppLink } from "/src/ui/app-link";
import { Badge, type BadgeTone } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";

/**
 * The one readable statement of a provider's credential state. A static key
 * never expires; an OAuth login without a stored expiry was never fresh, and
 * one past its expiry reconnects before it serves again.
 */
export function credentialBadge(provider: ProviderResponse): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    if (!isOAuthProviderRow(provider)) {
        return {
            tone: "muted",
            label: "Static key",
        };
    }

    if (provider.expires_at === null) {
        return {
            tone: "warn",
            label: "Stale",
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
 * One provider's row: its name, its kind, its model count, and its two
 * states. The row is one link to the provider's own page, where its models
 * and usage live and the edit page behind it owns every change.
 */
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
                        {`${upstreamFormatLabel(props.provider.format)} · ${credentialKindLabel(props.provider.type)} · ${props.modelCount} ${props.modelCount === 1 ? "model" : "models"}`}
                    </span>
                </span>
                <span class="flex items-center gap-2">
                    <Badge
                        tone={props.provider.is_enabled ? "ok" : "muted"}
                        label={props.provider.is_enabled ? "Enabled" : "Disabled"}
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
