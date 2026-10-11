import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import { createManagementApi } from "./client";
import type { Response as RouteResponse } from "./generated/v0";
import { ErrorCode } from "./generated/v0";

/**
 * The management client's transport boundary, against a stubbed `fetch`: the
 * request's composition (path, method, verbatim token, JSON body), the
 * response's narrowing through the route's own parser, and the failure
 * classification — transport for everything no management error body
 * explains, gateway for a rendered errno body, contract for a 2xx body that
 * fails its shape, with the `unauthorized` errno additionally reported.
 */

const CONNECTION = {
    baseUrl: "http://gateway.test:8787",
    token: "raw-management-token",
};

/** One static-key provider row, as the providers listing parser accepts it. */
const API_KEY_PROVIDER_ROW = {
    type: "api_key",
    id: 7,
    name: "main",
    model_prefix: "main",
    is_enabled: true,
    provider_definition_name: "generic-openai-responses",
    provider_definition_version: 2,
    format: "openai_chat_completion",
    base_url: null,
};

type FetchCall = {
    readonly url: string;
    readonly init: RequestInit;
};

/**
 * Installs a fetch stub answering every call with the given status and body,
 * recording the calls it saw.
 */
function stubFetch(
    respond: (call: FetchCall) => {
        status: number;
        body: string;
    },
): {
    readonly calls: FetchCall[];
} {
    const calls: FetchCall[] = [];

    vi.stubGlobal(
        "fetch",
        vi.fn(async (url: string | URL, init: RequestInit) => {
            const call = {
                url: String(url),
                init,
            };
            calls.push(call);
            const answer = respond(call);

            return new Response(answer.body, {
                status: answer.status,
            });
        }),
    );

    return {
        calls,
    };
}

beforeEach(() => {
    vi.unstubAllGlobals();
});

afterEach(() => {
    vi.unstubAllGlobals();
});

describe("createManagementApi request composition", () => {
    test("composes the versioned path onto the base URL and posts JSON with the verbatim token", async () => {
        const { calls } = stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                providers: [
                    API_KEY_PROVIDER_ROW,
                ],
            }),
        }));

        const api = createManagementApi(CONNECTION);
        await api.listProviders();

        expect(calls).toHaveLength(1);
        const call = calls[0];
        if (call === undefined) throw new Error("the stub recorded no call");
        expect(call.url).toBe("http://gateway.test:8787/management-api/v0/providers:list");
        expect(call.init.method).toBe("POST");
        const headers = new Headers(call.init.headers);
        // The token travels verbatim: no `Bearer` prefix.
        expect(headers.get("Authorization")).toBe("raw-management-token");
        expect(headers.get("Content-Type")).toBe("application/json");
        expect(call.init.credentials).toBe("omit");
    });

    test("routes whose callers send no arguments build the fixed empty body", async () => {
        const { calls } = stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                providers: [],
            }),
        }));

        await createManagementApi(CONNECTION).listProviders();

        const call = calls[0];
        if (call === undefined) throw new Error("the stub recorded no call");
        expect(call.init.body).toBe("{}");
    });

    test("routes whose caller supplies the body send it verbatim", async () => {
        const { calls } = stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                provider: API_KEY_PROVIDER_ROW,
            }),
        }));

        const request = {
            model_prefix: "acme",
            provider_definition_name: "generic-openai-responses",
            provider_definition_version: 2,
            name: "acme",
            is_enabled: true,
            base_url: null,
            api_key: "sk-static",
        };
        await createManagementApi(CONNECTION).createProvider(request);

        const call = calls[0];
        if (call === undefined) throw new Error("the stub recorded no call");
        expect(JSON.parse(call.init.body as string)).toEqual(request);
    });
});

describe("createManagementApi response narrowing", () => {
    test("narrows a success body through the route's parser into typed rows", async () => {
        stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                providers: [
                    API_KEY_PROVIDER_ROW,
                ],
            }),
        }));

        const providers = await createManagementApi(CONNECTION).listProviders();

        expect(providers.providers).toHaveLength(1);
        const row = providers.providers[0];
        if (row === undefined) throw new Error("the fixture listed no row");
        expect(row.type).toBe("api_key");
        expect(row.id).toBe(7);
    });

    test("resolves the empty-record writes as void, still parsing the body", async () => {
        stubFetch(() => ({
            status: 200,
            body: JSON.stringify({}),
        }));

        const request = {
            provider_id: 7,
            name: "renamed",
            model_prefix: "renamed",
            is_enabled: true,
            base_url: null,
        };

        await expect(
            createManagementApi(CONNECTION).updateProvider(request),
        ).resolves.toBeUndefined();
    });

    test("rejects a void route's foreign 2xx body as a contract failure", async () => {
        stubFetch(() => ({
            status: 200,
            body: JSON.stringify([
                1,
                2,
            ]),
        }));

        const request = {
            provider_id: 7,
            name: "renamed",
            model_prefix: "renamed",
            is_enabled: true,
            base_url: null,
        };

        await expect(createManagementApi(CONNECTION).updateProvider(request)).rejects.toThrowError(
            "Unexpected management API response",
        );
    });

    test("forwards the abort signal to the transport", async () => {
        const { calls } = stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                providers: [],
            }),
        }));
        const controller = new AbortController();

        await createManagementApi(CONNECTION).listProviders(controller.signal);

        expect(calls[0]?.init.signal).toBe(controller.signal);
    });

    test("propagates an aborted fetch as the abort itself, never a wrapped failure", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => {
                throw new DOMException("The operation was aborted.", "AbortError");
            }),
        );

        await expect(createManagementApi(CONNECTION).listProviders()).rejects.toThrowError(
            "The operation was aborted.",
        );
    });
});

describe("createManagementApi failure classification", () => {
    test("classifies a network refusal as transport, retaining the base URL", async () => {
        vi.stubGlobal(
            "fetch",
            vi.fn(async () => {
                throw new TypeError("fetch failed");
            }),
        );

        const failure = await createManagementApi(CONNECTION)
            .listProviders()
            .then(
                () => null,
                (error: unknown) => error,
            );

        expect(failure).toBeInstanceOf(Error);
        const message = (
            failure as {
                message: string;
            }
        ).message;
        expect(message).toContain("Network request failed");
        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("transport");
        expect(
            (
                failure as {
                    httpStatus: number | null;
                }
            ).httpStatus,
        ).toBeNull();
        expect(
            (
                failure as {
                    baseUrl: string | null;
                }
            ).baseUrl,
        ).toBe("http://gateway.test:8787");
    });

    test("classifies a non-JSON error body as transport carrying only the status", async () => {
        stubFetch(() => ({
            status: 502,
            body: "<html>Bad Gateway</html>",
        }));

        const failure = await createManagementApi(CONNECTION)
            .listProviders()
            .then(
                () => null,
                (error: unknown) => error,
            );

        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("transport");
        expect(
            (
                failure as {
                    httpStatus: number | null;
                }
            ).httpStatus,
        ).toBe(502);
    });

    test("classifies a rendered errno body as gateway with its errno and message", async () => {
        stubFetch(() => ({
            status: 404,
            body: JSON.stringify({
                errno: ErrorCode.NotFound,
                message: "no such provider row",
            }),
        }));

        const failure = await createManagementApi(CONNECTION)
            .listProviders()
            .then(
                () => null,
                (error: unknown) => error,
            );

        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("gateway");
        expect(
            (
                failure as {
                    errno: ErrorCode | null;
                }
            ).errno,
        ).toBe(ErrorCode.NotFound);
        expect(
            (
                failure as {
                    message: string;
                }
            ).message,
        ).toBe("no such provider row");
        expect(
            (
                failure as {
                    httpStatus: number | null;
                }
            ).httpStatus,
        ).toBe(404);
    });

    test("reports a foreign errno body as transport, not gateway", async () => {
        stubFetch(() => ({
            status: 400,
            body: JSON.stringify({
                errno: 999,
                message: "not the taxonomy",
            }),
        }));

        const failure = await createManagementApi(CONNECTION)
            .listProviders()
            .then(
                () => null,
                (error: unknown) => error,
            );

        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("transport");
    });

    test("reports the unauthorized errno to onUnauthorized while still rejecting", async () => {
        stubFetch(() => ({
            status: 401,
            body: JSON.stringify({
                errno: ErrorCode.Unauthorized,
                message: "the token is dead",
            }),
        }));

        const notices: string[] = [];
        const api = createManagementApi(CONNECTION, {
            onUnauthorized: (notice) => {
                notices.push(notice);
            },
        });

        const failure = await api.listProviders().then(
            () => null,
            (error: unknown) => error,
        );

        expect(notices).toEqual([
            "the token is dead",
        ]);
        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("gateway");
        expect(
            (
                failure as {
                    errno: ErrorCode | null;
                }
            ).errno,
        ).toBe(ErrorCode.Unauthorized);
    });

    test("classifies a 2xx body failing its shape as contract", async () => {
        stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                providers: "not-an-array",
            }),
        }));

        const failure = await createManagementApi(CONNECTION)
            .listProviders()
            .then(
                () => null,
                (error: unknown) => error,
            );

        expect(
            (
                failure as {
                    category: string;
                }
            ).category,
        ).toBe("contract");
        expect(
            (
                failure as {
                    message: string;
                }
            ).message,
        ).toContain("Unexpected management API response");
    });
});

describe("createManagementApi route surface", () => {
    test("answers each route on its own management path", async () => {
        const { calls } = stubFetch(() => ({
            status: 200,
            body: JSON.stringify({}),
        }));

        const api = createManagementApi(CONNECTION);
        await api.updateApiKey({
            api_key_id: 1,
            name: "k",
            key: "secret-1",
            is_enabled: true,
            access_control: {
                default_access: "deny",
                owner_defaults: [],
                route_overrides: [],
            },
        });
        await api.deleteModel({
            model_id: 3,
        });
        await api.resetProviderAvailability({
            provider_id: 5,
        });
        await api.deleteProviderPool({
            provider_pool_id: 9,
        });

        expect(calls.map((call) => call.url)).toEqual([
            "http://gateway.test:8787/management-api/v0/api-keys:update",
            "http://gateway.test:8787/management-api/v0/models:delete",
            "http://gateway.test:8787/management-api/v0/providers:reset-availability",
            "http://gateway.test:8787/management-api/v0/provider-pools:delete",
        ]);
    });

    test("types every route's response as the generated bindings' own", async () => {
        stubFetch(() => ({
            status: 200,
            body: JSON.stringify({
                provider_pool_id: 4,
            }),
        }));

        const created: RouteResponse<"/provider-pools:create", "POST"> = await createManagementApi(
            CONNECTION,
        ).createProviderPool({
            name: "fast",
            model_prefix: "fast",
            is_enabled: true,
            members: [
                {
                    provider_id: 1,
                    priority: 0,
                },
            ],
        });

        expect(created.provider_pool_id).toBe(4);
    });
});
