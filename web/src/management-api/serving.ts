import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";

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
