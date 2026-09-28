import { render } from "@solidjs/web";
import { describe, expect, it, vi } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type {
    DiscoverModelsRequest,
    DiscoverModelsResponse,
    ListModelsRequest,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { ProviderDetailScreen } from "/src/screens/provider-detail-screen";
import {
    bodyText,
    buttonWithText,
    createDisposals,
    linkTo,
    modelRow,
    paragraphWithText,
    providerRow,
    settle,
    stubManagementApi,
} from "./screen-test-support";

const provider = providerRow({
    id: 5,
    base_url: "https://upstream.example.com",
});

const discoveredModel = modelRow({
    provider_id: 5,
});

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
    const disposes = createDisposals();

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
    const disposes = createDisposals();

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
    const disposes = createDisposals();

    /** Mounts the screen over one provider row with the discovered model listed. */
    function mount(row: ProviderResponse): void {
        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={screenApi({
                            providers: [
                                row,
                            ],
                            models: [
                                discoveredModel,
                            ],
                            onDiscover: () => undefined,
                        })}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
    }

    it("names a preset's definition by the title the operator picked, with the stored id beneath", async () => {
        // The shared row is bound to the catalog preset
        // `generic-openai-chat-completions` — at creation the operator saw
        // its title, so the fact leads with that and keeps the stored
        // `name vversion` beneath for exact identification.
        mount(provider);
        await settle();

        expect(bodyText()).toContain("Definition");
        expect(bodyText()).toContain("OpenAI-compatible Chat Completions API");
        expect(bodyText()).toContain("generic-openai-chat-completions v1");
        expect(bodyText()).not.toContain("Recipe");
    });

    it("states a hand-authored definition by its stored name alone, inventing no title", async () => {
        // A name outside the preset catalog never existed as a pick, so the
        // fact has only the stored identity to state — and states it plainly.
        mount({
            ...provider,
            provider_definition_name: "my-own-definition",
            provider_definition_version: 3,
        });
        await settle();

        expect(bodyText()).toContain("Definition");
        expect(bodyText()).toContain("my-own-definition v3");
        expect(bodyText()).not.toContain("Recipe");
    });

    it("names a static key's kind once — on the header line, never as a badge", async () => {
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

        // One term per surface: the header line names "Static API key", and
        // the shorter "Static key" alias must not return as a badge beside it.
        expect(bodyText().split("Static API key").length - 1).toBe(1);
        expect(bodyText()).not.toContain("Static key");
    });

    it("speaks one enabled pair per page — the header badge and the Details card both read Enabled", async () => {
        mount(provider);
        await settle();

        // The Details card echoes the header badge's word exactly: both
        // surfaces count one "Enabled" each, and the tight list-row value
        // never leaks onto the page.
        expect(bodyText().split("Enabled").length - 1).toBe(2);
        expect(bodyText()).not.toContain("Off");
    });

    it("names a disabled provider Disabled on both surfaces, never Off", async () => {
        mount({
            ...provider,
            is_enabled: false,
        });
        await settle();

        expect(bodyText().split("Disabled").length - 1).toBe(2);
        expect(bodyText()).not.toContain("Enabled");
        expect(bodyText()).not.toContain("Off");
    });

    it("opens the model's creation under the same Create model label the models screen uses", async () => {
        mount(provider);
        await settle();

        // The creation page's own title is "Create model" — the card's link
        // must name the destination by the word the destination answers with.
        // It is a ButtonLink: an anchor, not a button element.
        const createModel = document.querySelector('a[href="/models/new?provider=5"]');
        expect(createModel?.textContent).toContain("Create model");
        expect(bodyText()).not.toContain("Add model");
    });

    it("hands the searchable models view over as a third door", async () => {
        mount(provider);
        await settle();

        // The inline list stays unsearchable; the quiet third door opens the
        // models screen filtered to this provider — the address the home
        // screen's "Review models" row already speaks.
        const allModels = linkTo("/models?provider=5");
        expect(allModels.textContent).toContain("All models");
    });
});

describe("ProviderDetailScreen credential reconnect", () => {
    const disposes = createDisposals();

    /** One OAuth provider whose authorization lapsed long ago. */
    const expiredOauth = providerRow({
        type: "oauth",
        id: 5,
        name: "local",
        model_prefix: "local-",
        provider_definition_name: "codex-oauth",
        format: "openai_response",
        base_url: null,
        expires_at: 1,
    });

    /** The same provider with an authorization good for decades yet. */
    const validOauth: ProviderResponse = {
        ...expiredOauth,
        expires_at: 4_102_444_800_000_000,
    };

    function mount(providers: ReadonlyArray<ProviderResponse>): void {
        disposes.push(
            render(
                () => (
                    <ProviderDetailScreen
                        api={screenApi({
                            providers,
                            models: [
                                discoveredModel,
                            ],
                            onDiscover: () => undefined,
                        })}
                        providerId={5}
                    />
                ),
                document.body,
            ),
        );
    }

    it("names reconnect on the edit page under a lapsed credential badge", async () => {
        mount([
            expiredOauth,
        ]);
        await settle();

        expect(bodyText()).toContain("The OAuth credentials have expired.");

        // The edit page already has a header button; the line is the pin, so
        // the link is read out of the reconnect line itself.
        const reconnectLine = paragraphWithText("Reconnect on the");
        const editLink = reconnectLine.querySelector("a");

        expect(editLink?.getAttribute("href")).toBe("/providers/5/edit");
        expect(editLink?.textContent).toContain("edit page");
        expect(reconnectLine.textContent).toBe(
            "Reconnect on the edit page — its Credentials card replaces them.",
        );
    });

    it("defers the reconnect to the re-enable while the provider is parked", async () => {
        // The overview's attention row defers the same fact for a parked
        // provider instead of ordering a reconnect the operator deliberately
        // deferred; the detail page's line must defer with it, while still
        // naming the edit page — both the re-enable and the reconnect live
        // there.
        mount([
            {
                ...expiredOauth,
                is_enabled: false,
            },
        ]);
        await settle();

        expect(bodyText()).toContain("The OAuth credentials have expired.");

        const parkedLine = paragraphWithText("The provider is disabled");
        const editLink = parkedLine.querySelector("a");

        expect(editLink?.getAttribute("href")).toBe("/providers/5/edit");
        expect(editLink?.textContent).toContain("edit page");
        expect(parkedLine.textContent).toBe(
            "The provider is disabled — reconnect when you re-enable it; the edit page's Credentials card replaces them.",
        );
    });

    it("states no reconnect line while the credential is valid", async () => {
        mount([
            validOauth,
        ]);
        await settle();

        expect(bodyText()).toContain("Valid");
        expect(bodyText()).not.toContain("Reconnect on the");
    });
});
