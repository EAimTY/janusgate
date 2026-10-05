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
});
