import { createRoot, createSignal } from "solid-js";
import { describe, expect, it, vi } from "vitest";
import type {
    DefinitionResponse,
    GetProviderQuotaResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { PROVIDER_PRESETS } from "/src/presets";
import { createProviderQuotaView } from "./quota-view";
import { createDisposals, providerRow, settle, stubManagementApi } from "./screen-test-support";

const disposals = createDisposals();
const preset = PROVIDER_PRESETS.find((value) => value.id === "codex");
if (preset === undefined) throw new Error("Codex fixture is required");
const definition: DefinitionResponse = {
    ...preset.payload,
    name: "exact",
    version: 2,
    created_at: 1,
};
const provider = providerRow({
    provider_definition_name: "exact",
    provider_definition_version: 2,
});
const snapshot: GetProviderQuotaResponse = {
    observed_at: 123,
    quotas: [
        {
            key: [
                "pool",
                "usage",
            ],
            label: "Pool",
            scope: {
                type: "unknown",
            },
            window: {
                type: "unknown",
            },
            measurement: {
                type: "ratio",
                used_fraction: 0.5,
            },
            reset: {
                type: "unknown",
            },
        },
    ],
};

function deferred<T>(): {
    promise: Promise<T>;
    resolve: (value: T) => void;
    reject: (cause: unknown) => void;
} {
    let resolve: (value: T) => void = () => {
        throw new Error("not initialized");
    };
    let reject: (cause: unknown) => void = () => {
        throw new Error("not initialized");
    };
    const promise = new Promise<T>((answer, refuse) => {
        resolve = answer;
        reject = refuse;
    });
    return {
        promise,
        resolve,
        reject,
    };
}

describe("quota mounted identity and refresh lifecycle", () => {
    it("starts fresh after credential-edit navigation and aborts disposed identities", async () => {
        const late = deferred<GetProviderQuotaResponse>();
        const signals: Array<AbortSignal | undefined> = [];
        const get = vi.fn((_request, signal?: AbortSignal) => {
            signals.push(signal);
            return signals.length === 1
                ? late.promise
                : Promise.resolve({
                      observed_at: 999,
                      quotas: [],
                  });
        });
        const api = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    definition,
                ],
            }),
            getProviderQuota: get,
        });
        const mount = () =>
            createRoot((dispose) => {
                disposals.push(dispose);
                return createProviderQuotaView({
                    api: () => api,
                    provider: () => provider,
                });
            });
        const beforeEdit = mount();
        await settle();
        disposals.pop()?.();
        expect(signals[0]?.aborted).toBe(true);
        const afterEdit = mount();
        await settle();
        late.resolve(snapshot);
        await settle();
        expect(beforeEdit.state().type).toBe("loading");
        expect(afterEdit.state()).toMatchObject({
            type: "ready",
            snapshot: {
                observed_at: 999,
                quotas: [],
            },
        });
        expect(get).toHaveBeenCalledTimes(2);
    });
    it("fetches once, suppresses duplicate refreshes, retains failed refreshes, and replaces the whole snapshot", async () => {
        const held = deferred<GetProviderQuotaResponse>();
        const get = vi
            .fn()
            .mockResolvedValueOnce(snapshot)
            .mockImplementationOnce(() => held.promise)
            .mockResolvedValueOnce({
                observed_at: 456,
                quotas: [],
            });
        const stub = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    definition,
                ],
            }),
            getProviderQuota: get,
        });
        const view = createRoot((dispose) => {
            disposals.push(dispose);
            return createProviderQuotaView({
                api: () => stub,
                provider: () => provider,
            });
        });
        await settle();
        expect(get).toHaveBeenCalledTimes(1);
        view.refresh();
        view.refresh();
        await settle();
        expect(get).toHaveBeenCalledTimes(2);
        held.reject(new Error("quota upstream unavailable"));
        await settle();
        const failed = view.state();
        expect(failed.type).toBe("ready");
        if (failed.type !== "ready") throw new Error("previous snapshot should remain");
        expect(failed.snapshot).toBe(snapshot);
        expect(failed.refresh.type).toBe("failed");
        view.refresh();
        await settle();
        expect(view.state()).toEqual({
            type: "ready",
            snapshot: {
                observed_at: 456,
                quotas: [],
            },
            refresh: {
                type: "idle",
            },
        });
    });

    it("discards late responses after provider, connection, rebind, and base URL changes", async () => {
        const first = deferred<GetProviderQuotaResponse>();
        const second = deferred<GetProviderQuotaResponse>();
        const oldGet = vi
            .fn()
            .mockImplementationOnce(() => first.promise)
            .mockImplementationOnce(() => second.promise)
            .mockResolvedValue(snapshot);
        const oldApi = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    definition,
                ],
            }),
            getProviderQuota: oldGet,
        });
        const newGet = vi.fn().mockResolvedValue({
            observed_at: 999,
            quotas: [],
        });
        const newApi = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    definition,
                    {
                        ...definition,
                        version: 3,
                    },
                ],
            }),
            getProviderQuota: newGet,
        });
        const mounted = createRoot((dispose) => {
            disposals.push(dispose);
            const [row, setRow] = createSignal<ProviderResponse | null>(provider);
            const [api, setApi] = createSignal(oldApi);
            return {
                view: createProviderQuotaView({
                    api,
                    provider: row,
                }),
                setRow,
                setApi,
            };
        });
        await settle();
        mounted.setRow({
            ...provider,
            id: provider.id + 1,
        });
        await settle();
        first.resolve({
            observed_at: 1,
            quotas: [],
        });
        await settle();
        expect(mounted.view.state().type).toBe("loading");
        mounted.setApi(newApi);
        await settle();
        second.reject(new Error("late failure"));
        await settle();
        expect(mounted.view.state()).toEqual({
            type: "ready",
            snapshot: {
                observed_at: 999,
                quotas: [],
            },
            refresh: {
                type: "idle",
            },
        });
        mounted.setRow({
            ...provider,
            provider_definition_version: 3,
        });
        await settle();
        mounted.setRow({
            ...provider,
            provider_definition_version: 3,
            base_url: "https://changed.example",
        });
        await settle();
        expect(newGet).toHaveBeenCalledTimes(3);
        mounted.setRow({
            ...provider,
            provider_definition_version: 3,
            base_url: "https://changed.example",
            is_enabled: false,
        });
        await settle();
        expect(newGet).toHaveBeenCalledTimes(3);
    });

    it("uses exact-version capability and isolates initial quota/catalog failures", async () => {
        const get = vi.fn().mockRejectedValue(new Error("quota unavailable"));
        const api = stubManagementApi({
            listProviderDefinitions: async () => ({
                provider_definitions: [
                    {
                        ...definition,
                        version: 1,
                        quota: null,
                    },
                    definition,
                ],
            }),
            getProviderQuota: get,
        });
        const mounted = createRoot((dispose) => {
            disposals.push(dispose);
            const [row, setRow] = createSignal<ProviderResponse | null>({
                ...provider,
                provider_definition_version: 1,
            });
            return {
                view: createProviderQuotaView({
                    api: () => api,
                    provider: row,
                }),
                setRow,
            };
        });
        await settle();
        expect(mounted.view.state().type).toBe("unsupported");
        expect(get).not.toHaveBeenCalled();
        mounted.setRow(provider);
        await settle();
        expect(mounted.view.state().type).toBe("failed");
        const failedApi = stubManagementApi({
            listProviderDefinitions: async () => {
                throw new Error("catalog unavailable");
            },
            getProviderQuota: get,
        });
        const failed = createRoot((dispose) => {
            disposals.push(dispose);
            return createProviderQuotaView({
                api: () => failedApi,
                provider: () => provider,
            });
        });
        await settle();
        expect(failed.state().type).toBe("failed");
        expect(get).toHaveBeenCalledTimes(1);
    });
});
