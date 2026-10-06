import { render } from "@solidjs/web";
import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type {
    DefinitionResponse,
    GetProviderQuotaResponse,
} from "/src/management-api/generated/v0";
import { PROVIDER_PRESETS } from "/src/presets/catalog";
import {
    bodyText,
    buttonWithText,
    createDisposals,
    providerRow,
    settle,
    stubManagementApi,
} from "/src/test-support/management-api";
import { ProviderQuotaCard } from "./provider-quota-card";
import type { QuotaViewState } from "./quota-view";
import { createProviderQuotaView } from "./quota-view";

const disposes = createDisposals();
const codexPreset = PROVIDER_PRESETS.find((value) => value.id === "codex");
if (codexPreset === undefined) throw new Error("Codex fixture is required");
const quotaDefinition: DefinitionResponse = {
    ...codexPreset.payload,
    name: "exact",
    version: 2,
    created_at: 1,
};
const quotaProvider = providerRow({
    provider_definition_name: "exact",
    provider_definition_version: 2,
});
const snapshot: GetProviderQuotaResponse = {
    observed_at: 1_000_000,
    quotas: [
        {
            key: [
                "ratio",
            ],
            label: "Five-hour limit",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "unknown",
            },
            window: {
                type: "duration",
                seconds: 18000,
            },
            measurement: {
                type: "ratio",
                used_fraction: 1.125,
            },
            reset: {
                type: "at",
                at: 2_000_000,
            },
        },
        {
            key: [
                "counter",
            ],
            label: "Pool",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "named",
                name: "chat",
            },
            window: {
                type: "calendar",
                unit: "month",
            },
            measurement: {
                type: "counter",
                used: 12.5,
                limit: 10,
                unit: "requests",
            },
            reset: {
                type: "date",
                date: "2026-10-31",
            },
        },
        {
            key: [
                "remaining",
            ],
            label: "Pool",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "named",
                name: "completion",
            },
            window: {
                type: "unknown",
            },
            measurement: {
                type: "remaining",
                remaining: 0,
                unit: "tokens",
            },
            reset: {
                type: "unknown",
            },
        },
        {
            key: [
                "unknown",
            ],
            label: "Unknown pool",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "unknown",
            },
            window: {
                type: "unknown",
            },
            measurement: {
                type: "unknown",
            },
            reset: {
                type: "unknown",
            },
        },
        {
            key: [
                "unlimited",
            ],
            label: "Unlimited pool",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "unknown",
            },
            window: {
                type: "unknown",
            },
            measurement: {
                type: "unlimited",
            },
            reset: {
                type: "unknown",
            },
        },
        {
            key: [
                "zero",
            ],
            label: "Weekly limit",
            routing: {
                type: "monitoring_only",
            },
            scope: {
                type: "unknown",
            },
            window: {
                type: "duration",
                seconds: 604800,
            },
            measurement: {
                type: "ratio",
                used_fraction: 0,
            },
            reset: {
                type: "unknown",
            },
        },
    ],
};

afterEach(() => vi.useRealTimers());

describe("quota card observations", () => {
    it("keeps unknown, unlimited, zero, over-limit, and date-only facts distinct and ages locally without fetching", async () => {
        vi.useFakeTimers();
        vi.setSystemTime(1500);
        const refresh = vi.fn();
        disposes.push(
            render(
                () => (
                    <ProviderQuotaCard
                        view={{
                            state: () => ({
                                type: "ready",
                                snapshot,
                                refresh: {
                                    type: "idle",
                                },
                            }),
                            refresh,
                        }}
                    />
                ),
                document.body,
            ),
        );
        await vi.advanceTimersByTimeAsync(0);
        expect(bodyText()).toContain("112.5% used");
        expect(bodyText()).toContain("12.5 / 10 requests used");
        expect(bodyText()).toContain("0 tokens remaining");
        expect(bodyText()).toContain("0.0% used");
        expect(bodyText()).toContain("Unlimited");
        expect(bodyText()).toContain("Unknown");
        expect(bodyText()).toContain("5 hours");
        expect(bodyText()).toContain("7 days");
        expect(bodyText()).toContain("Monthly");
        expect(bodyText()).toContain("2026-10-31 · Exact reset time unknown");
        const progress = Array.from(document.querySelectorAll("[role=progressbar]"));
        expect(progress).toHaveLength(3);
        expect(progress[0]?.getAttribute("aria-valuenow")).toBe("100");
        expect(progress[0]?.getAttribute("aria-valuetext")).toBe("112.5% used");
        await vi.advanceTimersByTimeAsync(2000);
        expect(bodyText()).toContain("Reset time reached; refresh to update");
        expect(bodyText()).toContain("112.5% used");
        expect(refresh).not.toHaveBeenCalled();
        disposes.pop()?.();
        expect(vi.getTimerCount()).toBe(0);
    });

    it("renders isolated initial failure, older refresh failure, loading, unsupported, and valid empty success", async () => {
        let update: (next: QuotaViewState) => void = () => undefined;
        const refresh = vi.fn();
        disposes.push(
            render(() => {
                const [state, setState] = createSignal<QuotaViewState>({
                    type: "failed",
                    error: {
                        tone: "bad",
                        title: "Quota unavailable",
                    },
                });
                update = setState;
                return (
                    <ProviderQuotaCard
                        view={{
                            state,
                            refresh,
                        }}
                    />
                );
            }, document.body),
        );
        await settle();
        expect(bodyText()).toContain("Quota unavailable");
        buttonWithText("Retry quota").click();
        expect(refresh).toHaveBeenCalledTimes(1);
        update({
            type: "ready",
            snapshot,
            refresh: {
                type: "failed",
                error: {
                    tone: "bad",
                    title: "Refresh unavailable",
                },
            },
        });
        await settle();
        expect(bodyText()).toContain("Showing the previous reading.");
        expect(bodyText()).toContain("112.5% used");
        expect(bodyText()).toContain("Observed");
        update({
            type: "ready",
            snapshot,
            refresh: {
                type: "loading",
            },
        });
        await settle();
        expect(buttonWithText("Refreshing quota").disabled).toBe(true);
        update({
            type: "ready",
            snapshot: {
                observed_at: 2,
                quotas: [],
            },
            refresh: {
                type: "idle",
            },
        });
        await settle();
        expect(bodyText()).toContain("The upstream returned no quota entries.");
        expect(bodyText()).not.toContain("112.5% used");
        update({
            type: "unsupported",
        });
        await settle();
        expect(bodyText()).toContain("This provider definition does not declare quota viewing.");
        expect(document.querySelectorAll("button")).toHaveLength(0);
        update({
            type: "loading",
        });
        await settle();
        expect(bodyText()).toContain("Reading upstream quota");
    });

    it("reports a succeeded quota observation and never a failed one", async () => {
        // The completion callback is the card's only word to the shell: a
        // succeeded observation may stamp the shell's freshness (README
        // §6.3.1), so the pin rides the real view over the stubbed wire — a
        // failure states its diagnosis on the card and reports nothing,
        // re-running no other read.
        const get = vi
            .fn()
            .mockResolvedValueOnce(snapshot)
            .mockRejectedValueOnce(new Error("quota upstream unavailable"))
            .mockResolvedValueOnce({
                observed_at: 456,
                quotas: [],
            });
        const api = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    quotaDefinition,
                ],
            }),
            getProviderQuota: get,
        });
        const onQuotaReadComplete = vi.fn();
        const view = createRoot((dispose) => {
            disposes.push(dispose);
            return createProviderQuotaView({
                api: () => api,
                provider: () => quotaProvider,
                onQuotaReadComplete,
            });
        });
        await settle();

        // The capability check alone observes no quota: nothing to report.
        expect(view.state().type).toBe("unobserved");
        expect(onQuotaReadComplete).not.toHaveBeenCalled();

        view.refresh();
        await settle();
        expect(view.state()).toMatchObject({
            type: "ready",
            snapshot,
        });
        expect(onQuotaReadComplete).toHaveBeenCalledTimes(1);

        view.refresh();
        await settle();
        expect(view.state()).toMatchObject({
            type: "ready",
            refresh: {
                type: "failed",
            },
        });
        expect(onQuotaReadComplete).toHaveBeenCalledTimes(1);

        view.refresh();
        await settle();
        expect(view.state()).toMatchObject({
            type: "ready",
            snapshot: {
                observed_at: 456,
            },
        });
        expect(onQuotaReadComplete).toHaveBeenCalledTimes(2);
    });
});
