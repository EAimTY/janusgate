import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { type ActionName, diagnoseFailure, probeReachability } from "./diagnosis";
import { ManagementApiError } from "./errors";
import { ErrorCode } from "./generated/v0";

/**
 * The failure panel's wording decisions: the gateway's message kept verbatim
 * as the detail wherever the gateway rendered one, the kind selecting the
 * explanation (errno kinds, the contract kind, the HTTP-status kind, the
 * network kinds), the certainty rule over mixed content and reachability —
 * a deterministic browser block needs no probe, and a deadline is never read
 * as an answer about the network — and the recovery named only where the app
 * can be certain of one.
 */

const PAGE_ORIGIN = "https://console.example.test";

/** The context every diagnosis below starts from. */
function context(overrides: { action?: ActionName; baseUrl?: string } = {}) {
    return {
        baseUrl: "http://gateway.example.test:8787",
        pageOrigin: PAGE_ORIGIN,
        ...overrides,
    };
}

beforeEach(() => {
    vi.unstubAllGlobals();
});

afterEach(() => {
    vi.unstubAllGlobals();
});

describe("gateway-category diagnoses", () => {
    test("keeps the gateway's message verbatim as the detail for every errno", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("gateway", "the pool name is taken", {
                errno: ErrorCode.Conflict,
                httpStatus: 409,
            }),
            context({
                action: "create-pool",
            }),
        );

        expect(diagnosis.kind).toBe("conflict");
        expect(diagnosis.detail).toBe("the pool name is taken");
    });

    test("specializes the conflict fix per action and falls back to the shared wording", () => {
        const pool = diagnoseFailure(
            conflict(),
            context({
                action: "create-pool",
            }),
        );
        const model = diagnoseFailure(
            conflict(),
            context({
                action: "create-model",
            }),
        );
        const plain = diagnoseFailure(conflict(), context());

        expect(String(pool.fix)).not.toBe(String(plain.fix));
        expect(String(model.fix)).not.toBe(String(plain.fix));
    });

    test("maps each known errno to its own kind", () => {
        const kindOf = (errno: ErrorCode): string =>
            diagnoseFailure(
                new ManagementApiError("gateway", "m", {
                    errno,
                    httpStatus: 400,
                }),
                context(),
            ).kind;

        expect(kindOf(ErrorCode.Unauthorized)).toBe("unauthorized");
        expect(kindOf(ErrorCode.NotFound)).toBe("gone");
        expect(kindOf(ErrorCode.Conflict)).toBe("conflict");
        expect(kindOf(ErrorCode.UpstreamError)).toBe("upstream");
        expect(kindOf(ErrorCode.Internal)).toBe("internal");
    });

    test("answers the generic rejected kind for a foreign errno and states no fix", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("gateway", "unsupported media type", {
                errno: ErrorCode.UnsupportedMediaType,
                httpStatus: 415,
            }),
            context(),
        );

        expect(diagnosis.kind).toBe("rejected");
        expect(diagnosis.fix).toBeNull();
    });

    test("specializes the upstream fix on the discovery action by credential kind", () => {
        const plainContext = context({
            action: "discover-models",
        });
        const oauthContext = {
            ...context({
                action: "discover-models",
            }),
            isOAuthProvider: true,
        };
        const staticContext = {
            ...context({
                action: "discover-models",
            }),
            isOAuthProvider: false,
        };

        // Without a known credential kind the fix claims neither recovery;
        // with one it names exactly that recovery.
        const plain = String(diagnoseFailure(upstream(), plainContext).fix);
        const oauth = String(diagnoseFailure(upstream(), oauthContext).fix);
        const staticKey = String(diagnoseFailure(upstream(), staticContext).fix);

        expect(plain).not.toBe(oauth);
        expect(oauth).not.toBe(staticKey);
        expect(plain).not.toBe(staticKey);
        // Non-discovery actions share the credentials-and-reachability wording.
        expect(
            String(
                diagnoseFailure(
                    upstream(),
                    context({
                        action: "connect",
                    }),
                ).fix,
            ),
        ).toBe(String(diagnoseFailure(upstream(), context()).fix));
    });
});

describe("contract-category diagnosis", () => {
    test("answers the contract kind naming the bindings disagreement", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("contract", "Unexpected management API response: boom"),
            context(),
        );

        expect(diagnosis.kind).toBe("contract");
        expect(diagnosis.detail).toContain("Unexpected management API response");
    });
});

describe("transport-category diagnoses", () => {
    test("answers an answered-but-unshaped body with the status wording", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "HTTP 502", {
                httpStatus: 502,
            }),
            context(),
        );

        expect(diagnosis.kind).toBe("gateway");
        expect(String(diagnosis.title)).toContain("502");
        expect(diagnosis.detail).toBe("HTTP 502");
    });

    test("answers a cross-origin refusal with no reachability evidence as no-answer", () => {
        // An https target: cross-origin from the page, but never a
        // mixed-content block, so the kind stays open for the probe.
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed"),
            context({
                baseUrl: "https://gateway.example.test:8787",
            }),
        );

        expect(diagnosis.kind).toBe("no-answer");
    });

    test("answers a truly same-origin refusal as unreachable", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed"),
            {
                baseUrl: "https://console.example.test",
                pageOrigin: "https://console.example.test",
            },
        );

        expect(diagnosis.kind).toBe("unreachable");
    });

    test("answers the one certain refusal — https page calling a non-loopback http address — as mixed content", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed"),
            context({
                baseUrl: "http://intranet.example.test:9000",
            }),
        );

        expect(diagnosis.kind).toBe("mixed-content");
    });

    test("exempts the loopback addresses from the mixed-content verdict", () => {
        for (const host of [
            "localhost",
            "localhost.",
            "127.0.0.1",
            "127.9.9.9",
            "0.0.0.0",
            "[::1]",
        ]) {
            const diagnosis = diagnoseFailure(
                new ManagementApiError("transport", "Network request failed"),
                context({
                    baseUrl: `http://${host}:8787`,
                }),
            );

            // Over the exemption the probe decides: without one the kind stays open.
            expect(diagnosis.kind).toBe("no-answer");
        }
    });

    test("never claims mixed content for an http page or an https address", () => {
        expect(
            diagnoseFailure(new ManagementApiError("transport", "Network request failed"), {
                baseUrl: "http://gw.example.test",
                pageOrigin: "http://console.example.test",
            }).kind,
        ).not.toBe("mixed-content");
        expect(
            diagnoseFailure(new ManagementApiError("transport", "Network request failed"), {
                baseUrl: "https://gw.example.test",
                pageOrigin: "https://console.example.test",
            }).kind,
        ).not.toBe("mixed-content");
    });

    test("refines a no-answer verdict by the probe's reachability reading", () => {
        const refused = new ManagementApiError("transport", "Network request failed");
        const crossOrigin = context({
            baseUrl: "https://gateway.example.test:8787",
        });

        expect(
            diagnoseFailure(refused, {
                ...crossOrigin,
                reachability: "reached",
            }).kind,
        ).toBe("blocked");
        expect(
            diagnoseFailure(refused, {
                ...crossOrigin,
                reachability: "unreached",
            }).kind,
        ).toBe("unreachable");
        expect(
            diagnoseFailure(refused, {
                ...crossOrigin,
                reachability: "unknown",
            }).kind,
        ).toBe("no-answer");
    });

    test("answers an unparsable address as unreachable — CORS is never the explanation offered", () => {
        const diagnosis = diagnoseFailure(
            new ManagementApiError("transport", "Network request failed"),
            context({
                baseUrl: "not a url",
            }),
        );

        expect(diagnosis.kind).toBe("unreachable");
    });
});

describe("unknown failures", () => {
    test("answers a non-management error as the unknown kind with its message", () => {
        const diagnosis = diagnoseFailure(new Error("boom"), context());

        expect(diagnosis.kind).toBe("unknown");
        expect(diagnosis.detail).toBe("boom");
        expect(diagnosis.fix).toBeNull();
    });

    test("renders a non-error value through String", () => {
        expect(diagnoseFailure(7, context()).detail).toBe("7");
    });
});

describe("probeReachability", () => {
    test("reads an answered no-CORS request as reached", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(
                async () =>
                    new Response(null, {
                        status: 200,
                    }),
            ),
        );

        await expect(probeReachability("http://gw.example.test")).resolves.toBe("reached");
    });

    test("reads a TypeError refusal as unreached", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => {
                throw new TypeError("fetch failed");
            }),
        );

        await expect(probeReachability("http://gw.example.test")).resolves.toBe("unreached");
    });

    test("never reads an aborted probe as an answer", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => {
                throw new DOMException("aborted", "AbortError");
            }),
        );

        await expect(probeReachability("http://gw.example.test")).resolves.toBe("unknown");
    });

    test("probes the origin root without credentials, cache, or CORS mode", async () => {
        const stub = vi.fn(
            async () =>
                new Response(null, {
                    status: 200,
                }),
        );
        vi.stubGlobal("fetch", stub);

        await probeReachability("http://gw.example.test:8787");

        expect(stub).toHaveBeenCalledWith(
            "http://gw.example.test:8787/",
            expect.objectContaining({
                method: "GET",
                mode: "no-cors",
                cache: "no-store",
                credentials: "omit",
            }),
        );
    });
});

/** One conflict-category gateway error, the fixture the fix wording starts from. */
function conflict(): ManagementApiError {
    return new ManagementApiError("gateway", "taken", {
        errno: ErrorCode.Conflict,
        httpStatus: 409,
    });
}

/** One upstream-error-category gateway error. */
function upstream(): ManagementApiError {
    return new ManagementApiError("gateway", "upstream refused", {
        errno: ErrorCode.UpstreamError,
        httpStatus: 502,
    });
}
