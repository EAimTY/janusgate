import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type {
    UpdateModelRouteRequest,
    UpdateProviderPoolRequest,
} from "/src/management-api/generated/v0";
import {
    buttonWithText,
    chooseSelect,
    createDisposals,
    labeledInput,
    modelRow,
    providerRow,
    settle,
    stubManagementApi,
    typeInto,
} from "/src/test-support/management-api";
import { ProviderPoolEditorScreen } from "../provider-pools/provider-pool-editor-screen";
import { ModelRouteEditorScreen } from "./model-route-editor-screen";

const disposes = createDisposals();

describe("routing editors", () => {
    it("commits a priority swap, removal, addition, label, and prefix edit in one complete pool update", async () => {
        const sent: UpdateProviderPoolRequest[] = [];
        const pool = {
            model_prefix: "",
            id: 3,
            name: "Coding",
            is_enabled: true,
            members: [
                {
                    provider_id: 1,
                    priority: 10,
                },
                {
                    provider_id: 2,
                    priority: 30,
                },
            ],
        };
        const api = stubManagementApi({
            getProviderAvailability: () =>
                Promise.resolve({
                    blocks: [],
                    last_refresh: {
                        type: "not_observed",
                    },
                }),
            listModels: () =>
                Promise.resolve({
                    models: [],
                }),
            listProviderPools: () =>
                Promise.resolve({
                    provider_pools: [
                        pool,
                    ],
                }),
            listProviders: () =>
                Promise.resolve({
                    providers: [
                        providerRow(),
                        providerRow({
                            id: 2,
                            name: "second",
                        }),
                        providerRow({
                            id: 4,
                            name: "third",
                        }),
                    ],
                }),
            updateProviderPool: (request) => {
                sent.push(request);
                return Promise.resolve();
            },
        });
        disposes.push(
            render(
                () => (
                    <ProviderPoolEditorScreen
                        api={api}
                        poolId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        document.querySelector<HTMLButtonElement>('button[title="Move second up"]')?.click();
        await settle();
        document.querySelector<HTMLButtonElement>('button[title="Remove local"]')?.click();
        await settle();
        chooseSelect("Add account", "4");
        await settle();
        buttonWithText("Add member").click();
        await settle();
        typeInto(labeledInput("Name"), "Coding team");
        typeInto(labeledInput("Model prefix"), "team-");
        await settle();
        buttonWithText("Save changes").click();
        await settle();
        expect(sent).toEqual([
            {
                model_prefix: "team-",
                provider_pool_id: 3,
                name: "Coding team",
                is_enabled: true,
                members: [
                    {
                        provider_id: 2,
                        priority: 10,
                    },
                    {
                        provider_id: 4,
                        priority: 11,
                    },
                ],
            },
        ]);
    });

    it("switches a direct route to a pool while retaining its public name and allowing missing inventory", async () => {
        const sent: UpdateModelRouteRequest[] = [];
        const api = stubManagementApi({
            getProviderAvailability: () =>
                Promise.resolve({
                    blocks: [],
                    last_refresh: {
                        type: "not_observed",
                    },
                }),
            listModelRoutes: () =>
                Promise.resolve({
                    model_routes: [
                        {
                            is_automatic: false,
                            id: 7,
                            public_id: "coding",
                            is_enabled: true,
                            target: {
                                type: "direct",
                                model_id: 9,
                            },
                        },
                    ],
                }),
            listProviderPools: () =>
                Promise.resolve({
                    provider_pools: [
                        {
                            model_prefix: "",
                            id: 3,
                            name: "Coding",
                            is_enabled: true,
                            members: [
                                {
                                    provider_id: 1,
                                    priority: 0,
                                },
                            ],
                        },
                    ],
                }),
            listProviders: () =>
                Promise.resolve({
                    providers: [
                        providerRow(),
                    ],
                }),
            listModels: () =>
                Promise.resolve({
                    models: [
                        modelRow(),
                    ],
                }),
            updateModelRoute: (request) => {
                sent.push(request);
                return Promise.resolve();
            },
        });
        disposes.push(
            render(
                () => (
                    <ModelRouteEditorScreen
                        api={api}
                        routeId={7}
                        initialModelId={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        chooseSelect("Target type", "pool");
        await settle();
        chooseSelect("Pool", "3");
        typeInto(labeledInput("Upstream ID"), "not-discovered-yet");
        await settle();
        expect(document.body.textContent).toContain("Model missing");
        expect(buttonWithText("Save changes").disabled).toBe(false);
        buttonWithText("Save changes").click();
        await settle();
        expect(sent).toEqual([
            {
                model_route_id: 7,
                public_id: "coding",
                is_enabled: true,
                target: {
                    type: "pool",
                    pool_id: 3,
                    upstream_id: "not-discovered-yet",
                },
            },
        ]);
    });

    it("renders a pristine pool creation without an error and names member buttons after the account", async () => {
        const api = stubManagementApi({
            getProviderAvailability: () =>
                Promise.resolve({
                    blocks: [],
                    last_refresh: {
                        type: "not_observed",
                    },
                }),
            listProviderPools: () =>
                Promise.resolve({
                    provider_pools: [],
                }),
            listProviders: () =>
                Promise.resolve({
                    providers: [
                        providerRow(),
                        providerRow({
                            id: 2,
                            name: "second",
                        }),
                    ],
                }),
            listModels: () =>
                Promise.resolve({
                    models: [],
                }),
        });
        disposes.push(
            render(
                () => (
                    <ProviderPoolEditorScreen
                        api={api}
                        poolId={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // A pristine form is not an error: the seeded-empty name states no
        // required message from first paint — the save gate holds the action.
        expect(document.body.textContent).not.toContain("Name is required.");

        chooseSelect("Add account", "2");
        await settle();
        buttonWithText("Add member").click();
        await settle();

        // Every member row repeats the visible Move up/Remove labels, so the
        // accessible name — not the shared label — carries the member.
        expect(document.querySelector('button[aria-label="Move second up"]')).not.toBeNull();
        expect(document.querySelector('button[aria-label="Move second down"]')).not.toBeNull();
        expect(document.querySelector('button[aria-label="Remove second"]')).not.toBeNull();

        // An emptied name is typed input: its error opens without a submit.
        typeInto(labeledInput("Name"), "");
        await settle();
        expect(document.body.textContent).toContain("Name is required.");
    });

    it("renders a pristine route creation without errors, however the target type stands", async () => {
        const api = stubManagementApi({
            getProviderAvailability: () =>
                Promise.resolve({
                    blocks: [],
                    last_refresh: {
                        type: "not_observed",
                    },
                }),
            listModelRoutes: () =>
                Promise.resolve({
                    model_routes: [],
                }),
            listProviderPools: () =>
                Promise.resolve({
                    provider_pools: [
                        {
                            model_prefix: "",
                            id: 3,
                            name: "Coding",
                            is_enabled: true,
                            members: [
                                {
                                    provider_id: 1,
                                    priority: 0,
                                },
                            ],
                        },
                    ],
                }),
            listProviders: () =>
                Promise.resolve({
                    providers: [
                        providerRow(),
                    ],
                }),
            listModels: () =>
                Promise.resolve({
                    models: [],
                }),
        });
        disposes.push(
            render(
                () => (
                    <ModelRouteEditorScreen
                        api={api}
                        routeId={null}
                        initialModelId={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // A pristine form is not an error: the seeded-empty public ID states
        // no required message from first paint.
        expect(document.body.textContent).not.toContain("Public ID is required.");

        // Switching the target type mounts the untouched Upstream ID: it
        // states no required message from its first paint either.
        chooseSelect("Target type", "pool");
        await settle();
        expect(document.body.textContent).not.toContain("Upstream ID is required.");

        // Typed input opens each field's error without a submit.
        typeInto(labeledInput("Public ID"), "");
        await settle();
        expect(document.body.textContent).toContain("Public ID is required.");

        typeInto(labeledInput("Upstream ID"), "");
        await settle();
        expect(document.body.textContent).toContain("Upstream ID is required.");
    });
});
