import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    DiscoverModelsRequest,
    DiscoverModelsResponse,
    ListModelsRequest,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { ProviderDetailScreen } from "/src/screens/provider-detail-screen";
import { buttonWithText, settle, stubManagementApi } from "./screen-test-support";

const provider: ProviderResponse = {
    type: "api_key",
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    id: 5,
    name: "local",
    model_prefix: "local-",
    is_enabled: true,
    base_url: "https://upstream.example.com",
};

const discoveredModel: ModelResponse = {
    id: 9,
    provider_id: 5,
    provider: "local",
    upstream_id: "qwen3:32b",
    public_id: "local-qwen3-32b",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

/**
 * A management API whose reads answer from fixtures and whose write surface
 * records every call. Routes this screen never touches reject, so an
 * unexpected call fails the test loudly instead of hanging it.
 */
function screenApi(options: {
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly onDiscover: (request: DiscoverModelsRequest) => void;
    readonly onListProviders?: () => void;
    readonly discoveryAnswer?: Promise<DiscoverModelsResponse>;
}): ManagementApi {
    return stubManagementApi({
        listProviders: (signal) => {
            options.onListProviders?.();
            signal?.throwIfAborted();
            return Promise.resolve({
                providers: [
                    ...options.providers,
                ],
            });
        },
        listModels: (request: ListModelsRequest, signal) => {
            signal?.throwIfAborted();
            const filtered =
                request.provider_id === undefined
                    ? options.models
                    : options.models.filter((model) => model.provider_id === request.provider_id);
            return Promise.resolve({
                models: [
                    ...filtered,
                ],
            });
        },
        discoverModels: (request) => {
            options.onDiscover(request);

            return (
                options.discoveryAnswer ??
                Promise.resolve({
                    models: [
                        {
                            ...discoveredModel,
                        },
                    ],
                })
            );
        },
        queryUsage: () =>
            Promise.resolve({
                buckets: [],
            }),
    });
}

function bodyText(): string {
    return document.body.textContent ?? "";
}

/** A discovery answer held back until the test resolves it by hand. */
function deferredDiscoveryAnswer(): {
    readonly answer: Promise<DiscoverModelsResponse>;
    readonly resolve: (result: DiscoverModelsResponse) => void;
} {
    let resolve: (result: DiscoverModelsResponse) => void = () => undefined;
    const answer = new Promise<DiscoverModelsResponse>((settleResolve) => {
        resolve = settleResolve;
    });

    return {
        answer,
        resolve,
    };
}

describe("ProviderDetailScreen automatic discovery", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("starts one discovery on its own when the provider reports no models", async () => {
        const onDiscover = vi.fn();
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [],
            onDiscover,
        });

        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={api}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(onDiscover).toHaveBeenCalledTimes(1);
        expect(onDiscover).toHaveBeenCalledWith({
            provider_id: 5,
        });
    });

    it("does not discover when the provider already reports models", async () => {
        const onDiscover = vi.fn();
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [
                discoveredModel,
            ],
            onDiscover,
        });

        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={api}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(onDiscover).not.toHaveBeenCalled();
    });

    it("does not read again when the discovery settles after the screen is gone", async () => {
        let listCount = 0;
        const onDiscover = vi.fn();
        const discovery = deferredDiscoveryAnswer();
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [],
            onDiscover,
            onListProviders: () => {
                listCount += 1;
            },
            discoveryAnswer: discovery.answer,
        });

        const dispose = render(
            () => (
                <ProviderDetailScreen
                    api={api}
                    providerId={5}
                />
            ),
            document.body,
        );
        await settle();

        expect(onDiscover).toHaveBeenCalledTimes(1);
        expect(listCount).toBe(1);

        // The visitor navigates away before the upstream answers.
        dispose();
        discovery.resolve({
            models: [
                {
                    ...discoveredModel,
                },
            ],
        });
        await settle();

        expect(listCount).toBe(1);
    });
});

describe("ProviderDetailScreen discovery notice", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** One new model the upstream began reporting: a fresh row identity. */
    const newlyReported: ModelResponse = {
        ...discoveredModel,
        id: 12,
        upstream_id: "qwen3:14b",
        public_id: "local-qwen3-14b",
    };

    function mountWithRefresh(api: ManagementApi): void {
        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={api}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
    }

    it("counts only the models the upstream newly reports", async () => {
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [
                discoveredModel,
            ],
            onDiscover: () => undefined,
            discoveryAnswer: Promise.resolve({
                models: [
                    // The known row again, plus one genuinely new one: the
                    // headline counts the newcomer, not the whole listing.
                    {
                        ...discoveredModel,
                    },
                    newlyReported,
                ],
            }),
        });

        mountWithRefresh(api);
        await settle();
        buttonWithText("Discover models").click();
        await settle();

        expect(bodyText()).toContain("1 model discovered from local.");
        expect(bodyText()).not.toContain("2 models discovered from local.");
    });

    it("announces nothing new when the upstream reports only known rows", async () => {
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [
                discoveredModel,
            ],
            onDiscover: () => undefined,
            discoveryAnswer: Promise.resolve({
                models: [
                    {
                        ...discoveredModel,
                    },
                ],
            }),
        });

        mountWithRefresh(api);
        await settle();
        buttonWithText("Discover models").click();
        await settle();

        expect(bodyText()).toContain("Nothing new discovered from local.");
        expect(bodyText()).not.toContain("model discovered from local.");
    });
});

describe("ProviderDetailScreen facts", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("states the stored definition version the provider is bound to", async () => {
        const api = screenApi({
            providers: [
                provider,
            ],
            models: [
                discoveredModel,
            ],
            onDiscover: () => undefined,
        });

        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={api}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(bodyText()).toContain("Recipe");
        expect(bodyText()).toContain("generic-openai-chat-completions v1");
    });
});
