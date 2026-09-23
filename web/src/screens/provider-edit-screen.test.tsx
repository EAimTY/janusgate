import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ModelResponse,
    ProviderResponse,
    UpdateProviderRequest,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ProviderEditScreen } from "/src/screens/provider-edit-screen";

const provider: ProviderResponse = {
    type: "openai_response",
    id: 3,
    name: "alpha",
    model_prefix: "alpha/",
    is_enabled: true,
    base_url: "https://alpha.example",
};

const model: ModelResponse = {
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
    public_id: "alpha/gpt-x",
    id_override: null,
    is_enabled: true,
    discovery_state: "discovered",
};

/**
 * A management API standing in for the gateway itself: its listings answer
 * from mutable holders, and `providers:update` applies the request's complete
 * desired state back into the holder, so a reload after a save answers the
 * saved row the way the real gateway would. Requests are recorded into the
 * optional wire the test passes, for assertions on what the form sent.
 */
function screenApi(
    holder: {
        providers: ReadonlyArray<ProviderResponse>;
        models: ReadonlyArray<ModelResponse>;
    },
    sent: UpdateProviderRequest[] = [],
): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
        listProviders: (signal) => {
            signal?.throwIfAborted();
            return Promise.resolve({
                providers: [
                    ...holder.providers,
                ],
            });
        },
        listModels: () =>
            Promise.resolve({
                models: [
                    ...holder.models,
                ],
            }),
        updateProvider: (request) => {
            sent.push(request);

            holder.providers = holder.providers.map((row) => {
                if (row.id !== request.provider_id) {
                    return row;
                }

                const rewritten: ProviderResponse = {
                    ...row,
                    name: request.name,
                    model_prefix: request.model_prefix,
                    is_enabled: request.is_enabled,
                };

                return "base_url" in request
                    ? {
                          ...rewritten,
                          base_url: request.base_url,
                      }
                    : rewritten;
            });

            return Promise.resolve();
        },
        deleteProvider: refused,
        createProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        createModel: refused,
        updateModel: refused,
        deleteModel: refused,
        discoverModels: refused,
        createApiKey: refused,
        updateApiKey: refused,
        deleteApiKey: refused,
        listApiKeys: refused,
        listUsage: refused,
        queryUsage: refused,
    };
}

/** Drains the pending promises and scheduled work of one screen exchange. */
async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

function labeledInput(label: string): HTMLInputElement {
    for (const candidate of document.querySelectorAll("label")) {
        const id = candidate.getAttribute("for");

        if (candidate.textContent === label && id !== null) {
            const input = document.getElementById(id);

            if (input instanceof HTMLInputElement) {
                return input;
            }
        }
    }

    throw new Error(`No input labeled ${label}`);
}

function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

/** Types into an input the way a browser delivers an edit. */
function typeInto(input: HTMLInputElement, value: string): void {
    input.value = value;
    input.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
}

function switchWithLabel(label: string): HTMLButtonElement {
    const candidate = document.querySelector(`button[role='switch'][aria-label='${label}']`);

    if (candidate === null) {
        throw new Error(`No switch labeled ${label}`);
    }

    return candidate as HTMLButtonElement;
}

function goneFailure(message: string): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno: ErrorCode.NotFound,
        httpStatus: 404,
    });
}

/**
 * A management API whose third provider listing is held until the test
 * releases it, exposing the window between a save and its reload landing.
 */
function gatedScreenApi(holder: {
    providers: ReadonlyArray<ProviderResponse>;
    models: ReadonlyArray<ModelResponse>;
}): {
    api: ManagementApi;
    isReloadInFlight: () => boolean;
    releaseReload: (rows: ReadonlyArray<ProviderResponse>) => void;
    failReload: (error: unknown) => void;
} {
    let readCount = 0;
    let release: ((rows: ReadonlyArray<ProviderResponse>) => void) | null = null;
    let fail: ((error: unknown) => void) | null = null;

    const api: ManagementApi = {
        ...screenApi(holder),
        listProviders: (signal) => {
            signal?.throwIfAborted();
            readCount += 1;

            if (readCount !== 3) {
                return Promise.resolve({
                    providers: [
                        ...holder.providers,
                    ],
                });
            }

            return new Promise((resolve, reject) => {
                release = (rows) =>
                    resolve({
                        providers: [
                            ...rows,
                        ],
                    });
                fail = reject;
            });
        },
    };

    return {
        api,
        isReloadInFlight: () => release !== null,
        releaseReload: (rows) => release?.(rows),
        failReload: (error) => fail?.(error),
    };
}

describe("ProviderEditScreen refresh against an open form", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mount(
        holder: {
            providers: ReadonlyArray<ProviderResponse>;
            models: ReadonlyArray<ModelResponse>;
        },
        sent: UpdateProviderRequest[] = [],
    ): void {
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={screenApi(holder, sent)}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
    }

    it("keeps a dirty form across a refresh that answers the same row", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        await settle();
        expect(document.body.textContent).toContain("Unsaved changes");

        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
    });

    it("restates a typed custom prefix byte-for-byte in the update request", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const sent: UpdateProviderRequest[] = [];
        mount(holder, sent);
        await settle();

        // The typed prefix carries its own trailing slash, and the field
        // promises it is used exactly as typed (gateway README §7 stores the
        // prefix verbatim and recomposes every unoverridden public ID from
        // exactly what the write states).
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "q/");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("q/");
        expect(holder.providers).toEqual([
            {
                ...provider,
                model_prefix: "q/",
            },
        ]);
        expect(labeledInput("Model prefix").value).toBe("q/");
        expect(document.body.textContent).not.toContain("Unsaved changes");
    });

    it("restates a typed custom prefix byte-for-byte on an OAuth provider", async () => {
        const codex: ProviderResponse = {
            type: "codex_oauth",
            id: 3,
            name: "chatgpt",
            model_prefix: "gpt-",
            is_enabled: true,
            expires_at: null,
        };
        const holder = {
            providers: [
                codex,
            ],
            models: [],
        };
        const sent: UpdateProviderRequest[] = [];
        mount(holder, sent);
        await settle();

        // The OAuth row already carries a hand-set prefix, so its field is
        // open at mount; the save must restate the typed value byte-for-byte
        // exactly as the static-key path must.
        typeInto(labeledInput("Model prefix"), "gpt/");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("gpt/");
        expect(sent[0] && "base_url" in sent[0]).toBe(false);
        expect(holder.providers).toEqual([
            {
                ...codex,
                model_prefix: "gpt/",
            },
        ]);
    });

    it("opens clean when the stored prefix is a custom value ending in a slash", async () => {
        const holder = {
            providers: [
                {
                    ...provider,
                    model_prefix: "q/",
                },
            ],
            models: [],
        };
        mount(holder);
        await settle();

        // The stored prefix is verbatim gateway state, so the form must
        // restate it byte-for-byte: a derived or normalized value would
        // present an untouched form as dirty at mount.
        expect(labeledInput("Model prefix").value).toBe("q/");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(buttonWithText("Save changes").disabled).toBe(true);
    });

    it("accepts a name the gateway would store verbatim, whitespace included", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const sent: UpdateProviderRequest[] = [];
        mount(holder, sent);
        await settle();

        // The gateway's only name rule is non-empty, stored verbatim: a
        // whitespace-only name is legal once the prefix is hand-set, and
        // nothing here is trimmed — rejecting more than the gateway would
        // block a legal name.
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "q/");
        typeInto(labeledInput("Name"), " ");
        await settle();

        expect(document.body.textContent).not.toContain("A provider needs a name.");
        expect(buttonWithText("Save changes").disabled).toBe(false);

        buttonWithText("Save changes").click();
        await settle();

        expect(sent[0]?.name).toBe(" ");
        expect(holder.providers).toEqual([
            {
                ...provider,
                name: " ",
                model_prefix: "q/",
            },
        ]);
    });

    it("keeps the held row from adopting the pre-save listing before the save's reload lands", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const gate = gatedScreenApi(holder);
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={gate.api}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "saved-here");
        holder.providers = [
            {
                ...provider,
                name: "renamed-elsewhere",
                model_prefix: "renamed-elsewhere/",
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();
        expect(gate.isReloadInFlight()).toBe(true);

        // The window between the dirt clearing and the save's reload
        // landing: the listing still answers the pre-save rows, and the
        // held row must not adopt them — the sent values persist.
        expect(labeledInput("Name").value).toBe("saved-here");
        expect(document.body.textContent).not.toContain("renamed-elsewhere");

        gate.releaseReload(holder.providers);
        await settle();

        expect(labeledInput("Name").value).toBe("saved-here");
        expect(document.body.textContent).not.toContain("renamed-elsewhere");
        expect(document.body.textContent).toContain("saved-here saved.");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).not.toContain("changed on the gateway");
    });

    it("keeps the sent values when the save-triggered reload fails, until a later read resolves", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const gate = gatedScreenApi(holder);
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={gate.api}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "saved-here");
        holder.providers = [
            {
                ...provider,
                name: "renamed-elsewhere",
                model_prefix: "renamed-elsewhere/",
            },
        ];
        requestRefresh();
        await settle();

        buttonWithText("Save changes").click();
        await settle();
        expect(gate.isReloadInFlight()).toBe(true);

        gate.failReload(
            new ManagementApiError("gateway", "reload dropped", {
                errno: ErrorCode.UpstreamError,
                httpStatus: 502,
            }),
        );
        await settle();

        // A failed reload must not hand the form the stale pre-save rows
        // either: the save succeeded, so the listing is stale by
        // construction and the sent values stand.
        expect(labeledInput("Name").value).toBe("saved-here");
        expect(document.body.textContent).not.toContain("renamed-elsewhere");
        expect(document.body.textContent).toContain("reload dropped");

        // The first read that starts after the save resolves the hand-off.
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("saved-here");
        expect(document.body.textContent).toContain("Edit saved-here");
        expect(document.body.textContent).not.toContain("renamed-elsewhere");
    });

    it("reloads the listing when a save answers gone, handing over to the not-found state", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            updateProvider: () => Promise.reject(goneFailure("no provider 3")),
        };
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={api}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        await settle();
        holder.providers = [];
        buttonWithText("Save changes").click();
        await settle();

        // The message is rendered and the listing that showed the row is
        // reloaded (web README §6.4), so the screen stops showing state the
        // gateway no longer holds instead of inviting a ghost re-save.
        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("Provider not found");
    });

    it("reloads the listing when a delete answers gone", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const api: ManagementApi = {
            ...screenApi(holder),
            deleteProvider: () => Promise.reject(goneFailure("no provider 3")),
        };
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={api}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        holder.providers = [];
        for (const candidate of document.querySelectorAll("button")) {
            if (candidate.textContent?.includes("Delete…") === true) {
                candidate.click();
            }
        }
        await settle();
        buttonWithText("Delete for good").click();
        await settle();

        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("Provider not found");
    });

    it("keeps a dirty form across a refresh that answers a changed row, and says so", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");

        holder.providers = [
            {
                ...provider,
                name: "renamed-elsewhere",
                model_prefix: "renamed-elsewhere/",
            },
        ];
        requestRefresh();
        await settle();

        expect(labeledInput("Name").value).toBe("renamed");
        expect(document.body.textContent).toContain("changed on the gateway");
    });

    it("picks the gateway's latest version up once the operator reverts a held form", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "renamed");
        holder.providers = [
            {
                ...provider,
                name: "renamed-elsewhere",
                model_prefix: "renamed-elsewhere/",
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

    it("lands a save over a held row on the sent values, clearing the hold", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "saved-here");
        holder.providers = [
            {
                ...provider,
                name: "renamed-elsewhere",
                model_prefix: "renamed-elsewhere/",
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();

        expect(holder.providers).toEqual([
            {
                ...provider,
                name: "saved-here",
                model_prefix: "saved-here/",
            },
        ]);
        expect(labeledInput("Name").value).toBe("saved-here");
        expect(document.body.textContent).not.toContain("changed on the gateway");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(document.body.textContent).toContain("saved-here saved.");
    });

    it("keeps the recomposition warning out of a held save that recomposes no public ID", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        // The gateway renames itself to the very name the form holds and
        // recomposes its model to match: the save then submits a prefix the
        // gateway already holds, so no public ID changes and no warning may
        // state one — whatever the form's first-read baseline says.
        typeInto(labeledInput("Name"), "gamma");
        holder.providers = [
            {
                ...provider,
                name: "gamma",
                model_prefix: "gamma/",
            },
        ];
        holder.models = [
            {
                ...model,
                provider: "gamma",
                public_id: "gamma/gpt-x",
            },
        ];
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("changed on the gateway");

        buttonWithText("Save changes").click();
        await settle();

        expect(document.body.textContent).toContain("gamma saved.");
        expect(document.body.textContent).not.toContain("recomposes the public ID");
    });

    it("states the recomposition warning when the save does recompose a public ID", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [
                model,
            ],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "beta");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(holder.providers).toEqual([
            {
                ...provider,
                name: "beta",
                model_prefix: "beta/",
            },
        ]);
        expect(document.body.textContent).toContain("beta saved.");
        expect(document.body.textContent).toContain("recomposes the public ID");
    });

    it("states no recomposition warning over a provider with no models", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        mount(holder);
        await settle();

        typeInto(labeledInput("Name"), "beta");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(document.body.textContent).toContain("beta saved.");
        expect(document.body.textContent).not.toContain("recomposes the public ID");
    });
});

describe("ProviderEditScreen deletion copy", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the screen over a read-only gateway with the given models. */
    function mountWith(models: ReadonlyArray<ModelResponse>): void {
        const holder = {
            providers: [
                provider,
            ],
            models,
        };
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={screenApi(holder)}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
    }

    it("names the cascade and the clients' 404s while models stand behind the provider", async () => {
        mountWith([
            model,
        ]);
        await settle();

        expect(document.body.textContent).toContain(
            "Deleting removes the provider and, by cascade, its 1 model.",
        );

        const deleteButton = [
            ...document.querySelectorAll("button"),
        ].find((candidate) => candidate.textContent?.includes("Delete…"));
        deleteButton?.click();
        await settle();

        expect(document.body.textContent).toContain(
            "Delete alpha? Clients calling those public IDs will start getting 404s.",
        );
    });

    it("states no cascade over a provider with no models", async () => {
        mountWith([]);
        await settle();

        expect(document.body.textContent).toContain(
            "Deleting removes the provider. It has no models, so nothing else is removed.",
        );
        expect(document.body.textContent).not.toContain("by cascade");

        const deleteButton = [
            ...document.querySelectorAll("button"),
        ].find((candidate) => candidate.textContent?.includes("Delete…"));
        deleteButton?.click();
        await settle();

        expect(document.body.textContent).toContain("Delete alpha?");
        expect(document.body.textContent).not.toContain("404s");
    });
});
