import type { JSX } from "@solidjs/web";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { formatTimestamp, nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { quotaFactMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    GetProviderAvailabilityResponse,
    LastRefresh,
    ModelResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { createDisposalGuard } from "/src/screen-state/lifetime";
import { type ScreenNotice, screenFailureNotice } from "/src/screen-state/notices";
import { createScreenRead, ReadCallFailure } from "/src/screen-state/read";
import { Button } from "/src/ui/controls";
import { Card, CardHeader } from "/src/ui/layout";
import { ReadNotice, StatedNotice } from "/src/ui/read-notice";
import { deriveMemberAvailability, type InventoryAvailability } from "./pool-availability";

function inventoryLabel(value: InventoryAvailability): string {
    return translation()(($) => $.quotaMessages.inventory[value]);
}

function refreshText(refresh: LastRefresh | null): JSX.Element {
    if (refresh === null)
        return translation()(($) => $.screens.poolAvailabilityCard.readingStoredAvailability);
    switch (refresh.type) {
        case "not_observed":
            return translation()(($) => $.screens.poolAvailabilityCard.quotaNotObserved);
        case "succeeded":
            return quotaFactMessage("quotaObserved", formatTimestamp(refresh.observed_at));
        case "failed":
            return translation()(($) => $.quotaMessages.quotaFailed, {
                reason: translation()(($) => $.quotaMessages.failure[refresh.reason]),
            });
        case "unsupported":
            return translation()(($) => $.screens.poolAvailabilityCard.quotaReadsUnsupported);
        case "reset":
            return translation()(
                ($) => $.screens.poolAvailabilityCard.administratorResetRefreshGraceInstalled,
            );
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
                        <bdi dir="auto">
                            {props.provider?.name ??
                                translation()(($) => $.quotaMessages.deletedAccount, {
                                    id: formatNumber(props.providerId),
                                })}
                        </bdi>
                    </p>
                    <p class="mt-1 text-ink-dim text-[12.5px]">
                        {props.priority !== undefined ? (
                            <>
                                {translation()(($) => $.quotaMessages.priority, {
                                    priority: formatNumber(props.priority),
                                })}{" "}
                                ·{" "}
                            </>
                        ) : (
                            ""
                        )}
                        {props.isAccountInspection &&
                        availability().inventory === "model_not_selected"
                            ? translation()(($) => $.screens.poolAvailabilityCard.accountEnabled)
                            : inventoryLabel(availability().inventory)}
                    </p>
                </div>
                <div class="flex flex-wrap gap-2">
                    <Show when={props.canRefreshQuota !== false}>
                        <Button
                            isDisabled={isBusy() || props.provider === undefined}
                            onClick={() => void act(false)}
                        >
                            {translation()(($) => $.screens.poolAvailabilityCard.refreshQuota)}
                        </Button>
                    </Show>
                    <Button
                        isDisabled={isBusy() || props.provider === undefined}
                        onClick={() => void act(true)}
                    >
                        {translation()(($) => $.screens.poolAvailabilityCard.resetRestrictions)}
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
                {isBusy()
                    ? translation()(($) => $.screens.poolAvailabilityCard.updatingAvailability)
                    : refreshText(availability().refresh)}
            </p>
            <Show when={blocks().length > 0}>
                <p class="mt-3 font-medium">
                    {translation()(($) => $.screens.poolAvailabilityCard.quotaExhausted)}
                </p>
                <ul class="mt-2 space-y-2 text-[12.5px] text-ink-dim">
                    <For each={blocks()}>
                        {(block) => (
                            <li class="break-words">
                                <bdi dir="ltr">{block.key.join(" / ")}</bdi> ·{" "}
                                {block.applies_to.type === "all_models"
                                    ? translation()(($) => $.screens.poolAvailabilityCard.allModels)
                                    : quotaFactMessage(
                                          "models",
                                          block.applies_to.upstream_ids.join(", "),
                                      )}{" "}
                                ·{" "}
                                {block.blocked_until === null
                                    ? translation()(
                                          ($) => $.screens.poolAvailabilityCard.resetUnknown,
                                      )
                                    : block.blocked_until <= nowMicros()
                                      ? translation()(
                                            ($) =>
                                                $.screens.poolAvailabilityCard
                                                    .resetTimePassedRefreshRequired,
                                        )
                                      : quotaFactMessage(
                                            "resets",
                                            formatTimestamp(block.blocked_until),
                                        )}
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
                title={translation()(($) => $.screens.poolAvailabilityCard.poolAvailability)}
                description={translation()(
                    ($) =>
                        $.screens.poolAvailabilityCard
                            .storedObservationsRefreshExplicitlyQueriesTheUpstreamAccountMessage,
                )}
            />
            <Show
                when={props.pool.members.length > 0}
                fallback={
                    <p class="mt-4 text-ink-dim">
                        {translation()(
                            ($) => $.screens.poolAvailabilityCard.thisPoolHasNoMembersMessage,
                        )}
                    </p>
                }
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
