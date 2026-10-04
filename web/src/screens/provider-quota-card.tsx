import type { JSX } from "@solidjs/web";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { formatTimestamp, nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import { formatNumber, formatRelative } from "/src/i18n/formatters";
import { quotaFactMessage } from "/src/i18n/rich";
import type {
    QuotaEntry,
    QuotaMeasurement,
    QuotaReset,
    QuotaWindow,
} from "/src/management-api/generated/v0";
import { Button } from "/src/ui/controls";
import { Card, CardHeader } from "/src/ui/layout";
import type { ProviderQuotaView } from "./quota-view";
import { StatedNotice } from "./read-notice";

/** Fixed seconds say duration, never an inferred calendar boundary. */
function duration(seconds: number): string {
    const units: Array<
        readonly [
            number,
            "day" | "hour" | "minute" | "second",
        ]
    > = [
        [
            86400,
            "day",
        ],
        [
            3600,
            "hour",
        ],
        [
            60,
            "minute",
        ],
        [
            1,
            "second",
        ],
    ];
    const parts: string[] = [];
    let remainder = seconds;
    for (const [size, name] of units) {
        const amount = Math.floor(remainder / size);
        if (amount > 0)
            parts.push(
                translation()(($) => $.quotaMessages[name], {
                    count: amount,
                    quantity: formatNumber(amount),
                }),
            );
        remainder %= size;
    }
    return parts.join(" ");
}

function period(window: QuotaWindow): string {
    switch (window.type) {
        case "unknown":
            return translation()(($) => $.additional.providerQuotaCard.periodUnknown);
        case "calendar":
            return translation()(($) => $.additional.providerQuotaCard.monthly);
        case "duration":
            return duration(window.seconds);
    }
}

/** Preserve even extreme finite observations without overflowing the percentage multiplication. */
function measurementText(measurement: QuotaMeasurement): JSX.Element {
    switch (measurement.type) {
        case "unknown":
            return translation()(($) => $.additional.providerQuotaCard.unknown);
        case "unlimited":
            return translation()(($) => $.additional.providerQuotaCard.unlimited);
        case "ratio":
            return translation()(($) => $.quotaMessages.used, {
                percentage: formatNumber(measurement.used_fraction, "percentage"),
            });
        case "counter":
            return quotaFactMessage("counter", measurement.unit, {
                used: formatNumber(measurement.used, "quantity"),
                limit: formatNumber(measurement.limit, "quantity"),
            });
        case "remaining":
            return quotaFactMessage("remaining", measurement.unit, {
                remaining: formatNumber(measurement.remaining, "quantity"),
            });
    }
}

/** Accessible progress text uses the same observation, without flattening or interpreting JSX. */
function measurementAccessibleText(measurement: QuotaMeasurement): string {
    switch (measurement.type) {
        case "counter":
            return translation()(($) => $.quotaMessages.counter)
                .replace("[[used]]", formatNumber(measurement.used, "quantity"))
                .replace("[[limit]]", formatNumber(measurement.limit, "quantity"))
                .replace("[[unit]]", measurement.unit);
        case "remaining":
            return translation()(($) => $.quotaMessages.remaining)
                .replace("[[remaining]]", formatNumber(measurement.remaining, "quantity"))
                .replace("[[unit]]", measurement.unit);
        case "ratio":
            return translation()(($) => $.quotaMessages.used, {
                percentage: formatNumber(measurement.used_fraction, "percentage"),
            });
        case "unknown":
            return translation()(($) => $.additional.providerQuotaCard.unknown);
        case "unlimited":
            return translation()(($) => $.additional.providerQuotaCard.unlimited);
    }
}

/** This value is visual only; the accessible text always states the actual observation. */
function fill(measurement: QuotaMeasurement): number | null {
    if (measurement.type === "ratio") return Math.min(100, measurement.used_fraction * 100);
    if (measurement.type === "counter")
        return Math.min(100, (measurement.used / measurement.limit) * 100);
    return null;
}

function resetText(reset: QuotaReset, now: number): JSX.Element {
    switch (reset.type) {
        case "unknown":
            return translation()(($) => $.additional.providerQuotaCard.resetUnknown);
        case "date":
            return quotaFactMessage("dateReset", reset.date);
        case "at":
            return quotaFactMessage("atReset", formatTimestamp(reset.at), {
                distance:
                    reset.at <= now
                        ? translation()(
                              ($) => $.helpers.providerQuotaCard.resetTimeReachedRefreshToUpdate,
                          )
                        : translation()(($) => $.quotaMessages.future, {
                              duration: duration(Math.ceil((reset.at - now) / 1_000_000)),
                          }),
            });
    }
}

function QuotaRow(props: { readonly entry: QuotaEntry; readonly now: number }): JSX.Element {
    const text = (): JSX.Element => measurementText(props.entry.measurement);
    return (
        <li class="py-4 first:pt-0 last:pb-0">
            <div class="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
                <div class="min-w-0 flex-1 break-words">
                    <p class="text-[13.5px] font-medium">
                        <bdi dir="auto">{props.entry.label}</bdi>
                    </p>
                    <p class="mt-0.5 text-[12.5px] text-ink-dim">
                        <Show
                            when={
                                props.entry.scope.type === "named" ? props.entry.scope.name : null
                            }
                        >
                            {(name) => (
                                <>
                                    <bdi dir="auto">{name()}</bdi> ·{" "}
                                </>
                            )}
                        </Show>
                        {period(props.entry.window)}
                    </p>
                </div>
                <p class="tnum break-words text-[13.5px] font-medium">{text()}</p>
            </div>
            <Show when={fill(props.entry.measurement) !== null}>
                <div
                    role="progressbar"
                    aria-label={props.entry.label}
                    aria-valuemin={0}
                    aria-valuemax={100}
                    aria-valuenow={fill(props.entry.measurement) ?? 0}
                    aria-valuetext={measurementAccessibleText(props.entry.measurement)}
                    class="mt-2.5 h-1.5 overflow-hidden rounded-full bg-muted"
                >
                    <div
                        class="h-full rounded-full bg-accent"
                        style={{
                            width: `${fill(props.entry.measurement)}%`,
                        }}
                    />
                </div>
            </Show>
            <p class="tnum mt-2 break-words text-[12.5px] text-ink-dim">
                {resetText(props.entry.reset, props.now)}
            </p>
        </li>
    );
}

/** Upstream allowance is an independent read, separate from gateway request accounting. */
export function ProviderQuotaCard(props: { readonly view: ProviderQuotaView }): JSX.Element {
    const [now, setNow] = createSignal(nowMicros());
    const timer = setInterval(() => setNow(nowMicros()), 1000);
    onCleanup(() => clearInterval(timer));
    const ready = () => {
        const current = props.view.state();
        return current.type === "ready" ? current : null;
    };
    const notice = () => {
        const current = props.view.state();
        return current.type === "failed"
            ? current.error
            : current.type === "ready" && current.refresh.type === "failed"
              ? current.refresh.error
              : null;
    };
    const isBusy = (): boolean => {
        const current = props.view.state();
        return (
            current.type === "checking" ||
            current.type === "loading" ||
            (current.type === "ready" && current.refresh.type === "loading")
        );
    };
    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.screens.providerQuotaCard.upstreamQuota)}
                description={translation()(
                    ($) =>
                        $.screens.providerQuotaCard
                            .allowancesBelongToTheUpstreamAccountAndMayMessage,
                )}
            >
                <Show when={props.view.state().type !== "unsupported"}>
                    <Button
                        glyph="refresh"
                        isBusy={isBusy()}
                        onClick={props.view.refresh}
                    >
                        {isBusy()
                            ? ready() === null
                                ? translation()(($) => $.additional.providerQuotaCard.readingQuota)
                                : translation()(
                                      ($) => $.additional.providerQuotaCard.refreshingQuota,
                                  )
                            : notice() === null
                              ? translation()(($) => $.additional.providerQuotaCard.refreshQuota)
                              : translation()(($) => $.additional.providerQuotaCard.retryQuota)}
                    </Button>
                </Show>
            </CardHeader>
            <StatedNotice
                notice={notice}
                class="mt-4"
            />
            <Show when={props.view.state().type === "unsupported"}>
                <p class="mt-4 text-ink-dim">
                    {translation()(
                        ($) =>
                            $.screens.providerQuotaCard
                                .thisProviderDefinitionDoesNotDeclareQuotaViewingMessage,
                    )}
                </p>
            </Show>
            <Show when={props.view.state().type === "checking"}>
                <p
                    role="status"
                    class="mt-4 text-ink-dim"
                >
                    {translation()(
                        ($) =>
                            $.screens.providerQuotaCard
                                .checkingWhetherThisDefinitionSupportsQuotaReads,
                    )}
                </p>
            </Show>
            <Show when={props.view.state().type === "unobserved"}>
                <p class="mt-4 text-ink-dim">
                    {translation()(
                        ($) =>
                            $.screens.providerQuotaCard
                                .quotaNotObservedRefreshExplicitlyQueriesTheUpstreamMessage,
                    )}
                </p>
            </Show>
            <Show when={props.view.state().type === "loading"}>
                <p
                    role="status"
                    class="mt-4 text-ink-dim"
                >
                    {translation()(($) => $.screens.providerQuotaCard.readingUpstreamQuota)}
                </p>
            </Show>
            <Show when={ready()}>
                {(current) => (
                    <>
                        <p class="tnum mt-4 break-words text-[12.5px] text-ink-dim">
                            <Show when={current().refresh.type === "failed"}>
                                {translation()(
                                    ($) =>
                                        $.screens.providerQuotaCard
                                            .showingThePreviousReadingMessage,
                                )}{" "}
                            </Show>
                            {quotaFactMessage(
                                "observed",
                                formatTimestamp(current().snapshot.observed_at),
                                {
                                    distance: formatRelative(
                                        -Math.max(
                                            0,
                                            Math.floor(
                                                (now() - current().snapshot.observed_at) /
                                                    1_000_000,
                                            ),
                                        ),
                                        "second",
                                    ),
                                },
                            )}
                        </p>
                        <Show
                            when={current().snapshot.quotas.length > 0}
                            fallback={
                                <p class="mt-4 text-ink-dim">
                                    {translation()(
                                        ($) =>
                                            $.screens.providerQuotaCard
                                                .theUpstreamReturnedNoQuotaEntriesMessage,
                                    )}
                                </p>
                            }
                        >
                            <ul class="mt-4 divide-y divide-rule">
                                <For each={current().snapshot.quotas}>
                                    {(entry) => (
                                        <QuotaRow
                                            entry={entry}
                                            now={now()}
                                        />
                                    )}
                                </For>
                            </ul>
                        </Show>
                    </>
                )}
            </Show>
        </Card>
    );
}
