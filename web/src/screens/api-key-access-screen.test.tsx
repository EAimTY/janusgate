import { render } from "@solidjs/web";
import { createSignal } from "solid-js";
import { describe, expect, it } from "vitest";
import { type AccessInventory, newAccessControl } from "/src/access-control";
import type {
    ApiKeyAccessControl,
    ApiKeyResponse,
    CreateApiKeyRequest,
    UpdateApiKeyRequest,
} from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ApiKeyAccess } from "/src/ui/api-key-access";
import { ApiKeyEditScreen } from "./api-key-edit-screen";
import { ApiKeyNewScreen } from "./api-key-new-screen";
import {
    apiKeyRow,
    bodyText,
    buttonWithText,
    chooseSelect,
    createDisposals,
    goneFailure,
    type HeldCall,
    heldCall,
    labeledInput,
    modelRow,
    providerRow,
    settle,
    stubListings,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

const inventory: AccessInventory = {
    providers: [
        providerRow({
            id: 1,
            name: "Account",
        }),
        providerRow({
            id: 2,
            name: "Future",
        }),
    ],
    models: [
        modelRow({
            id: 3,
            provider_id: 1,
            upstream_id: "M",
        }),
    ],
    pools: [
        {
            id: 1,
            name: "Shared",
            model_prefix: "shared-",
            is_enabled: true,
            members: [
                {
                    provider_id: 1,
                    priority: 0,
                },
            ],
        },
    ],
    modelRoutes: [
        {
            id: 4,
            public_id: "alias",
            is_enabled: false,
            is_automatic: false,
            target: {
                type: "direct",
                model_id: 3,
            },
        },
        {
            id: 5,
            public_id: "shared-M",
            is_enabled: true,
            is_automatic: true,
            target: {
                type: "pool",
                pool_id: 1,
                upstream_id: "M",
            },
        },
    ],
};
function listings(): ReturnType<typeof stubListings> {
    return stubListings({
        providers: inventory.providers,
        models: inventory.models,
        pools: inventory.pools,
        modelRoutes: inventory.modelRoutes,
    });
}

describe("serving-key permission drafts", () => {
    const disposals = createDisposals();

    it("blocks creation until inventory success and preserves typed fields and policy through failed reads", async () => {
        const calls: Array<
            HeldCall<{
                model_routes: [];
            }>
        > = [];
        const api = stubManagementApi({
            ...listings(),
            listModelRoutes: (signal) => heldCall(signal, calls),
        });
        disposals.push(render(() => <ApiKeyNewScreen api={api} />, document.body));
        await settle();
        typeInto(labeledInput("Name"), "Client");
        typeInto(labeledInput("Key"), "client-secret");
        await settle();
        expect(buttonWithText("Create API key").disabled).toBe(true);
        calls[0]?.refuse(new Error("Inventory unavailable"));
        await settle();
        expect(bodyText()).toContain("Inventory unavailable");
        expect(buttonWithText("Create API key").disabled).toBe(true);
        buttonWithText("Read again").click();
        await settle();
        calls[1]?.answer({
            model_routes: [],
        });
        await settle();
        expect(labeledInput("Name").value).toBe("Client");
        expect(labeledInput("Key").value).toBe("client-secret");
        expect(buttonWithText("Create API key").disabled).toBe(false);
        expect(bodyText()).toContain("Default model access");
        expect(bodyText()).toContain("This default applies to future entries.");
        chooseSelect("Default model access", "allow");
        await settle();
        requestRefresh();
        await settle();
        calls[2]?.answer({
            model_routes: [],
        });
        await settle();
        expect(document.querySelector<HTMLSelectElement>("select")?.value).toBe("allow");
    });

    it("creates a complete default-deny policy after inventory reads", async () => {
        const sent: CreateApiKeyRequest[] = [];
        const api = stubManagementApi({
            ...listings(),
            createApiKey: (request) => {
                sent.push(request);
                return new Promise(() => {});
            },
        });
        disposals.push(render(() => <ApiKeyNewScreen api={api} />, document.body));
        await settle();
        typeInto(labeledInput("Name"), "Client");
        typeInto(labeledInput("Key"), "client-secret");
        await settle();
        buttonWithText("Create API key").click();
        await settle();
        expect(sent).toEqual([
            {
                name: "Client",
                key: "client-secret",
                is_enabled: true,
                access_control: newAccessControl(),
            },
        ]);
    });

    it("keeps explicit pins and owner kinds through filtering and inherited preview changes", async () => {
        const [policy, setPolicy] = createSignal<ApiKeyAccessControl>(newAccessControl());
        disposals.push(
            render(
                () => (
                    <ApiKeyAccess
                        inventory={inventory}
                        policy={policy()}
                        onChange={setPolicy}
                    />
                ),
                document.body,
            ),
        );
        chooseSelect("Account provider default", "deny");
        await settle();
        chooseSelect("Shared pool default", "allow");
        await settle();
        chooseSelect("alias model access", "allow");
        await settle();
        expect(bodyText()).toContain("Allowed · Model override");
        expect(bodyText()).toContain("Globally disabled");
        expect(bodyText()).toContain("Allowed · from pool");
        const pinned = policy();
        typeInto(labeledInput("Search model access"), "Shared");
        switchWithLabel("Overrides only").click();
        await settle();
        expect(bodyText()).not.toContain("alias");
        expect(policy()).toBe(pinned);
        chooseSelect("Default model access", "allow");
        await settle();
        expect(policy().owner_defaults).toEqual(pinned.owner_defaults);
        expect(policy().route_overrides).toEqual(pinned.route_overrides);
        typeInto(labeledInput("Search model access"), "");
        switchWithLabel("Overrides only").click();
        await settle();
        chooseSelect("alias model access", "inherit");
        await settle();
        expect(policy().route_overrides).toEqual([]);
        expect(bodyText()).toContain("Denied · from provider");
    });

    it("holds permission-only changes, saves pins with ordinary edits, and keeps a racing 404 draft", async () => {
        let keys: ApiKeyResponse[] = [
            apiKeyRow({
                access_control: {
                    default_access: "allow",
                    owner_defaults: [],
                    route_overrides: [
                        {
                            model_route_id: 4,
                            access: "allow",
                        },
                    ],
                },
            }),
        ];
        let routes = inventory.modelRoutes;
        const sent: UpdateApiKeyRequest[] = [];
        let shouldReject = true;
        const api = stubManagementApi({
            ...listings(),
            listApiKeys: () =>
                Promise.resolve({
                    api_keys: keys,
                }),
            listModelRoutes: () =>
                Promise.resolve({
                    model_routes: [
                        ...routes,
                    ],
                }),
            updateApiKey: (request) => {
                sent.push(request);
                if (shouldReject)
                    return Promise.reject(goneFailure("Model route 4 does not exist"));
                keys = keys.map((key) => ({
                    ...key,
                    name: request.name,
                    key: request.key,
                    is_enabled: request.is_enabled,
                    access_control: request.access_control,
                }));
                return Promise.resolve();
            },
        });
        disposals.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        typeInto(labeledInput("Name"), "Renamed");
        await settle();
        keys = keys.map((key) => ({
            ...key,
            access_control: newAccessControl(),
        }));
        requestRefresh();
        await settle();
        expect(bodyText()).toContain("changed on the gateway");
        expect(bodyText()).toContain("Allowed · Model override");
        buttonWithText("Save").click();
        await settle();
        expect(labeledInput("Name").value).toBe("Renamed");
        expect(sent[0]?.access_control.route_overrides).toEqual([
            {
                model_route_id: 4,
                access: "allow",
            },
        ]);
        routes = inventory.modelRoutes.filter((route) => route.id !== 4);
        requestRefresh();
        await settle();
        expect(bodyText()).toContain("Deleted route #4");
        expect(buttonWithText("Save").disabled).toBe(true);
        buttonWithText("Remove override").click();
        await settle();
        expect(buttonWithText("Save").disabled).toBe(false);
        shouldReject = false;
        buttonWithText("Save").click();
        await settle();
        expect(sent[1]?.access_control.route_overrides).toEqual([]);
        expect(bodyText()).not.toContain("changed on the gateway");
        expect(labeledInput("Name").value).toBe("Renamed");
    });
});
