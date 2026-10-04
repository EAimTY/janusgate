import type { JSX } from "@solidjs/web";
import { createSignal, For, onCleanup, Show } from "solid-js";
import { formatTimestamp, nowMicros } from "/src/format";
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
            string,
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
        if (amount > 0) parts.push(`${amount.toLocaleString()} ${name}${amount === 1 ? "" : "s"}`);
        remainder %= size;
    }
    return parts.join(" ");
}

function period(window: QuotaWindow): string {
    switch (window.type) {
        case "unknown":
            return "Period unknown";
        case "calendar":
            return "Monthly";
        case "duration":
            return duration(window.seconds);
    }
}

/** Preserve even extreme finite observations without overflowing the percentage multiplication. */
function percent(fraction: number): string {
    if (fraction < 1e19) return `${(fraction * 100).toFixed(1)}%`;
    const [mantissa, exponent] = fraction.toExponential(1).split("e");
    return `${mantissa}e+${Number(exponent) + 2}%`;
}

function measurementText(measurement: QuotaMeasurement): string {
    switch (measurement.type) {
        case "unknown":
            return "Unknown";
        case "unlimited":
            return "Unlimited";
        case "ratio":
            return `${percent(measurement.used_fraction)} used`;
        case "counter":
            return `${measurement.used.toLocaleString()} / ${measurement.limit.toLocaleString()} ${measurement.unit} used`;
        case "remaining":
            return `${measurement.remaining.toLocaleString()} ${measurement.unit} remaining`;
    }
}

/** This value is visual only; the accessible text always states the actual observation. */
function fill(measurement: QuotaMeasurement): number | null {
    if (measurement.type === "ratio") return Math.min(100, measurement.used_fraction * 100);
    if (measurement.type === "counter")
        return Math.min(100, (measurement.used / measurement.limit) * 100);
    return null;
}

function resetText(reset: QuotaReset, now: number): string {
    switch (reset.type) {
        case "unknown":
            return "Reset unknown";
        case "date":
            return `${reset.date} · Exact reset time unknown`;
        case "at":
            return `Resets ${formatTimestamp(reset.at)} · ${
                reset.at <= now
                    ? "Reset time reached; refresh to update"
                    : `in ${duration(Math.ceil((reset.at - now) / 1_000_000))}`
            }`;
    }
}

function QuotaRow(props: { readonly entry: QuotaEntry; readonly now: number }): JSX.Element {
    const text = (): string => measurementText(props.entry.measurement);
    return (
        <li class="py-4 first:pt-0 last:pb-0">
            <div class="flex flex-wrap items-start justify-between gap-x-6 gap-y-2">
                <div class="min-w-0 flex-1 break-words">
                    <p class="text-[13.5px] font-medium">{props.entry.label}</p>
                    <p class="mt-0.5 text-[12.5px] text-ink-dim">
                        <Show
                            when={
                                props.entry.scope.type === "named" ? props.entry.scope.name : null
                            }
                        >
                            {(name) => <>{name()} · </>}
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
                    aria-valuetext={text()}
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
                title="Upstream quota"
                description="Allowances belong to the upstream account and may include usage from other clients."
            >
                <Show when={props.view.state().type !== "unsupported"}>
                    <Button
                        glyph="refresh"
                        isBusy={isBusy()}
                        onClick={props.view.refresh}
                    >
                        {isBusy()
                            ? ready() === null
                                ? "Reading quota…"
                                : "Refreshing quota…"
                            : notice() === null
                              ? "Refresh quota"
                              : "Retry quota"}
                    </Button>
                </Show>
            </CardHeader>
            <StatedNotice
                notice={notice}
                class="mt-4"
            />
            <Show when={props.view.state().type === "unsupported"}>
                <p class="mt-4 text-ink-dim">
                    This provider definition does not declare quota viewing.
                </p>
            </Show>
            <Show when={props.view.state().type === "checking"}>
                <p
                    role="status"
                    class="mt-4 text-ink-dim"
                >
                    Checking whether this definition supports quota reads…
                </p>
            </Show>
            <Show when={props.view.state().type === "unobserved"}>
                <p class="mt-4 text-ink-dim">
                    Quota not observed. Refresh explicitly queries the upstream account.
                </p>
            </Show>
            <Show when={props.view.state().type === "loading"}>
                <p
                    role="status"
                    class="mt-4 text-ink-dim"
                >
                    Reading upstream quota…
                </p>
            </Show>
            <Show when={ready()}>
                {(current) => (
                    <>
                        <p class="tnum mt-4 break-words text-[12.5px] text-ink-dim">
                            <Show when={current().refresh.type === "failed"}>
                                Showing the previous reading.{" "}
                            </Show>
                            Observed {formatTimestamp(current().snapshot.observed_at)} ·{" "}
                            {Math.max(
                                0,
                                Math.floor((now() - current().snapshot.observed_at) / 1_000_000),
                            ).toLocaleString()}{" "}
                            seconds ago
                        </p>
                        <Show
                            when={current().snapshot.quotas.length > 0}
                            fallback={
                                <p class="mt-4 text-ink-dim">
                                    The upstream returned no quota entries.
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
