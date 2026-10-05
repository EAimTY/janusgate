import { translation } from "/src/i18n";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import type { CredentialStanding } from "/src/management-api/serving";
import { credentialStanding } from "/src/management-api/serving";
import type { BadgeTone } from "/src/ui/feedback";

/**
 * A credential badge's words, plus the native tooltip where the badge's one
 * word understates the state it names.
 */
export type CredentialBadge = {
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
