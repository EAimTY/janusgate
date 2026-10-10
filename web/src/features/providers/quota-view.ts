import { type Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { GetProviderQuotaResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { type ScreenNotice, screenFailureNotice } from "/src/screen-state/notices";

/** One mounted identity's whole observation and independently owned failure. */
type QuotaViewState =
    | {
          readonly type: "checking";
      }
    | {
          readonly type: "unobserved";
      }
    | {
          readonly type: "unsupported";
      }
    | {
          readonly type: "loading";
      }
    | {
          readonly type: "failed";
          readonly error: ScreenNotice;
      }
    | {
          readonly type: "ready";
          readonly snapshot: GetProviderQuotaResponse;
          readonly refresh:
              | {
                    readonly type: "idle";
                }
              | {
                    readonly type: "loading";
                }
              | {
                    readonly type: "failed";
                    readonly error: ScreenNotice;
                };
      };

/** Quota state lives outside keyed detail-card renders, so ordinary row reads never poll it. */
export type ProviderQuotaView = {
    readonly state: Accessor<QuotaViewState>;
    readonly refresh: () => void;
};

/** The readable identity; credential mutations invalidate by leaving and remounting the detail screen. */
type Identity = {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
};

function isSameIdentity(before: Identity, after: Identity): boolean {
    return (
        before.api === after.api &&
        before.provider.id === after.provider.id &&
        before.provider.provider_definition_name === after.provider.provider_definition_name &&
        before.provider.provider_definition_version ===
            after.provider.provider_definition_version &&
        before.provider.base_url === after.provider.base_url
    );
}

/** Resolves exact-version capability without upstream I/O; every quota query is explicit. */
export function createProviderQuotaView(props: {
    readonly api: Accessor<ManagementApi>;
    readonly provider: Accessor<ProviderResponse | null>;
    /**
     * Called once a quota observation succeeds (`web/docs/architecture.md`): the
     * observation reads independently of every other card, so the consumer
     * may stamp the shell's freshness (`web/docs/architecture.md`) but must never re-run another
     * read. A failed or superseded observation never calls it.
     */
    readonly onQuotaReadComplete?: () => void;
}): ProviderQuotaView {
    const [state, setState] = createSignal<QuotaViewState>({
        type: "checking",
    });
    let identity: Identity | null = null;
    let controller: AbortController | null = null;
    let generation = 0;
    let isBusy = false;
    let isDisposed = false;
    let hasResolvedCapability = false;

    const run = async (shouldReadQuota: boolean): Promise<void> => {
        if (identity === null || isBusy || isDisposed) return;
        const current = identity;
        const currentGeneration = generation;
        const call = new AbortController();
        controller = call;
        isBusy = true;
        const before = state();
        const snapshot = before.type === "ready" ? before.snapshot : null;
        setState(
            snapshot === null
                ? {
                      type: shouldReadQuota ? "loading" : "checking",
                  }
                : {
                      type: "ready",
                      snapshot,
                      refresh: {
                          type: "loading",
                      },
                  },
        );

        const outcome = await performManagementCall(async () => {
            if (!hasResolvedCapability) {
                const catalog = await current.api.listProviderDefinitions(call.signal);
                call.signal.throwIfAborted();
                const definition = catalog.provider_definitions.find(
                    (candidate) =>
                        candidate.name === current.provider.provider_definition_name &&
                        candidate.version === current.provider.provider_definition_version,
                );
                if (definition === undefined)
                    throw new Error("The bound provider definition is missing from the catalog.");
                if (definition.quota == null) return null;
                // This immutable version's capability can be reused by manual refreshes.
                hasResolvedCapability = true;
            }
            if (!shouldReadQuota) return undefined;
            return current.api.getProviderQuota(
                {
                    provider_id: current.provider.id,
                },
                call.signal,
            );
        });

        // Cancellation is also enforced here for transports that cannot abort or that ignore the
        // signal.
        if (isDisposed || call.signal.aborted || generation !== currentGeneration) return;
        isBusy = false;
        if (outcome.status === "aborted") return;
        if (outcome.status === "failed") {
            const error = screenFailureNotice(outcome.cause);
            setState(
                snapshot === null
                    ? {
                          type: "failed",
                          error,
                      }
                    : {
                          type: "ready",
                          snapshot,
                          refresh: {
                              type: "failed",
                              error,
                          },
                      },
            );
            return;
        }
        setState(
            outcome.result === null
                ? {
                      type: "unsupported",
                  }
                : outcome.result === undefined
                  ? {
                        type: "unobserved",
                    }
                  : {
                        type: "ready",
                        snapshot: outcome.result,
                        refresh: {
                            type: "idle",
                        },
                    },
        );
        // A completed read is the card's own business, but a succeeded quota
        // observation is a successful read of gateway-visible state: the
        // consumer may stamp the shell's freshness with it (`web/docs/architecture.md`). A
        // failed observation states its diagnosis here and never reports — a
        // failure re-runs nothing and stamps nothing.
        if (shouldReadQuota && hasResolvedCapability) props.onQuotaReadComplete?.();
    };

    createEffect(
        () => ({
            api: props.api(),
            provider: props.provider(),
        }),
        (next) => {
            if (
                next.provider !== null &&
                identity !== null &&
                isSameIdentity(identity, {
                    api: next.api,
                    provider: next.provider,
                })
            )
                return;
            controller?.abort();
            generation += 1;
            isBusy = false;
            hasResolvedCapability = false;
            identity =
                next.provider === null
                    ? null
                    : {
                          api: next.api,
                          provider: next.provider,
                      };
            setState({
                type: "checking",
            });
            void run(false);
        },
    );
    onCleanup(() => {
        isDisposed = true;
        generation += 1;
        controller?.abort();
    });
    return {
        state,
        refresh: () => {
            if (state().type !== "unsupported") void run(true);
        },
    };
}
