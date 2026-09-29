import { isCredentialExpired, nowMicros } from "/src/format";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";

/**
 * The serving derivations the wire contract leaves to the client: rule 7
 * (§4.5 *Responses carry only what the caller cannot derive*) ships the
 * model row's inputs — never the three-input verdict over them — and the
 * provider row's credential columns — never the standing over them — so the
 * verdicts and the counts taken over the first are computed here, under the
 * gateway's own definitions (root README §3). The generic row indexing these
 * derivations' callers share lives outside the contract, in `/src/row-index`.
 */

/**
 * Counts the listed models per provider row id, once per listings change,
 * through the one accumulation every inclusion shares. The optional predicate
 * states which rows count; without one, every row does. The providers page
 * states the raw row count on every row through this one map instead of
 * rebuilding the same counts.
 */
export function modelCountsByProvider(
    models: ReadonlyArray<ModelResponse>,
    isIncluded?: (model: ModelResponse) => boolean,
): ReadonlyMap<number, number> {
    const counts = new Map<number, number>();

    for (const model of models) {
        if (isIncluded === undefined || isIncluded(model)) {
            counts.set(model.provider_id, (counts.get(model.provider_id) ?? 0) + 1);
        }
    }

    return counts;
}

/**
 * Counts the listed models that are effectively serving (README §3) per
 * provider row id, once per listings change: `modelCountsByProvider` with
 * `isModelEffectivelyServing` as the inclusion predicate. The overview's
 * attention lines read this map: an enabled provider whose count is zero
 * serves nothing — its every row disabled, vanished, or absent — which is the
 * quietest way a provider fails and the one the overview must still name.
 */
export function servingModelCountsByProvider(
    models: ReadonlyArray<ModelResponse>,
    providersById: ReadonlyMap<number, ProviderResponse>,
): ReadonlyMap<number, number> {
    return modelCountsByProvider(models, (model) =>
        isModelEffectivelyServing(model, providersById.get(model.provider_id)),
    );
}

/**
 * The gateway's three-input effective serving definition (README §3),
 * derived client-side from rows the app already fetches and never read off
 * the wire (rule 7): the provider's switch, the row's own switch, and a
 * discovery state that is not `vanished`. A vanished row can carry
 * `is_enabled: true` and still not serve.
 */
export function isModelEffectivelyServing(
    model: ModelResponse,
    provider: ProviderResponse | undefined,
): boolean {
    return (
        (provider?.is_enabled ?? false) && model.is_enabled && model.discovery_state !== "vanished"
    );
}

/**
 * A provider row's credential standing — the one verdict every credential
 * presentation branches on: `"valid"` while a stored OAuth expiry still
 * holds, `"expired"` once it has passed, `"never-fresh"` when an OAuth row
 * stores no expiry at all, and `null` for a static-key row, which never
 * expires and so has no credential standing to state.
 */
export type CredentialStanding = "valid" | "expired" | "never-fresh" | null;

/**
 * Derives a provider row's credential standing from its own columns — the
 * single home of the verdict the providers table's credential badge, the
 * provider page's reconnect notice, and the overview's lapsed-credential
 * attention each branch on, so the three presentations cannot drift. An
 * OAuth row with no stored expiry was never fresh, an expiry already past
 * at the reading moment is expired, one still ahead is valid, and a static
 * key — which never expires — states no standing at all.
 */
export function credentialStanding(provider: ProviderResponse): CredentialStanding {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    if (provider.expires_at === null) {
        return "never-fresh";
    }

    return isCredentialExpired(provider.expires_at, nowMicros()) ? "expired" : "valid";
}

/**
 * Whether a credential standing states a credential the gateway cannot use as
 * stored — the "needs a human" verdict over the standing: `"expired"` once
 * the stored OAuth expiry has passed, or `"never-fresh"` when an OAuth row
 * stores no expiry at all. A `"valid"` login is not lapsed, and a static
 * key — which never expires and states no standing at all — never is. The
 * internal disjunction the exported row-level verdict
 * `lapsedCredentialStanding` composes over `credentialStanding`'s
 * derivation.
 */
function isLapsedCredentialStanding(
    standing: CredentialStanding,
): standing is "expired" | "never-fresh" {
    return standing === "expired" || standing === "never-fresh";
}

/**
 * A provider row's credential standing when it names a credential the gateway
 * cannot use as stored — the "needs a human" verdict: `"expired"` once the
 * stored OAuth expiry has passed, or `"never-fresh"` when an OAuth row stores
 * no expiry at all — and `null` when there is nothing to show: a valid OAuth
 * login needs no human, and a static key, which never expires, states no
 * standing at all. The composition of `credentialStanding` with
 * `isLapsedCredentialStanding` over the row itself, hoisted beside its two
 * pieces so the overview's lapsed-credential attention, the provider page's
 * reconnect notice, and the edit page's reconnect offer state one verdict
 * instead of spelling the composition three times.
 */
export function lapsedCredentialStanding(
    provider: ProviderResponse,
): "expired" | "never-fresh" | null {
    const standing = credentialStanding(provider);

    return isLapsedCredentialStanding(standing) ? standing : null;
}
