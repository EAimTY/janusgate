import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { type CredentialStanding, credentialStanding } from "/src/management-api/serving";
import { CREDENTIAL_KIND, UPSTREAM_FORMAT } from "/src/management-api/vocabulary";
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
 * The never-fresh standing's verdict sentence, named once beside the badge
 * record that renders it as the "Stale" badge's tooltip: the provider page's
 * lapsed-credential notice states the same words instead of its own spelling.
 * The short per-surface labels ("Stale" on the badge, "Credential not fresh"
 * on the overview) stay each surface's own.
 */
export function oauthNeverFreshSentence(): string {
    return translation()(($) => $.remainingText.theOAuthCredentialsWereNeverFresh);
}

/**
 * The never-fresh standing's stored-fact sentence — no OAuth expiry column
 * is stored — named once so the overview's attention detail and the edit
 * page's Credentials card state the same words instead of two spellings.
 */
export function noStoredExpirySentence(): string {
    return translation()(($) => $.dynamicMessages.noExpiry);
}

/**
 * The badge each non-null credential standing is presented under. The record
 * type is exhaustive, so a standing added to `serving.ts` fails to compile
 * until it is listed here.
 */
const CREDENTIAL_STANDING_BADGES: Record<Exclude<CredentialStanding, null>, CredentialBadge> = {
    valid: {
        tone: "ok",
        get label() {
            return translation()(($) => $.helpers.providersTable.valid);
        },
    },
    expired: {
        tone: "warn",
        get label() {
            return translation()(($) => $.helpers.providersTable.expired);
        },
    },
    "never-fresh": {
        tone: "warn",
        get label() {
            return translation()(($) => $.helpers.providersTable.stale);
        },
        get title() {
            return oauthNeverFreshSentence();
        },
    },
};

/**
 * The one glance at a credential that can go stale, read off `serving.ts`'s
 * single standing derivation: a fresh OAuth login reads `Valid`, one past its
 * expiry `Expired`, and one with no stored expiry was never fresh — `Stale`,
 * titled with the sentence the provider's page states, since a bare word
 * nowhere explains itself. A static key never expires, and the row's own line
 * already names its kind, so it earns no credential badge and this answers
 * `null` for it.
 */
export function credentialBadge(provider: ProviderResponse): CredentialBadge | null {
    const standing = credentialStanding(provider);

    return standing === null ? null : CREDENTIAL_STANDING_BADGES[standing];
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
                                    ? translation()(($) => $.additional.providersTable.on)
                                    : translation()(($) => $.additional.providersTable.off)
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
