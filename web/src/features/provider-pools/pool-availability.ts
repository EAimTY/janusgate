import type {
    GetProviderAvailabilityResponse,
    LastRefresh,
    ModelResponse,
    ProviderResponse,
    QuotaBlock,
} from "/src/management-api/generated/v0";

/** Structural state is independent of observed quota restrictions. */
export type InventoryAvailability =
    | "pool_disabled"
    | "account_missing"
    | "account_disabled"
    | "model_not_selected"
    | "model_missing"
    | "model_disabled"
    | "model_vanished"
    | "ready";

/** A read-only join never clears a block because its reset time has passed. */
export function deriveMemberAvailability(
    isPoolEnabled: boolean,
    provider: ProviderResponse | undefined,
    models: ReadonlyArray<ModelResponse>,
    upstreamId: string | null,
    observation: GetProviderAvailabilityResponse | null,
): {
    readonly inventory: InventoryAvailability;
    readonly blocks: ReadonlyArray<QuotaBlock>;
    readonly refresh: LastRefresh | null;
} {
    const model =
        provider === undefined || upstreamId === null
            ? undefined
            : models.find(
                  (candidate) =>
                      candidate.provider_id === provider.id && candidate.upstream_id === upstreamId,
              );
    const inventory: InventoryAvailability = !isPoolEnabled
        ? "pool_disabled"
        : provider === undefined
          ? "account_missing"
          : !provider.is_enabled
            ? "account_disabled"
            : upstreamId === null
              ? "model_not_selected"
              : model === undefined
                ? "model_missing"
                : !model.is_enabled
                  ? "model_disabled"
                  : model.discovery_state === "vanished"
                    ? "model_vanished"
                    : "ready";
    const blocks =
        observation?.blocks.filter(
            (block) =>
                block.applies_to.type === "all_models" ||
                (upstreamId !== null && block.applies_to.upstream_ids.includes(upstreamId)),
        ) ?? [];
    return {
        inventory,
        blocks,
        refresh: observation?.last_refresh ?? null,
    };
}
