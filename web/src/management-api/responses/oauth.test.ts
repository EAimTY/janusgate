import { describe, expect, test } from "vitest";
import { ContractViolationError } from "../errors";
import { parseCompleteProviderOauthResponse, parseStartProviderOauthResponse } from "./oauth";
import { parseGetProviderQuotaResponse } from "./quota";

/**
 * The OAuth flow bodies and the quota observation: the flow begin union
 * discriminated by the grant (the optional device URIs passing through
 * exactly as the wire omits them), the completion union over succeeded,
 * pending, and failed, and the quota observation validated whole — unique
 * non-empty keys, every member a declared variant — before any row reaches
 * the UI.
 */

describe("start/reauthorize response parser", () => {
    test("answers the authorization-code arm with its authorize URL", () => {
        const started = parseStartProviderOauthResponse({
            grant: "authorization_code",
            state: "s-1",
            expires_at: 1_800_000_000_000_000,
            authorize_url: "https://auth.example.test/authorize?client_id=abc",
        });

        expect(started).toEqual({
            grant: "authorization_code",
            state: "s-1",
            expires_at: 1_800_000_000_000_000,
            authorize_url: "https://auth.example.test/authorize?client_id=abc",
        });
    });

    test("answers the device arm with its codes and optional URIs", () => {
        const bare = parseStartProviderOauthResponse({
            grant: "device",
            state: "s-2",
            expires_at: 1_800_000_000_000_000,
            user_code: "ABCD-EFGH",
        });

        expect(bare).toEqual({
            grant: "device",
            state: "s-2",
            expires_at: 1_800_000_000_000_000,
            user_code: "ABCD-EFGH",
        });
        expect(bare).not.toHaveProperty("verification_uri");

        const complete = parseStartProviderOauthResponse({
            grant: "device",
            state: "s-3",
            expires_at: 1_800_000_000_000_000,
            user_code: "ABCD-EFGH",
            verification_uri: "https://auth.example.test/device",
            verification_uri_complete: "https://auth.example.test/device?code=ABCD-EFGH",
        });

        if (complete.grant !== "device") throw new Error("expected the device arm");
        expect(complete.verification_uri).toBe("https://auth.example.test/device");
        expect(complete.verification_uri_complete).toBe(
            "https://auth.example.test/device?code=ABCD-EFGH",
        );
    });

    test("rejects a foreign grant and a wrongly-typed expiry", () => {
        expect(() =>
            parseStartProviderOauthResponse({
                grant: "implicit",
            }),
        ).toThrowError(ContractViolationError);
        expect(() =>
            parseStartProviderOauthResponse({
                grant: "authorization_code",
                state: "s-1",
                expires_at: "soon",
                authorize_url: "https://auth.example.test",
            }),
        ).toThrowError("response.expires_at must be an integer");
    });
});

describe("complete/poll response parser", () => {
    test("answers the succeeded arm with the provider row", () => {
        const completed = parseCompleteProviderOauthResponse({
            status: "succeeded",
            provider: {
                type: "oauth",
                id: 4,
                name: "acme",
                model_prefix: "acme",
                is_enabled: true,
                provider_definition_name: "claude-code",
                provider_definition_version: 1,
                format: "anthropic_messages",
                base_url: null,
                expires_at: null,
            },
        });

        if (completed.status !== "succeeded") throw new Error("expected the succeeded arm");
        expect(completed.provider.name).toBe("acme");
    });

    test("answers the pending and failed arms with their own fields alone", () => {
        expect(
            parseCompleteProviderOauthResponse({
                status: "pending",
            }),
        ).toEqual({
            status: "pending",
        });

        const failed = parseCompleteProviderOauthResponse({
            status: "failed",
            failure_code: "access_denied",
            failure_message: "the administrator refused",
        });

        expect(failed).toEqual({
            status: "failed",
            failure_code: "access_denied",
            failure_message: "the administrator refused",
        });
    });

    test("rejects a foreign status", () => {
        expect(() =>
            parseCompleteProviderOauthResponse({
                status: "maybe",
            }),
        ).toThrowError("response.status is not a flow outcome: maybe");
    });
});

describe("provider quota observation parser", () => {
    test("answers a complete observation with its entries intact", () => {
        const observation = parseGetProviderQuotaResponse({
            observed_at: 1_800_000_000_000_000,
            quotas: [
                {
                    key: [
                        "usage",
                        "tokens",
                    ],
                    label: "Token pool",
                    scope: {
                        type: "named",
                        name: "weekly",
                    },
                    routing: {
                        type: "all_models",
                    },
                    window: {
                        type: "duration",
                        seconds: 604_800,
                    },
                    measurement: {
                        type: "counter",
                        used: 1000,
                        limit: 5000,
                        unit: "tokens",
                    },
                    reset: {
                        type: "at",
                        at: 1_800_100_000_000_000,
                    },
                },
            ],
        });

        expect(observation.quotas).toHaveLength(1);
        const entry = observation.quotas[0];
        if (entry === undefined) throw new Error("the fixture listed no entry");
        expect(entry.measurement.type).toBe("counter");
    });

    test("rejects an empty or duplicated quota key and an undeclared member", () => {
        expect(() =>
            parseGetProviderQuotaResponse({
                observed_at: 1,
                quotas: [
                    {
                        key: [],
                        label: "x",
                        scope: {
                            type: "unknown",
                        },
                        routing: {
                            type: "all_models",
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
                ],
            }),
        ).toThrowError("must not be empty or repeat another quota's key");

        expect(() =>
            parseGetProviderQuotaResponse({
                observed_at: 1,
                quotas: [
                    {
                        key: [
                            "a",
                        ],
                        label: "x",
                        scope: {
                            type: "unknown",
                        },
                        routing: {
                            type: "all_models",
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
                        extra: 1,
                    },
                ],
            }),
        ).toThrowError("response.quotas[0].extra is not a declared field");
    });
});
