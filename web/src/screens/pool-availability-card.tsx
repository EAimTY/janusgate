import type { JSX } from "@solidjs/web";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { formatTimestamp, nowMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    GetProviderAvailabilityResponse,
    LastRefresh,
    ModelResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { Button } from "/src/ui/controls";
import { Card, CardHeader } from "/src/ui/layout";
import { deriveMemberAvailability, type InventoryAvailability } from "./pool-availability";
import {
    createDisposalGuard,
    createScreenRead,
    ReadCallFailure,
    type ScreenNotice,
    screenFailureNotice,
} from "./read";
import { ReadNotice, StatedNotice } from "./read-notice";

const INVENTORY_LABELS: Record<InventoryAvailability, string> = {
    pool_disabled: "Pool disabled",
    account_missing: "Account missing",
    account_disabled: "Account disabled",
    model_not_selected: "Choose an upstream ID to inspect model availability",
    model_missing: "Model missing",
    model_disabled: "Model disabled",
    model_vanished: "Model vanished",
    ready: "Inventory ready",
};

function refreshText(refresh: LastRefresh | null): string {
    if (refresh === null) return "Reading stored availability…";
    switch (refresh.type) {
        case "not_observed":
            return "Quota not observed";
        case "succeeded":
            return `Quota observed ${formatTimestamp(refresh.observed_at)}`;
        case "failed":
            return `Quota read failed · ${refresh.reason.replaceAll("_", " ")}`;
        case "unsupported":
            return "Quota reads unsupported";
        case "reset":
            return "Administrator reset; refresh grace installed";
    }
}

/** Reads durable restrictions independently of structural inventory and explicit upstream actions. */
export function AccountAvailabilityRow(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
    readonly provider: ProviderResponse | undefined;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly isPoolEnabled: boolean;
    readonly upstreamId: string | null;
    readonly priority?: number;
    readonly isAccountInspection?: boolean;
    readonly canRefreshQuota?: boolean;
}): JSX.Element {
    const [observation, setObservation] = createSignal<GetProviderAvailabilityResponse | null>(
        null,
    );
    const [actionNotice, setActionNotice] = createSignal<ScreenNotice | null>(null);
    const [isBusy, setIsBusy] = createSignal(false);
    const isDisposed = createDisposalGuard();
    let generation = 0;
    let actionController: AbortController | null = null;
    onCleanup(() => actionController?.abort());
    const read = createScreenRead({
        load: async (signal) => {
            const capturedGeneration = ++generation;
            let value: GetProviderAvailabilityResponse;
            try {
                value = await props.api.getProviderAvailability(
                    {
                        provider_id: props.providerId,
                    },
                    signal,
                );
            } catch (error) {
                throw new ReadCallFailure(error);
            }
            if (!signal.aborted && !isDisposed() && capturedGeneration === generation)
                setObservation(value);
        },
    });
    const availability = () =>
        deriveMemberAvailability(
            props.isPoolEnabled,
            props.provider,
            props.models,
            props.upstreamId,
            observation(),
        );
    const blocks = () =>
        props.isAccountInspection ? (observation()?.blocks ?? []) : availability().blocks;
    const act = async (shouldReset: boolean): Promise<void> => {
        if (isBusy() || isDisposed()) return;
        setIsBusy(true);
        setActionNotice(null);
        ++generation;
        const controller = new AbortController();
        actionController = controller;
        try {
            if (shouldReset)
                await props.api.resetProviderAvailability(
                    {
                        provider_id: props.providerId,
                    },
                    controller.signal,
                );
            else
                await props.api.getProviderQuota(
                    {
                        provider_id: props.providerId,
                    },
                    controller.signal,
                );
        } catch (error) {
            if (!controller.signal.aborted && !isDisposed())
                setActionNotice(screenFailureNotice(error));
        }
        // A failed quota query still updates safe refresh metadata. Read it back without dropping prior blocks.
        if (!controller.signal.aborted && !isDisposed()) {
            const capturedGeneration = ++generation;
            try {
                const value = await props.api.getProviderAvailability(
                    {
                        provider_id: props.providerId,
                    },
                    controller.signal,
                );
                if (
                    !controller.signal.aborted &&
                    !isDisposed() &&
                    capturedGeneration === generation
                )
                    setObservation(value);
            } catch (error) {
                if (!controller.signal.aborted && !isDisposed())
                    setActionNotice(screenFailureNotice(error));
            }
        }
        if (!isDisposed()) setIsBusy(false);
        actionController = null;
    };
    return (
        <li class="py-4 first:pt-0 last:pb-0">
            <div class="flex flex-wrap items-start justify-between gap-3">
                <div class="min-w-0 break-words">
                    <p class="font-medium">
                        {props.provider?.name ?? `Deleted account #${props.providerId}`}
                    </p>
                    <p class="mt-1 text-ink-dim text-[12.5px]">
                        {props.priority !== undefined ? `Priority ${props.priority} · ` : ""}
                        {props.isAccountInspection &&
                        availability().inventory === "model_not_selected"
                            ? "Account enabled"
                            : INVENTORY_LABELS[availability().inventory]}
                    </p>
                </div>
                <div class="flex flex-wrap gap-2">
                    <Show when={props.canRefreshQuota !== false}>
                        <Button
                            isDisabled={isBusy() || props.provider === undefined}
                            onClick={() => void act(false)}
                        >
                            Refresh quota
                        </Button>
                    </Show>
                    <Button
                        isDisabled={isBusy() || props.provider === undefined}
                        onClick={() => void act(true)}
                    >
                        Reset restrictions
                    </Button>
                </div>
            </div>
            <ReadNotice
                read={read}
                retry={read.reload}
            />
            <StatedNotice
                notice={actionNotice}
                class="mt-3"
            />
            <p
                role="status"
                class="mt-3 text-[12.5px] text-ink-dim"
            >
                {isBusy() ? "Updating availability…" : refreshText(availability().refresh)}
            </p>
            <Show when={blocks().length > 0}>
                <p class="mt-3 font-medium">Quota exhausted</p>
                <ul class="mt-2 space-y-2 text-[12.5px] text-ink-dim">
                    <For each={blocks()}>
                        {(block) => (
                            <li class="break-words">
                                {block.key.join(" / ")} ·{" "}
                                {block.applies_to.type === "all_models"
                                    ? "All models"
                                    : `Models: ${block.applies_to.upstream_ids.join(", ")}`}{" "}
                                ·{" "}
                                {block.blocked_until === null
                                    ? "Reset unknown"
                                    : block.blocked_until <= nowMicros()
                                      ? "Reset time passed; refresh required"
                                      : `Resets ${formatTimestamp(block.blocked_until)}`}
                            </li>
                        )}
                    </For>
                </ul>
            </Show>
        </li>
    );
}

/** Stored member observations never trigger upstream quota reads on page load. */
export function PoolAvailabilityCard(props: {
    readonly api: ManagementApi;
    readonly pool: ProviderPoolResponse;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly upstreamId: string | null;
}): JSX.Element {
    return (
        <Card>
            <CardHeader
                title="Pool availability"
                description="Stored observations. Refresh explicitly queries the upstream account. Reset clears restrictions and installs refresh grace; it does not reset upstream quota."
            />
            <Show
                when={props.pool.members.length > 0}
                fallback={<p class="mt-4 text-ink-dim">This pool has no members.</p>}
            >
                <ul class="mt-4 divide-y divide-rule">
                    <For each={props.pool.members}>
                        {(member) => (
                            <AccountAvailabilityRow
                                api={props.api}
                                providerId={member.provider_id}
                                provider={props.providers.find(
                                    (provider) => provider.id === member.provider_id,
                                )}
                                models={props.models}
                                isPoolEnabled={props.pool.is_enabled}
                                upstreamId={props.upstreamId}
                                priority={member.priority}
                            />
                        )}
                    </For>
                </ul>
            </Show>
        </Card>
    );
}
