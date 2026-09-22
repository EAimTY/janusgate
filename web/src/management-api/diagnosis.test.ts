import { describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import type { ActionContext, Reachability } from "./diagnosis";
import { diagnoseFailure } from "./diagnosis";

const context: ActionContext = {
    baseUrl: "https://gateway.example.com",
    pageOrigin: "https://admin.example.com",
};

function gatewayFailure(errno: ErrorCode, message: string): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno,
        httpStatus: 400,
    });
}

describe("diagnoseFailure", () => {
    it("preserves the gateway's own message as the detail of every rendered failure", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.UpstreamError, "upstream answered 401"),
            context,
        );

        expect(diagnosis.detail).toBe("upstream answered 401");
        expect(diagnosis.kind).toBe("upstream");
    });

    it("names the token's source for an unauthorized answer", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.Unauthorized, "invalid management token"),
            context,
        );

        expect(diagnosis.kind).toBe("unauthorized");
        expect(diagnosis.fix).toContain("management_api.token");
    });

    it("reports a gone row as having no recovery on this screen", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.NotFound, "no such provider"),
            context,
        );

        expect(diagnosis.kind).toBe("gone");
        expect(diagnosis.fix).toBeNull();
    });

    it("names the taken identifier for a conflict", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.Conflict, "provider name is taken"),
            {
                ...context,
                action: "create-provider",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(diagnosis.fix).not.toBeNull();
    });

    it("names the colliding public ID for a model creation conflict", () => {
        const diagnosis = diagnoseFailure(gatewayFailure(ErrorCode.Conflict, "public id taken"), {
            ...context,
            action: "create-model",
        });

        expect(diagnosis.kind).toBe("conflict");
        expect(diagnosis.fix).toContain("public ID");
    });

    it("points a taken key secret at generating a fresh one", () => {
        const diagnosis = diagnoseFailure(gatewayFailure(ErrorCode.Conflict, "api key taken"), {
            ...context,
            action: "create-api-key",
        });

        expect(diagnosis.kind).toBe("conflict");
        expect(diagnosis.fix).toContain("fresh one");
    });

    it("keeps an upstream refusal during discovery agnostic when no provider type is known", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.UpstreamError, "upstream answered 502"),
            {
                ...context,
                action: "discover-models",
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(diagnosis.fix).toContain("provider's state");
    });

    it("points an OAuth provider's discovery refusal at reconnecting its expired authorization", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.UpstreamError, "upstream answered 502"),
            {
                ...context,
                action: "discover-models",
                providerType: "codex_oauth",
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(diagnosis.fix).toContain("authorization expired");
        expect(diagnosis.fix).toContain("reconnect");
        expect(diagnosis.fix).not.toContain("API key");
    });

    it("points a static-key provider's discovery refusal at its connection fields", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.UpstreamError, "upstream answered 502"),
            {
                ...context,
                action: "discover-models",
                providerType: "gemini_generate_content",
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(diagnosis.fix).toContain("base URL");
        expect(diagnosis.fix).toContain("API key");
        expect(diagnosis.fix).not.toContain("reconnect");
    });

    it("keeps internal failures opaque and points at the gateway's log", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.Internal, "internal error"),
            context,
        );

        expect(diagnosis.kind).toBe("internal");
        expect(diagnosis.fix).toContain("stderr");
    });

    it("treats a rejected field as the gateway's own statement", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure(ErrorCode.InvalidRequest, "`name` must not be empty"),
            context,
        );

        expect(diagnosis.kind).toBe("rejected");
        expect(diagnosis.title.length).toBeGreaterThan(0);
    });

    it("classifies a shape mismatch as a version disagreement", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("contract", "unexpected management API response: providers"),
            context,
        );

        expect(diagnosis.kind).toBe("contract");
        expect(diagnosis.fix).toContain("bindings");
    });

    const reachabilityCases: ReadonlyArray<{
        reachability: Reachability;
        kind: string;
    }> = [
        {
            reachability: "reached",
            kind: "blocked",
        },
        {
            reachability: "unreached",
            kind: "unreachable",
        },
        {
            reachability: "unknown",
            kind: "no-answer",
        },
    ];

    for (const { reachability, kind } of reachabilityCases) {
        it(`reads a silent network failure as ${kind} when reachability is ${reachability}`, () => {
            const diagnosis = diagnoseFailure(
                new ManagementApiError("transport", "Network request failed: Failed to fetch", {
                    cause: new TypeError("Failed to fetch"),
                }),
                {
                    ...context,
                    reachability,
                },
            );

            expect(diagnosis.kind).toBe(kind);
        });
    }

    it("names the CORS setting only once the origin was reached", () => {
        const blocked = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                ...context,
                reachability: "reached",
            },
        );
        const unreached = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                ...context,
                reachability: "unreached",
            },
        );

        expect(blocked.fix).toContain("management_api.cors.allowed_origin");
        expect(blocked.fix).toContain("https://admin.example.com");
        expect(unreached.fix).not.toContain("management_api.cors.allowed_origin");
    });

    it("renders the browser-refused network failures without the browser's jargon", () => {
        for (const reachability of [
            "reached",
            "unreached",
            "unknown",
        ] as const) {
            const diagnosis = diagnoseFailure(
                new ManagementApiError("transport", "Network request failed: Failed to fetch", {
                    cause: new TypeError("Failed to fetch"),
                }),
                {
                    ...context,
                    reachability,
                },
            );

            expect(diagnosis.detail).not.toContain("Failed to fetch");
            expect(diagnosis.detail.length).toBeGreaterThan(0);
        }
    });

    it("renders a same-origin network failure without the browser's jargon", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                baseUrl: "https://gateway.example.com",
                pageOrigin: "https://gateway.example.com",
            },
        );

        expect(diagnosis.kind).toBe("unreachable");
        expect(diagnosis.detail).not.toContain("Failed to fetch");
    });

    it("reports an unshaped HTTP failure with its status", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "HTTP 502", {
                httpStatus: 502,
            }),
            context,
        );

        expect(diagnosis.kind).toBe("gateway");
        expect(diagnosis.detail).toContain("502");
    });

    it("never throws on a failure that is not a management error", () => {
        const diagnosis = diagnoseFailure(new Error("bug"), context);

        expect(diagnosis.kind).toBe("unknown");
        expect(diagnosis.detail).toBe("bug");
    });
});
