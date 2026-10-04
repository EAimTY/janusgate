import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse, UpdateApiKeyRequest } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ApiKeyEditScreen } from "/src/screens/api-key-edit-screen";
import {
    apiKeyRow,
    buttonWithText,
    createDisposals,
    goneFailure,
    labeledInput,
    settle,
    stubListings,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

const key = apiKeyRow();

/**
 * A management API standing in for the gateway itself: its key listing
 * answers from a mutable holder (so a test can change what the gateway
 * answers between refreshes), and `api-keys:update` applies the request's
 * complete desired state back into the holder and records it, so a reload
 * after a save answers the saved row the way the real gateway would.
 */
function screenApi(
    holder: {
        keys: ReadonlyArray<ApiKeyResponse>;
    },
    sent: UpdateApiKeyRequest[] = [],
): ManagementApi {
    return stubManagementApi({
        ...stubListings({}),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [],
            }),
        listApiKeys: (signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                api_keys: [
                    ...holder.keys,
                ],
            });
        },
        updateApiKey: (request) => {
            sent.push(request);

            holder.keys = holder.keys.map((row) =>
                row.id === request.api_key_id
                    ? {
                          ...row,
                          name: request.name,
                          key: request.key,
                          is_enabled: request.is_enabled,
                          access_control: request.access_control,
                      }
                    : row,
            );

            return Promise.resolve();
        },
    });
}

/**
 * One route answer the test holds back and releases by hand, so an exchange's
 * interleaving is pinned exactly: the `promise` stands in for the route's
 * answer until `resolve` hands it over.
 */
function gated<TValue>(): {
    readonly promise: Promise<TValue>;
    readonly resolve: (value: TValue) => void;
} {
    // The executor runs synchronously, so the resolver is assigned before the
    // promise is in anyone's hands.
    let resolve!: (value: TValue) => void;
    const promise = new Promise<TValue>((settled) => {
        resolve = settled;
    });
    return {
        promise,
        resolve,
    };
}

describe("ApiKeyEditScreen refresh against an open form", () => {
    const disposes = createDisposals();

    function mount(holder: { readonly keys: ReadonlyArray<ApiKeyResponse> }): void {
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
    }

    it("keeps a dirty form across a refresh that answers the same row", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        // The field opens prefilled with the serving secret, so the reveal
        // names the current key — the way the operator inspects the live
        // value before rotating — never creation's typing label.
        expect(document.body.textContent).toContain("Show the current key");
        expect(document.body.textContent).not.toContain("Show the key while typing");

        const name = labeledInput("Name");
        typeInto(name, "renamed");
        await settle();
        expect(document.body.textContent).toContain("Unsaved changes");

        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
    });

    it("keeps a dirty form across a refresh that answers a changed row, and says so", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");

        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
        expect(document.body.textContent).toContain("changed on the gateway");
    });

    it("re-baselines a clean form across a refresh that answers a changed row", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed-elsewhere");
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("picks the gateway's version up once the operator reverts a held form", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();
        expect(labeledInput("Name").value).toBe("renamed");

        buttonWithText("Revert").click();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed-elsewhere");
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("lands a save over a held row on the sent values, carrying the rotated secret", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const sent: UpdateApiKeyRequest[] = [];
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder, sent)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "saved-here");
        typeInto(labeledInput("Key"), "sk-rotated");
        holder.keys = [
            {
                ...key,
                name: "renamed-elsewhere",
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toEqual([
            {
                api_key_id: 7,
                name: "saved-here",
                key: "sk-rotated",
                is_enabled: true,
                access_control: {
                    default_access: "deny",
                    owner_defaults: [],
                    route_overrides: [],
                },
            },
        ]);
        expect(holder.keys).toEqual([
            {
                ...key,
                name: "saved-here",
                key: "sk-rotated",
            },
        ]);
        expect(labeledInput("Name").value).toBe("saved-here");
        expect(labeledInput("Key").value).toBe("sk-rotated");
        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).toContain("saved-here saved.");
        // The save replaced the secret, so its notice names the old value's
        // death: every client holding it starts failing at once.
        expect(document.body.textContent).toContain(
            "The previous secret stopped working on this save — update every client that held it.",
        );
    });

    it("states the rotation consequence only when a save changes the secret", async () => {
        mount({
            keys: [
                key,
            ],
        });
        await settle();

        typeInto(labeledInput("Name"), "renamed-only");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(document.body.textContent).toContain("renamed-only saved.");
        expect(document.body.textContent).not.toContain(
            "The previous secret stopped working on this save — update every client that held it.",
        );
    });

    it("composes the rotation and last-enabled consequences when one save does both", async () => {
        mount({
            keys: [
                key,
            ],
        });
        await settle();

        switchWithLabel("Enabled").click();
        typeInto(labeledInput("Key"), "sk-rotated");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(document.body.textContent).toContain(
            "The previous secret stopped working on this save — update every client that held it. This was the last enabled key: the gateway now serves nothing until a key is turned on.",
        );
    });

    it("states the last-enabled-key consequence when a save turns the only enabled key off", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const sent: UpdateApiKeyRequest[] = [];
        disposes.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={screenApi(holder, sent)}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        switchWithLabel("Enabled").click();
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent[0]?.is_enabled).toBe(false);
        expect(holder.keys).toEqual([
            {
                ...key,
                is_enabled: false,
            },
        ]);
        expect(document.body.textContent).toContain(
            "This was the last enabled key: the gateway now serves nothing until a key is turned on.",
        );
    });

    it("does not claim the gateway serves nothing when the listing is re-read after the write and another key stays enabled", async () => {
        // The refresh read that was in flight when Save was clicked may land
        // after the gateway applied the disable: the listing the save's notice
        // is judged against then already answers the post-write state, where
        // the saved row itself has left the enabled count — so the count of
        // the OTHER enabled keys is the only figure that means the same thing
        // under both orderings.
        const holder = {
            keys: [
                key,
                apiKeyRow({
                    id: 8,
                    name: "service-b",
                    key: "sk-b",
                }),
            ],
        };
        const sent: UpdateApiKeyRequest[] = [];
        // Both the listing read and the save answer on the test's release, so
        // the interleaving is pinned exactly: the refresh read is in flight
        // when Save runs, lands after the gateway applied the disable, and the
        // save settles last — the notice is built against the post-write
        // listing.
        const listingGates: Array<{
            readonly promise: Promise<{
                api_keys: ApiKeyResponse[];
            }>;
            readonly resolve: (response: { api_keys: ApiKeyResponse[] }) => void;
        }> = [];
        const saveGates: Array<{
            readonly promise: Promise<void>;
            readonly resolve: () => void;
        }> = [];
        const api: ManagementApi = {
            ...screenApi(holder),
            listModelRoutes: () =>
                Promise.resolve({
                    model_routes: [],
                }),
            listApiKeys: (signal) => {
                signal?.throwIfAborted();
                const gate = gated<{
                    api_keys: ApiKeyResponse[];
                }>();
                listingGates.push(gate);
                return gate.promise;
            },
            updateApiKey: (request) => {
                sent.push(request);

                holder.keys = holder.keys.map((row) =>
                    row.id === request.api_key_id
                        ? {
                              ...row,
                              name: request.name,
                              key: request.key,
                              is_enabled: request.is_enabled,
                              access_control: request.access_control,
                          }
                        : row,
                );

                const gate = gated<void>();
                saveGates.push({
                    promise: gate.promise,
                    resolve: () => gate.resolve(undefined),
                });
                return gate.promise;
            },
        };
        disposes.push(
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
        listingGates.shift()?.resolve({
            api_keys: [
                ...holder.keys,
            ],
        });
        await settle();

        switchWithLabel("Enabled").click();
        await settle();

        // A refresh read in flight when Save runs, landing after the gateway
        // applied the disable, and the save settling last: the notice is built
        // against the post-write listing.
        requestRefresh();
        await settle();
        buttonWithText("Save changes").click();
        await settle();
        expect(sent).toEqual([
            {
                api_key_id: 7,
                name: "service-a",
                key: "sk-live",
                is_enabled: false,
                access_control: {
                    default_access: "deny",
                    owner_defaults: [],
                    route_overrides: [],
                },
            },
        ]);

        listingGates.shift()?.resolve({
            api_keys: [
                ...holder.keys,
            ],
        });
        await settle();
        saveGates.shift()?.resolve();
        await settle();

        expect(holder.keys).toEqual([
            {
                ...key,
                is_enabled: false,
            },
            apiKeyRow({
                id: 8,
                name: "service-b",
                key: "sk-b",
            }),
        ]);
        expect(document.body.textContent).toContain("service-a saved.");
        expect(document.body.textContent).not.toContain(
            "This was the last enabled key: the gateway now serves nothing until a key is turned on.",
        );

        // The same window judges the delete card's blast-radius question: the
        // held row still reads enabled while the standing listing already
        // answers the write, so judging the row against the listing's whole
        // enabled count would call it the last enabled key — key 8 is enabled,
        // and the claim would lie. Arming the card is what renders the
        // question, so the plain variant is pinned positively: the question
        // rendered and picked the non-last-enabled wording.
        buttonWithText("Delete…").click();
        await settle();

        expect(document.body.textContent).toContain("Clients holding this secret lose access.");
        expect(document.body.textContent).not.toContain("It is the last enabled key");
    });

    it("names the emptied secret and holds the save until one is typed back", async () => {
        // api-keys:update is a full-state write, so unlike the provider
        // form's write-only field the secret is restated here: an emptied Key
        // cannot save, and the disabled save must name that, not sit mute.
        mount({
            keys: [
                key,
            ],
        });
        await settle();

        typeInto(labeledInput("Key"), "");
        await settle();

        expect(document.body.textContent).toContain("A key needs a secret.");
        expect(document.body.textContent).toContain("Unsaved changes");
        expect(buttonWithText("Save changes").disabled).toBe(true);

        typeInto(labeledInput("Key"), "sk-restored");
        await settle();

        expect(document.body.textContent).not.toContain("A key needs a secret.");
        expect(buttonWithText("Save changes").disabled).toBe(false);
    });

    it("reloads the listing when a save answers gone, handing over to the not-found state", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            updateApiKey: () => Promise.reject(goneFailure("no key 7")),
        };
        disposes.push(
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

        typeInto(labeledInput("Name"), "renamed");
        await settle();
        holder.keys = [];
        buttonWithText("Save changes").click();
        await settle();

        // The message is rendered and the listing that showed the row is
        // reloaded (web README §6.4), so the not-found state takes over
        // instead of a ghost form.
        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("API key not found");
    });

    it("reloads the listing when a delete answers gone", async () => {
        const holder = {
            keys: [
                key,
            ],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            deleteApiKey: () => Promise.reject(goneFailure("no key 7")),
        };
        disposes.push(
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

        holder.keys = [];
        for (const candidate of document.querySelectorAll("button")) {
            if (candidate.textContent?.includes("Delete…") === true) {
                candidate.click();
            }
        }
        await settle();
        buttonWithText("Delete for good").click();
        await settle();

        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("API key not found");
    });
});
