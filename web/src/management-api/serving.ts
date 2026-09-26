import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";

/**
 * Maps listing rows by their id, ready for O(1) lookups. Row ids are unique
 * by contract, so each id answers exactly one row.
 */
export function indexRowsById<
    TRow extends {
        readonly id: number;
    },
>(rows: ReadonlyArray<TRow>): ReadonlyMap<number, TRow> {
    return new Map(
        rows.map((row) => [
            row.id,
            row,
        ]),
    );
}

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
