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
 * Counts the listed models per provider row id, once per listings change.
 * The providers page states the count on every row and the overview derives
 * its "has no models" attention lines from it, so both read this one map
 * instead of each rebuilding the same counts.
 */
export function modelCountsByProvider(
    models: ReadonlyArray<ModelResponse>,
): ReadonlyMap<number, number> {
    const counts = new Map<number, number>();

    for (const model of models) {
        counts.set(model.provider_id, (counts.get(model.provider_id) ?? 0) + 1);
    }

    return counts;
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
