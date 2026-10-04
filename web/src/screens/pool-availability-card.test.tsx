import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { GetProviderAvailabilityResponse } from "/src/management-api/generated/v0";
import { PoolAvailabilityCard } from "./pool-availability-card";
import {
    buttonWithText,
    createDisposals,
    modelRow,
    providerRow,
    settle,
    stubManagementApi,
} from "./screen-test-support";

const disposes = createDisposals();

describe("pool availability controls", () => {
    it("ignores a pre-reset read and performs no reconciliation after an upstream query outlives its owner", async () => {
        let resolveOldRead: (value: GetProviderAvailabilityResponse) => void = () => {
            throw new Error("old read not started");
        };
        const oldRead = new Promise<GetProviderAvailabilityResponse>((resolve) => {
            resolveOldRead = resolve;
        });
        let resolveQuota: () => void = () => {
            throw new Error("quota query not started");
        };
        const quota = new Promise<never>((_resolve, reject) => {
            resolveQuota = () => reject(new Error("late failure"));
        });
        const calls: string[] = [];
        const current: GetProviderAvailabilityResponse = {
            blocks: [],
            last_refresh: {
                type: "reset",
                attempted_at: 4,
            },
        };
        const api = stubManagementApi({
            getProviderAvailability: () => {
                calls.push("availability");
                return calls.length === 1 ? oldRead : Promise.resolve(current);
            },
            resetProviderAvailability: () => {
                calls.push("reset");
                return Promise.resolve();
            },
            getProviderQuota: () => {
                calls.push("quota");
                return quota;
            },
        });
        const dispose = render(
            () => (
                <PoolAvailabilityCard
                    api={api}
                    pool={{
                        model_prefix: "",
                        id: 3,
                        name: "Coding",
                        is_enabled: true,
                        members: [
                            {
                                provider_id: 1,
                                priority: 0,
                            },
                        ],
                    }}
                    providers={[
                        providerRow(),
                    ]}
                    models={[
                        modelRow(),
                    ]}
                    upstreamId={null}
                />
            ),
            document.body,
        );
        disposes.push(dispose);
        await settle();
        buttonWithText("Reset restrictions").click();
        await settle();
        resolveOldRead({
            blocks: [
                {
                    key: [
                        "old",
                    ],
                    applies_to: {
                        type: "all_models",
                    },
                    observed_at: 1,
                    blocked_until: null,
                },
            ],
            last_refresh: {
                type: "not_observed",
            },
        });
        await settle();
        expect(document.body.textContent).toContain("Administrator reset; refresh grace installed");
        expect(document.body.textContent).not.toContain("Quota exhausted");
        buttonWithText("Refresh quota").click();
        await settle();
        dispose();
        resolveQuota();
        await settle();
        expect(calls).toEqual([
            "availability",
            "reset",
            "availability",
            "quota",
        ]);
        expect(document.body.textContent).toBe("");
    });

    it("reads only durable state on mount, reconciles after a failed explicit query, and resets without another quota query", async () => {
        const calls: string[] = [];
        let observation: GetProviderAvailabilityResponse = {
            blocks: [
                {
                    key: [
                        "weekly",
                    ],
                    applies_to: {
                        type: "all_models",
                    },
                    observed_at: 1,
                    blocked_until: 2,
                },
            ],
            last_refresh: {
                type: "not_observed",
            },
        };
        const api = stubManagementApi({
            getProviderAvailability: () => {
                calls.push("availability");
                return Promise.resolve(observation);
            },
            getProviderQuota: () => {
                calls.push("quota");
                observation = {
                    ...observation,
                    last_refresh: {
                        type: "failed",
                        attempted_at: 3,
                        reason: "transport",
                    },
                };
                return Promise.reject(new Error("quota unavailable"));
            },
            resetProviderAvailability: () => {
                calls.push("reset");
                observation = {
                    blocks: [],
                    last_refresh: {
                        type: "reset",
                        attempted_at: 4,
                    },
                };
                return Promise.resolve();
            },
        });
        disposes.push(
            render(
                () => (
                    <PoolAvailabilityCard
                        api={api}
                        pool={{
                            model_prefix: "",
                            id: 3,
                            name: "Coding",
                            is_enabled: true,
                            members: [
                                {
                                    provider_id: 1,
                                    priority: 0,
                                },
                            ],
                        }}
                        providers={[
                            providerRow(),
                        ]}
                        models={[
                            modelRow({
                                upstream_id: "M",
                            }),
                        ]}
                        upstreamId="M"
                    />
                ),
                document.body,
            ),
        );
        await settle();
        expect(calls).toEqual([
            "availability",
        ]);
        expect(document.body.textContent).toContain("Quota exhausted");
        expect(document.body.textContent).toContain("Reset time passed; refresh required");
        buttonWithText("Refresh quota").click();
        await settle();
        expect(calls).toEqual([
            "availability",
            "quota",
            "availability",
        ]);
        expect(document.body.textContent).toContain("Quota read failed");
        expect(document.body.textContent).toContain("Quota exhausted");
        buttonWithText("Reset restrictions").click();
        await settle();
        expect(calls).toEqual([
            "availability",
            "quota",
            "availability",
            "reset",
            "availability",
        ]);
        expect(document.body.textContent).not.toContain("Quota exhausted");
        expect(document.body.textContent).toContain("Administrator reset; refresh grace installed");
    });
});
