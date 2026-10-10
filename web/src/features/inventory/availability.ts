import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { isModelInventoryReady } from "/src/management-api/serving";

/**
 * The gateway's structural serving ladder for one account-model pair (root
 * `web/docs/architecture.md`'s three-input effective serving state): the account's existence
 * and switch, then the model's existence and switch, then the row's
 * discovery state. Independent of quota restrictions and of any pool or
 * route switch authored above the pair.
 */
export type StructuralAvailability =
    | "account_missing"
    | "account_disabled"
    | "model_missing"
    | "model_disabled"
    | "model_vanished"
    | "ready";

/**
 * Classifies one account-model pair down the shared structural ladder. The
 * account rungs stand first, so a disabled account classifies before the
 * model row is even consulted, and `undefined` answers the join's absence
 * verdicts — an account or model row the caller's listing does not hold.
 * The terminal rung answers through the contract's own
 * `isModelInventoryReady` verdict (`docs/design/storage.md`'s effective-serving rule),
 * so the three-input conjunction is spelled once, in the contract layer.
 * `src/features/provider-pools/pool-availability.ts` and the API-key access
 * inventory's structural status render their own labels over this one
 * classification, so the ladder is spelled exactly once.
 */
export function classifyStructuralAvailability(
    provider: ProviderResponse | undefined,
    model: ModelResponse | undefined,
): StructuralAvailability {
    if (provider === undefined) return "account_missing";
    if (!provider.is_enabled) return "account_disabled";
    if (model === undefined) return "model_missing";
    if (!model.is_enabled) return "model_disabled";
    return isModelInventoryReady(model, provider) ? "ready" : "model_vanished";
}
