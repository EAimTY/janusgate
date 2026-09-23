import { afterEach, describe, expect, it, vi } from "vitest";
import type { Connection } from "/src/connection/model";
import { createManagementApi } from "./client";
import { ManagementApiError } from "./errors";
import { ErrorCode } from "./generated/v0";

/**
 * The client is exercised against a stubbed `fetch`: the wire contract it must
 * honor is the request it builds (method, path, headers, body) and the way it
 * classifies each possible answer. Real transport behavior is covered by the
 * browser-level verification against a running gateway.
 */
type RecordedCall = {
    readonly url: string;
    readonly init: RequestInit;
};

const connection: Connection = {
    baseUrl: "http://127.0.0.1:47182",
    token: "management-token",
};

function stubFetch(handler: (call: RecordedCall) => Promise<Response>): RecordedCall[] {
    const calls: RecordedCall[] = [];

    vi.stubGlobal("fetch", (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
        const call: RecordedCall = {
            url: String(input),
            init: init ?? {},
        };

        calls.push(call);
        return handler(call);
    });

    return calls;
}

function jsonResponse(status: number, payload: unknown): Promise<Response> {
    return Promise.resolve(
        new Response(JSON.stringify(payload), {
            status,
            headers: {
                "Content-Type": "application/json",
            },
        }),
    );
}

function headersOf(call: RecordedCall | undefined): Headers {
    return new Headers(call?.init.headers);
}

afterEach(() => {
    vi.unstubAllGlobals();
});

describe("createManagementApi", () => {
    it("POSTs an empty JSON body to the versioned path with the token verbatim", async () => {
        const calls = stubFetch(() =>
            jsonResponse(200, {
                providers: [],
            }),
        );

        const result = await createManagementApi(connection).listProviders();

        expect(result).toEqual({
            providers: [],
        });

        expect(calls).toHaveLength(1);
        const call = calls[0];
        expect(call?.url).toBe("http://127.0.0.1:47182/management-api/v0/providers:list");
        expect(call?.init.method).toBe("POST");
        expect(call?.init.credentials).toBe("omit");
        expect(call?.init.body).toBe("{}");
        const headers = headersOf(call);
        expect(headers.get("authorization")).toBe("management-token");
        expect(headers.get("content-type")).toBe("application/json");
    });

    it("sends each request payload as the body and forwards the abort signal", async () => {
        const calls = stubFetch(() =>
            jsonResponse(200, {
                models: [],
            }),
        );

        const controller = new AbortController();

        await createManagementApi(connection).listModels(
            {
                provider_id: 7,
            },
            controller.signal,
        );

        expect(calls[0]?.init.body).toBe(
            JSON.stringify({
                provider_id: 7,
            }),
        );

        expect(calls[0]?.init.signal).toBe(controller.signal);
    });

    it("renders the gateway's message verbatim for an errno-shaped failure", async () => {
        stubFetch(() =>
            jsonResponse(409, {
                errno: ErrorCode.Conflict,
                message: "provider name taken",
            }),
        );

        const error = await createManagementApi(connection)
            .createProvider({
                name: "taken",
                model_prefix: "",
                is_enabled: true,
                base_url: "https://upstream.example.com",
                type: "openai_response",
                api_key: "key",
            })
            .catch((failure: unknown) => failure);

        expect(error).toBeInstanceOf(ManagementApiError);
        const apiError = error as ManagementApiError;
        expect(apiError.category).toBe("gateway");
        expect(apiError.errno).toBe(ErrorCode.Conflict);
        expect(apiError.httpStatus).toBe(409);
        expect(apiError.message).toBe("provider name taken");
    });

    it("reports an unauthorized failure to the connection service", async () => {
        stubFetch(() =>
            jsonResponse(401, {
                errno: ErrorCode.Unauthorized,
                message: "invalid token",
            }),
        );

        const notices: string[] = [];

        const error = await createManagementApi(connection, {
            onUnauthorized: (notice) => {
                notices.push(notice);
            },
        })
            .listProviders()
            .catch((failure: unknown) => failure);

        expect((error as ManagementApiError).message).toBe("invalid token");

        expect(notices).toEqual([
            "invalid token",
        ]);
    });

    it("does not report other gateway failures as unauthorized", async () => {
        stubFetch(() =>
            jsonResponse(409, {
                errno: ErrorCode.Conflict,
                message: "taken",
            }),
        );

        const notices: string[] = [];

        await createManagementApi(connection, {
            onUnauthorized: (notice) => {
                notices.push(notice);
            },
        })
            .listProviders()
            .catch(() => undefined);

        expect(notices).toEqual([]);
    });

    it("reports the HTTP status alone when the body is not a management error", async () => {
        stubFetch(() =>
            Promise.resolve(
                new Response("<html>not found</html>", {
                    status: 404,
                    headers: {
                        "Content-Type": "text/html",
                    },
                }),
            ),
        );

        const error = await createManagementApi(connection)
            .listProviders()
            .catch((failure: unknown) => failure);

        expect(error).toBeInstanceOf(ManagementApiError);
        const apiError = error as ManagementApiError;
        expect(apiError.category).toBe("transport");
        expect(apiError.httpStatus).toBe(404);
        expect(apiError.message).toBe("HTTP 404");
    });

    it("reports a 2xx body that is not JSON as a transport failure", async () => {
        stubFetch(() =>
            Promise.resolve(
                new Response("<html>some other service</html>", {
                    status: 200,
                    headers: {
                        "Content-Type": "text/html",
                    },
                }),
            ),
        );

        const error = await createManagementApi(connection)
            .listProviders()
            .catch((failure: unknown) => failure);

        expect(error).toBeInstanceOf(ManagementApiError);
        const apiError = error as ManagementApiError;
        expect(apiError.category).toBe("transport");
        expect(apiError.message).toBe("HTTP 200");
    });

    it("addresses the api-keys and model creation routes", async () => {
        const calls = stubFetch((call) => {
            if (call.url.endsWith("/api-keys:list")) {
                return jsonResponse(200, {
                    api_keys: [],
                });
            }

            if (call.url.endsWith("/models:create")) {
                return jsonResponse(200, {
                    model: {
                        id: 9,
                        provider_id: 4,
                        provider: "local",
                        upstream_id: "gpt-4o",
                        public_id: "local-gpt-4o",
                        id_override: null,
                        is_enabled: true,
                        discovery_state: "discovered",
                    },
                });
            }

            if (call.url.endsWith("/usage:list")) {
                return jsonResponse(200, {
                    api_logs: [],
                });
            }

            if (call.url.endsWith("/usage:query")) {
                return jsonResponse(200, {
                    buckets: [],
                });
            }

            if (call.url.endsWith("/api-keys:create")) {
                return jsonResponse(200, {
                    api_key: {
                        id: 7,
                        name: "service-a",
                        key: "sk-live",
                        is_enabled: true,
                        created_at: 1_700_000_000_000_000,
                    },
                });
            }

            return jsonResponse(200, {});
        });

        const api = createManagementApi(connection);
        await api.listApiKeys();
        await api.createModel({
            provider_id: 4,
            upstream_id: "gpt-4o",
            id_override: null,
            is_enabled: true,
        });
        await api.deleteModel({
            model_id: 9,
        });
        await api.createApiKey({
            name: "service-a",
            key: "sk-live",
            is_enabled: true,
        });
        await api.updateApiKey({
            api_key_id: 7,
            name: "service-a",
            key: "sk-live",
            is_enabled: false,
        });
        await api.deleteApiKey({
            api_key_id: 7,
        });
        await api.listUsage({});
        await api.queryUsage({
            group_by: [
                "day",
            ],
        });

        expect(calls.map((call) => call.url)).toEqual([
            "http://127.0.0.1:47182/management-api/v0/api-keys:list",
            "http://127.0.0.1:47182/management-api/v0/models:create",
            "http://127.0.0.1:47182/management-api/v0/models:delete",
            "http://127.0.0.1:47182/management-api/v0/api-keys:create",
            "http://127.0.0.1:47182/management-api/v0/api-keys:update",
            "http://127.0.0.1:47182/management-api/v0/api-keys:delete",
            "http://127.0.0.1:47182/management-api/v0/usage:list",
            "http://127.0.0.1:47182/management-api/v0/usage:query",
        ]);

        expect(calls.map((call) => JSON.parse(String(call.init.body)))).toEqual([
            {},
            {
                provider_id: 4,
                upstream_id: "gpt-4o",
                id_override: null,
                is_enabled: true,
            },
            {
                model_id: 9,
            },
            {
                name: "service-a",
                key: "sk-live",
                is_enabled: true,
            },
            {
                api_key_id: 7,
                name: "service-a",
                key: "sk-live",
                is_enabled: false,
            },
            {
                api_key_id: 7,
            },
            {},
            {
                group_by: [
                    "day",
                ],
            },
        ]);
    });

    it("resolves a route whose response carries no fact as void", async () => {
        stubFetch(() => jsonResponse(200, {}));

        await expect(
            createManagementApi(connection).updateProvider({
                type: "codex_oauth",
                provider_id: 5,
                name: "chatgpt",
                model_prefix: "",
                is_enabled: true,
            }),
        ).resolves.toBeUndefined();
    });

    it("reports a contract violation when a 2xx body does not match the bindings", async () => {
        stubFetch(() =>
            jsonResponse(200, {
                providers: [
                    {
                        id: "one",
                    },
                ],
            }),
        );

        const error = await createManagementApi(connection)
            .listProviders()
            .catch((failure: unknown) => failure);

        expect(error).toBeInstanceOf(ManagementApiError);
        const apiError = error as ManagementApiError;
        expect(apiError.category).toBe("contract");
        expect(apiError.message).toContain("response.providers[0].type");
    });

    it("reports a network failure as a transport failure", async () => {
        stubFetch(() => Promise.reject(new TypeError("fetch failed")));

        const error = await createManagementApi(connection)
            .listProviders()
            .catch((failure: unknown) => failure);

        expect(error).toBeInstanceOf(ManagementApiError);
        const apiError = error as ManagementApiError;
        expect(apiError.category).toBe("transport");
        expect(apiError.message).toBe("Network request failed: fetch failed");
    });

    it("propagates an abort instead of rendering it as a failure", async () => {
        stubFetch(
            (call) =>
                new Promise<Response>((_resolve, reject) => {
                    call.init.signal?.addEventListener("abort", () => {
                        reject(new DOMException("The operation was aborted.", "AbortError"));
                    });
                }),
        );

        const controller = new AbortController();
        const pending = createManagementApi(connection).listProviders(controller.signal);
        controller.abort();

        const error = await pending.catch((failure: unknown) => failure);

        expect(error).not.toBeInstanceOf(ManagementApiError);
        expect(Reflect.get(error as object, "name")).toBe("AbortError");
    });
});
