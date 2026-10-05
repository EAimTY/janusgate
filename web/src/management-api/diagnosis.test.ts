import { type JSX, render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { gatewayFailure } from "../test-support/management-api";
import type { ActionContext, Reachability } from "./diagnosis";
import { diagnoseFailure, probeReachability } from "./diagnosis";
import { ManagementApiError } from "./errors";
import { ErrorCode } from "./generated/v0";

const context: ActionContext = {
    baseUrl: "https://gateway.example.com",
    pageOrigin: "https://admin.example.com",
};

function textOf(value: JSX.Element): string {
    const host = document.createElement("div");
    const dispose = render(() => value, host);
    const text = host.textContent ?? "";
    dispose();
    return text;
}

describe("diagnoseFailure", () => {
    it("preserves the gateway's own message as the detail of every rendered failure", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("upstream answered 401", ErrorCode.UpstreamError, 400),
            context,
        );

        expect(diagnosis.detail).toBe("upstream answered 401");
        expect(diagnosis.kind).toBe("upstream");
    });

    it("names the token's source for an unauthorized answer", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("invalid management token", ErrorCode.Unauthorized, 400),
            context,
        );

        expect(diagnosis.kind).toBe("unauthorized");
        expect(textOf(diagnosis.fix)).toContain("management_api.token");
    });

    it("reports a gone row as having no recovery on this screen", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("no such provider", ErrorCode.NotFound, 400),
            context,
        );

        expect(diagnosis.kind).toBe("gone");
        expect(diagnosis.fix).toBeNull();
    });

    it("names the taken identifier for a conflict", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("provider name is taken", ErrorCode.Conflict, 400),
            {
                ...context,
                action: "create-provider",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(textOf(diagnosis.fix)).toBe("Choose a provider name no other row holds.");
    });

    it("points a creation flow's completion conflict at Start over, the one control the live flow leaves reachable", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("provider name is taken", ErrorCode.Conflict, 400),
            {
                ...context,
                action: "complete-oauth",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(textOf(diagnosis.fix)).toBe(
            "Another provider took this name before the flow could finish. Start over ends this flow and unlocks the name; then create again with a name no other row holds.",
        );
    });

    it("points a definition-write conflict at the moved stored version, not the provider name", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("provider definition version moved", ErrorCode.Conflict, 400),
            {
                ...context,
                action: "ensure-definition",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(textOf(diagnosis.fix)).toBe(
            "The stored version moved — use Refresh in the bar above; it re-reads the stored versions and leaves this form untouched — then submit again.",
        );
    });

    it("names the existing physical inventory for a model creation conflict", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("public id taken", ErrorCode.Conflict, 400),
            {
                ...context,
                action: "create-model",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(textOf(diagnosis.fix)).toContain("exact upstream model");
    });

    it("points a taken key secret at generating a fresh one", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("api key taken", ErrorCode.Conflict, 400),
            {
                ...context,
                action: "create-api-key",
            },
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(textOf(diagnosis.fix)).toContain("fresh one");
    });

    it("keeps an upstream refusal during discovery agnostic when no provider type is known", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("upstream answered 502", ErrorCode.UpstreamError, 400),
            {
                ...context,
                action: "discover-models",
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(textOf(diagnosis.fix)).toContain("provider's state");
    });

    it("offers an OAuth provider's discovery refusal the reconnect and the manual add without claiming the cause", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("upstream answered 502", ErrorCode.UpstreamError, 400),
            {
                ...context,
                action: "discover-models",
                isOAuthProvider: true,
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(textOf(diagnosis.fix)).toContain("If the provider's authorization lapsed");
        expect(textOf(diagnosis.fix)).toContain("reconnect");
        expect(textOf(diagnosis.fix)).toContain("no models yet");
        expect(textOf(diagnosis.fix)).toContain("add one by hand");
        expect(textOf(diagnosis.fix)).not.toContain("API key");
    });

    it("offers a static-key provider's discovery refusal the credential fix and the manual add without claiming the cause", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("upstream answered 502", ErrorCode.UpstreamError, 400),
            {
                ...context,
                action: "discover-models",
                isOAuthProvider: false,
            },
        );

        expect(diagnosis.kind).toBe("upstream");
        expect(textOf(diagnosis.fix)).toContain("base URL");
        expect(textOf(diagnosis.fix)).toContain("API key");
        expect(textOf(diagnosis.fix)).toContain("no models yet");
        expect(textOf(diagnosis.fix)).toContain("add one by hand");
        expect(textOf(diagnosis.fix)).not.toContain("reconnect");
    });

    it("keeps internal failures opaque and points at the gateway's log", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("internal error", ErrorCode.Internal, 400),
            context,
        );

        expect(diagnosis.kind).toBe("internal");
        expect(textOf(diagnosis.fix)).toContain("stderr");
    });

    it("treats a rejected field as the gateway's own statement", () => {
        const diagnosis = diagnoseFailure(
            gatewayFailure("`name` must not be empty", ErrorCode.InvalidRequest, 400),
            context,
        );

        expect(diagnosis.kind).toBe("rejected");
        expect(textOf(diagnosis.title).length).toBeGreaterThan(0);
    });

    it("classifies a shape mismatch as a version disagreement", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("contract", "unexpected management API response: providers"),
            context,
        );

        expect(diagnosis.kind).toBe("contract");
        expect(textOf(diagnosis.fix)).toContain("bindings");
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

        expect(textOf(blocked.fix)).toContain("management_api.cors.allowed_origin");
        expect(textOf(blocked.fix)).toContain("https://admin.example.com");
        expect(textOf(unreached.fix)).not.toContain("management_api.cors.allowed_origin");
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

    it("diagnoses an HTTPS page's refused call to an HTTP address as blocked mixed content", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch", {
                cause: new TypeError("Failed to fetch"),
            }),
            {
                baseUrl: "http://gateway.example.com",
                pageOrigin: "https://admin.example.com",
            },
        );

        expect(diagnosis.kind).toBe("mixed-content");
        expect(diagnosis.rawDetail).toBe("Network request failed: Failed to fetch");
        expect(textOf(diagnosis.title)).toBe(
            "An HTTPS page cannot call the HTTP address http://gateway.example.com.",
        );
        expect(diagnosis.detail).toBe(
            "This page is served over HTTPS, and the browser blocks its calls to HTTP addresses outright, so the gateway was never contacted.",
        );
        expect(textOf(diagnosis.fix)).toBe(
            "Serve this app over HTTP as well, or add a `tls` section to that instance's configuration file so the gateway answers HTTPS.",
        );
    });

    it("spares loopback HTTP addresses the mixed-content diagnosis the browser does not impose", () => {
        for (const loopbackAddress of [
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://[::1]:8080",
        ] as const) {
            const diagnosis = diagnoseFailure(
                new ManagementApiError("transport", "Network request failed: Failed to fetch"),
                {
                    baseUrl: loopbackAddress,
                    pageOrigin: "https://admin.example.com",
                },
            );

            expect(diagnosis.kind).toBe("no-answer");
        }
    });

    it("keeps the mixed-content certainty for a 127-prefixed domain that is not the loopback", () => {
        // `127.evil.com` only borrows the prefix: it is a public domain, not
        // the normalized dotted-quad shape the URL parser gives the
        // 127.0.0.0/8 loopback, so the browser blocks it like any HTTP
        // address and the diagnosis is certain without a probe.
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                baseUrl: "http://127.evil.com",
                pageOrigin: "https://admin.example.com",
            },
        );

        expect(diagnosis.kind).toBe("mixed-content");
        expect(diagnosis.rawDetail).toBe("Network request failed: Failed to fetch");
    });

    it("reads a trailing-dot loopback host as the loopback it resolves to", () => {
        // WHATWG keeps the dot on the hostname while the browser resolves it
        // like the bare name, so the comparison reads past it.
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                baseUrl: "http://localhost./",
                pageOrigin: "https://admin.example.com",
            },
        );

        expect(diagnosis.kind).toBe("no-answer");
    });

    it("keeps an HTTP page's call to an HTTP address on the probe's readings", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed: Failed to fetch"),
            {
                baseUrl: "http://gateway.example.com",
                pageOrigin: "http://admin.example.com",
                reachability: "unreached",
            },
        );

        expect(diagnosis.kind).toBe("unreachable");
        expect(textOf(diagnosis.title)).toBe("Nothing answered at http://gateway.example.com.");
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

describe("probeReachability", () => {
    afterEach(() => {
        vi.unstubAllGlobals();
    });

    it("answers unknown for an aborted probe, not unreached", async () => {
        // The signal must reach the request for a cancellation to end it:
        // the stub only rejects once the carried signal fires, so a dropped
        // signal would hang this test rather than pass it.
        const controller = new AbortController();
        vi.stubGlobal(
            "fetch",
            (_url: unknown, init?: RequestInit) =>
                new Promise<never>((_resolve, reject) => {
                    init?.signal?.addEventListener("abort", () => {
                        reject(new DOMException("This operation was aborted", "AbortError"));
                    });
                }),
        );

        const pending = probeReachability("https://gateway.example.com", controller.signal);
        controller.abort();

        await expect(pending).resolves.toBe("unknown");
    });

    it("reads a deadline's TimeoutError expiry as unknown, never as nothing answered", async () => {
        // `AbortSignal.timeout` aborts with a `TimeoutError` reason, which
        // the fetch then rejects with — a shape that must stay distinct from
        // the network's own `TypeError` refusal, or an expired deadline would
        // sharpen "no answer yet" into "nothing answered".
        vi.stubGlobal("fetch", () =>
            Promise.reject(new DOMException("The signal timed out", "TimeoutError")),
        );

        await expect(probeReachability("https://gateway.example.com")).resolves.toBe("unknown");
    });
});
