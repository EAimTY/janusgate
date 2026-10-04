import { describe, expect, it } from "vitest";
import type { GetProviderAvailabilityResponse } from "/src/management-api/generated/v0";
import { deriveMemberAvailability } from "./pool-availability";
import { modelRow, providerRow } from "./screen-test-support";

const provider = providerRow();
const model = modelRow({
    upstream_id: "M",
});
const observation: GetProviderAvailabilityResponse = {
    blocks: [
        {
            key: [
                "all",
            ],
            applies_to: {
                type: "all_models",
            },
            observed_at: 10,
            blocked_until: 20,
        },
        {
            key: [
                "exact",
            ],
            applies_to: {
                type: "models",
                upstream_ids: [
                    "M",
                ],
            },
            observed_at: 10,
            blocked_until: null,
        },
        {
            key: [
                "other",
            ],
            applies_to: {
                type: "models",
                upstream_ids: [
                    "m",
                ],
            },
            observed_at: 10,
            blocked_until: 500,
        },
    ],
    last_refresh: {
        type: "failed",
        attempted_at: 100,
        reason: "transport",
    },
};

describe("pool member availability", () => {
    it("keeps exact applicable restrictions, including a passed reset, independently of failed refresh and structural state", () => {
        const result = deriveMemberAvailability(
            true,
            provider,
            [
                model,
            ],
            "M",
            observation,
        );
        expect(result.inventory).toBe("ready");
        expect(result.blocks).toEqual(observation.blocks.slice(0, 2));
        expect(result.refresh).toEqual(observation.last_refresh);
        const absent = deriveMemberAvailability(true, provider, [], "M", observation);
        expect(absent.inventory).toBe("model_missing");
        expect(absent.blocks).toEqual(result.blocks);
        expect(
            deriveMemberAvailability(
                true,
                provider,
                [
                    model,
                ],
                "different",
                observation,
            ).blocks,
        ).toEqual([
            observation.blocks[0],
        ]);
    });

    it("distinguishes authored disablement, missing inventory, vanished inventory, and unobserved quota without inventing capacity", () => {
        for (const [isPoolEnabled, account, inventory, upstreamId, expected] of [
            [
                false,
                provider,
                [
                    model,
                ],
                "M",
                "pool_disabled",
            ],
            [
                true,
                undefined,
                [
                    model,
                ],
                "M",
                "account_missing",
            ],
            [
                true,
                {
                    ...provider,
                    is_enabled: false,
                },
                [],
                "M",
                "account_disabled",
            ],
            [
                true,
                provider,
                [],
                "M",
                "model_missing",
            ],
            [
                true,
                provider,
                [
                    {
                        ...model,
                        discovery_state: "vanished",
                    },
                ],
                "M",
                "model_vanished",
            ],
            [
                true,
                provider,
                [
                    {
                        ...model,
                        is_enabled: false,
                    },
                ],
                "M",
                "model_disabled",
            ],
            [
                true,
                provider,
                [
                    model,
                ],
                null,
                "model_not_selected",
            ],
        ] as const) {
            const result = deriveMemberAvailability(
                isPoolEnabled,
                account,
                inventory,
                upstreamId,
                null,
            );
            expect(result.inventory).toBe(expected);
            expect(result.refresh).toBeNull();
            expect(result.blocks).toEqual([]);
        }
    });
});
