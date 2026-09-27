import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    CompleteProviderOauthRequest,
    CompleteProviderOauthResponse,
    ModelResponse,
    ProviderResponse,
    StartProviderOauthResponse,
    UpdateProviderRequest,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import { ProviderEditScreen } from "/src/screens/provider-edit-screen";
import {
    buttonWithText,
    cardWithTitle,
    labeledInput,
    modelRow,
    paragraphWithText,
    providerRow,
    settle,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

const provider = providerRow({
    provider_definition_name: "generic-openai-responses",
    format: "openai_response",
    id: 3,
    name: "alpha",
    model_prefix: "alpha-",
    base_url: "https://alpha.example",
});

const model = modelRow({
    id: 11,
    provider_id: 3,
    provider: "alpha",
    upstream_id: "gpt-x",
    public_id: "alpha-gpt-x",
});

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
    return stubManagementApi({
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
    });
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

        // The typed prefix carries its own trailing dash, and the field
        // promises it is used exactly as typed (gateway README §7 stores the
        // prefix verbatim and recomposes every unoverridden public ID from
        // exactly what the write states).
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "q-");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("q-");
        expect(holder.providers).toEqual([
            {
                ...provider,
                model_prefix: "q-",
            },
        ]);
        expect(labeledInput("Model prefix").value).toBe("q-");
        expect(document.body.textContent).not.toContain("Unsaved changes");
    });

    it("restates a typed custom prefix byte-for-byte on an OAuth provider", async () => {
        const codex = providerRow({
            type: "oauth",
            id: 3,
            name: "chatgpt",
            model_prefix: "gpt-",
            provider_definition_name: "codex",
            format: "openai_response",
            base_url: null,
            expires_at: null,
        });
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
        typeInto(labeledInput("Model prefix"), "gpt2-");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("gpt2-");
        expect(sent[0]?.base_url ?? null).toBe(null);
        expect(holder.providers).toEqual([
            {
                ...codex,
                model_prefix: "gpt2-",
            },
        ]);
    });

    it("saves a static provider whose definition never references a base URL", async () => {
        // A preset like OpenCode Zen stores `base_url: null`: its definition's
        // URLs never reference the base, so the empty field is the row's legal
        // state — the form must open clean, and saving must keep `null` on the
        // wire instead of demanding a URL the contract forbids.
        const zen: ProviderResponse = {
            ...provider,
            base_url: null,
        };
        const holder = {
            providers: [
                zen,
            ],
            models: [],
        };
        const sent: UpdateProviderRequest[] = [];
        mount(holder, sent);
        await settle();

        expect(labeledInput("Upstream base URL").value).toBe("");
        expect(document.body.textContent).not.toContain("Unsaved changes");
        expect(buttonWithText("Save changes").disabled).toBe(true);

        typeInto(labeledInput("Name"), "zen-renamed");
        await settle();
        expect(buttonWithText("Save changes").disabled).toBe(false);

        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.base_url).toBe(null);
        expect(holder.providers).toEqual([
            {
                ...zen,
                name: "zen-renamed",
                model_prefix: "zen-renamed-",
            },
        ]);
        expect(document.body.textContent).toContain("zen-renamed saved.");
        expect(document.body.textContent).not.toContain("Unsaved changes");
    });

    it("opens clean when the stored prefix is a custom value ending in a dash", async () => {
        const holder = {
            providers: [
                {
                    ...provider,
                    model_prefix: "q-",
                },
            ],
            models: [],
        };
        mount(holder);
        await settle();

        // The stored prefix is verbatim gateway state, so the form must
        // restate it byte-for-byte: a derived or normalized value would
        // present an untouched form as dirty at mount.
        expect(labeledInput("Model prefix").value).toBe("q-");
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
        typeInto(labeledInput("Model prefix"), "q-");
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
                model_prefix: "q-",
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
                model_prefix: "renamed-elsewhere-",
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
                model_prefix: "renamed-elsewhere-",
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
                model_prefix: "renamed-elsewhere-",
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
                model_prefix: "renamed-elsewhere-",
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
                model_prefix: "renamed-elsewhere-",
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
                model_prefix: "saved-here-",
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
                model_prefix: "gamma-",
            },
        ];
        holder.models = [
            {
                ...model,
                provider: "gamma",
                public_id: "gamma-gpt-x",
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
                model_prefix: "beta-",
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
        // This save typed no key either, so the notice names no replacement:
        // the line would read as a consequence the save never carried.
        expect(document.body.textContent).not.toContain("The upstream API key was replaced.");
    });

    it("states the upstream key's replacement when the save submits one", async () => {
        const holder = {
            providers: [
                provider,
            ],
            models: [],
        };
        const sent: UpdateProviderRequest[] = [];
        mount(holder, sent);
        await settle();

        // The field is write-only and the save empties it, so the notice is
        // the replacement's only statement anywhere in the app: a save that
        // typed a key must say the stored secret no longer holds.
        typeInto(labeledInput("Replace the upstream API key"), "sk-replacement");
        await settle();
        buttonWithText("Save changes").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.api_key).toBe("sk-replacement");
        expect(document.body.textContent).toContain("The upstream API key was replaced.");
        // The prefix is unchanged here, so the replacement line stands alone.
        expect(document.body.textContent).not.toContain("recomposes the public ID");
    });
});

describe("ProviderEditScreen base URL rules", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the screen over the base-carrying fixture row. */
    async function mountBaseCarryingRow(): Promise<void> {
        disposes.push(
            render(
                () => (
                    <ProviderEditScreen
                        api={screenApi({
                            providers: [
                                provider,
                            ],
                            models: [],
                        })}
                        providerId={3}
                    />
                ),
                document.body,
            ),
        );
        await settle();
    }

    it("names the emptiness rule, not the composition rule, when a base-carrying row's URL is cleared", async () => {
        await mountBaseCarryingRow();

        // The stored row proves its definition evaluates requests against a
        // base, so the cleared field's fault is emptiness — the composition
        // message would name a spacing fault over a field holding nothing.
        typeInto(labeledInput("Upstream base URL"), "");
        await settle();

        expect(document.body.textContent).toContain(
            "This provider's definition evaluates requests against a base URL — restore one, or Revert.",
        );
        expect(document.body.textContent).not.toContain(
            "Base URL must be an http:// or https:// address with no spaces.",
        );
        expect(buttonWithText("Save changes").disabled).toBe(true);
    });

    it("names the composition rule for a non-empty invalid URL", async () => {
        await mountBaseCarryingRow();

        typeInto(labeledInput("Upstream base URL"), "alpha.example");
        await settle();

        expect(document.body.textContent).toContain(
            "Base URL must be an http:// or https:// address with no spaces.",
        );
        expect(document.body.textContent).not.toContain("restore one, or Revert.");
        expect(buttonWithText("Save changes").disabled).toBe(true);
    });

    it("clears the error and opens the save once a valid URL stands again", async () => {
        await mountBaseCarryingRow();

        typeInto(labeledInput("Upstream base URL"), "alpha.example");
        await settle();
        expect(buttonWithText("Save changes").disabled).toBe(true);

        typeInto(labeledInput("Upstream base URL"), "https://alpha.example/v2");
        await settle();

        expect(document.body.textContent).not.toContain(
            "Base URL must be an http:// or https:// address with no spaces.",
        );
        expect(document.body.textContent).not.toContain("restore one, or Revert.");
        expect(buttonWithText("Save changes").disabled).toBe(false);
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

describe("ProviderEditScreen prefix-change warning", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    /** Mounts the screen over the fixture provider with the given models. */
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

    it("counts the recomposed public IDs beside the prefix field while the prefix stands changed", async () => {
        mountWith([
            model,
            {
                ...model,
                id: 12,
                upstream_id: "gpt-y",
                public_id: "hand-named",
                id_override: "hand-named",
            },
        ]);
        await settle();

        expect(document.body.textContent).not.toContain("Saving recomposes");

        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "q-");
        await settle();

        // The same predicate the save notice answers with states itself
        // before the write: only the unoverridden row whose public ID the
        // submitted prefix would rewrite is counted — the override keeps
        // its ID under any prefix, so it counts nothing.
        const warning = paragraphWithText(
            "Saving recomposes 1 model ID — clients calling the old IDs will 404 until updated.",
        );
        expect(cardWithTitle("Settings").contains(warning)).toBe(true);
    });

    it("counts two recomposed public IDs in the plural", async () => {
        mountWith([
            model,
            {
                ...model,
                id: 12,
                upstream_id: "gpt-y",
                public_id: "alpha-gpt-y",
            },
        ]);
        await settle();

        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "q-");
        await settle();

        expect(document.body.textContent).toContain(
            "Saving recomposes 2 model IDs — clients calling the old IDs will 404 until updated.",
        );
    });

    it("warns before the save while a rename moves the derived prefix", async () => {
        mountWith([
            model,
        ]);
        await settle();

        // The prefix rides the name while the custom switch is off, so the
        // rename alone stands the warning — no prefix field is open.
        typeInto(labeledInput("Name"), "beta");
        await settle();

        expect(document.body.textContent).toContain(
            "Saving recomposes 1 model ID — clients calling the old IDs will 404 until updated.",
        );
    });

    it("states no warning over a dirty form whose prefix is unchanged", async () => {
        mountWith([
            model,
        ]);
        await settle();

        // The custom switch on over the stored prefix restates it byte for
        // byte, and the Enabled toggle dirties the form without moving the
        // prefix: no recomposition stands to warn about.
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "alpha-");
        await settle();
        switchWithLabel("Enabled").click();
        await settle();

        expect(document.body.textContent).toContain("Unsaved changes");
        expect(document.body.textContent).not.toContain("Saving recomposes");
    });
});

describe("ProviderEditScreen reauthorization", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    const oauthProvider = providerRow({
        type: "oauth",
        expires_at: 1_700_000_000_000_000,
        id: 3,
        name: "chatgpt",
        model_prefix: "gpt-",
        provider_definition_name: "codex",
        format: "openai_response",
        base_url: null,
    });

    const reauthorizationFlow: StartProviderOauthResponse = {
        grant: "authorization_code",
        authorize_url: "https://auth.example/authorize?state=state-9",
        state: "state-9",
        expires_at: 1_700_000_060_000_000,
    };

    /** The edit screen over an OAuth provider whose reauthorization completes as the test says. */
    function reauthorizeScreenApi(options: {
        readonly complete: (
            request: CompleteProviderOauthRequest,
        ) => Promise<CompleteProviderOauthResponse>;
    }): ManagementApi {
        const base = screenApi({
            providers: [
                oauthProvider,
            ],
            models: [],
        });

        return {
            ...base,
            reauthorizeProvider: () => Promise.resolve(reauthorizationFlow),
            completeProviderOauth: (request) => options.complete(request),
        };
    }

    async function mountAndReconnect(api: ManagementApi): Promise<void> {
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

        buttonWithText("Reconnect the account").click();
        await settle();
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await settle();
    }

    it("states the gateway's failure when the exchange is refused instead of claiming success", async () => {
        await mountAndReconnect(
            reauthorizeScreenApi({
                complete: () =>
                    Promise.resolve({
                        status: "failed",
                        failure_code: "invalid_grant",
                        failure_message: "The upstream refused the exchange.",
                    }),
            }),
        );
        buttonWithText("Replace the credentials").click();
        await settle();

        // The refused exchange is the gateway's HTTP-200 verdict: it renders
        // with its retry hint, and the consumed flow resets so a fresh one can
        // start — the old credentials stand untouched.
        expect(document.body.textContent).toContain("The upstream refused the exchange.");
        expect(document.body.textContent).toContain("Start a fresh flow");
        expect(document.body.textContent).not.toContain("Credentials replaced.");
        expect(buttonWithText("Reconnect the account")).toBeInstanceOf(HTMLButtonElement);

        // The exchange is a step of the flow the Credentials card runs, so
        // its failure states itself on that card, beside the reconnect
        // action it refused — never on the screen's read-level notice above
        // the page.
        expect(
            cardWithTitle("Credentials").contains(
                paragraphWithText("The upstream refused the exchange."),
            ),
        ).toBe(true);
    });

    it("states the flow's own verdict when the completion answers gone and reloads the listing", async () => {
        const base = reauthorizeScreenApi({
            complete: () => Promise.reject(goneFailure("no such flow")),
        });
        let listCount = 0;
        const api: ManagementApi = {
            ...base,
            listProviders: (signal) => {
                listCount += 1;
                return base.listProviders(signal);
            },
        };
        await mountAndReconnect(api);
        buttonWithText("Replace the credentials").click();
        await settle();

        // The gone answer is the spent flow's own verdict — the flow named
        // as the subject, the gateway's message kept as the detail, the
        // fresh-flow recovery as the way forward — never the raw row
        // diagnosis, which names neither this flow nor any way forward.
        expect(document.body.textContent).toContain(
            "This authorization flow is no longer completable — its state expired or was already used.",
        );
        expect(document.body.textContent).toContain("no such flow");
        expect(document.body.textContent).toContain("Start a fresh flow");
        expect(document.body.textContent).not.toContain("The gateway holds no such row.");
        expect(document.body.textContent).not.toContain("Credentials replaced.");

        // The spent flow reset, so a fresh reauthorization can start, and
        // the verdict states itself on the Credentials card beside the
        // action it answered — the gone handover's listing reload lands
        // beside it without clearing the verdict.
        expect(buttonWithText("Reconnect the account")).toBeInstanceOf(HTMLButtonElement);
        expect(cardWithTitle("Credentials").contains(paragraphWithText("no such flow"))).toBe(true);
        expect(listCount).toBe(2);
    });

    it("states the replacement and reloads the listing when the completion succeeds", async () => {
        const base = reauthorizeScreenApi({
            complete: () =>
                Promise.resolve({
                    status: "succeeded",
                    provider: {
                        ...oauthProvider,
                        expires_at: 1_800_000_000_000_000,
                    },
                }),
        });
        let listCount = 0;
        const api: ManagementApi = {
            ...base,
            listProviders: (signal) => {
                listCount += 1;
                return base.listProviders(signal);
            },
        };
        await mountAndReconnect(api);
        buttonWithText("Replace the credentials").click();
        await settle();

        expect(document.body.textContent).toContain("Credentials replaced.");
        // The success reload lands: one initial read plus the action's own reload.
        expect(listCount).toBe(2);
        expect(buttonWithText("Reconnect the account")).toBeInstanceOf(HTMLButtonElement);
    });

    it("states the fresh expiry beside a succeeded flow over a held row, and stands the reconnect offer down", async () => {
        const holder = {
            providers: [
                oauthProvider,
            ],
            models: [],
        };
        const base = screenApi(holder);
        const api: ManagementApi = {
            ...base,
            reauthorizeProvider: () => Promise.resolve(reauthorizationFlow),
            completeProviderOauth: () =>
                Promise.resolve({
                    status: "succeeded" as const,
                    provider: {
                        ...oauthProvider,
                        expires_at: 1_900_000_000_000_000,
                    },
                }),
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

        // The settings form stands dirty, so the reload the succeeded
        // completion triggers keeps the pre-reconnect row held behind it.
        typeInto(labeledInput("Model prefix"), "gpt2-");
        await settle();

        // The gateway reauthorized: its listing now answers the fresh row
        // with the current credential. Nothing reads it yet — the answer
        // lands with the reload the succeeded completion itself triggers.
        holder.providers = [
            {
                ...oauthProvider,
                expires_at: 1_900_000_000_000_000,
            },
        ];

        buttonWithText("Reconnect the account").click();
        await settle();
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await settle();
        buttonWithText("Replace the credentials").click();
        await settle();

        // The flow's ok notice and the dirty form's Save-or-Revert hold stand
        // together, and the card states the credential the gateway now holds —
        // the fresh expiry, never the held row's lapsed sentence — offering no
        // second flow over a credential that is current.
        expect(document.body.textContent).toContain("Credentials replaced.");
        expect(document.body.textContent).toContain("changed on the gateway");
        expect(labeledInput("Model prefix").value).toBe("gpt2-");
        expect(document.body.textContent).toContain("Valid for");
        expect(document.body.textContent).not.toContain("Expired");
        expect(document.body.textContent).not.toContain("Reconnect the account");
    });

    it("keeps a live flow and its pasted code across a read that rewrites the row", async () => {
        // The token-refresh lease rewrites the row on the gateway: the fresh
        // `expires_at` flips the row's identity, the screen's keyed content
        // remounts — and a flow held inside that content would be discarded
        // silently, paste gone and the gateway's flow row orphaned.
        const holder = {
            providers: [
                {
                    ...oauthProvider,
                },
            ],
            models: [],
        };
        const base = screenApi(holder);
        const api: ManagementApi = {
            ...base,
            reauthorizeProvider: () => Promise.resolve(reauthorizationFlow),
            completeProviderOauth: () =>
                Promise.resolve({
                    status: "succeeded" as const,
                    provider: {
                        ...oauthProvider,
                        expires_at: 1_800_000_000_000_000,
                    },
                }),
        };
        await mountAndReconnect(api);
        expect(document.body.textContent).toContain("Expired");

        holder.providers = [
            {
                ...oauthProvider,
                expires_at: 1_900_000_000_000_000,
            },
        ];
        requestRefresh();
        await settle();

        // The remount happened — the expiry line states the fresh row — yet
        // the flow survives above it: the paste stands and the completion is
        // still offered, with no silent reset to the reconnect button.
        expect(document.body.textContent).toContain("Valid for");
        expect(labeledInput("Callback URL or authorization code").value).toBe("the-code");
        expect(buttonWithText("Replace the credentials")).toBeInstanceOf(HTMLButtonElement);
        expect(document.body.textContent).not.toContain("Reconnect the account");

        // And the completion still lands the replacement over the remount.
        buttonWithText("Replace the credentials").click();
        await settle();

        expect(document.body.textContent).toContain("Credentials replaced.");
    });

    it("states an unreadable paste inline beside the paste field, and retracts it once the paste reads", async () => {
        await mountAndReconnect(
            reauthorizeScreenApi({
                complete: () => Promise.reject(new Error("route not expected in test")),
            }),
        );
        typeInto(labeledInput("Callback URL or authorization code"), "not a code at all");
        await settle();
        buttonWithText("Replace the credentials").click();
        await settle();

        // The unreadable paste is the field's own mistake, so its diagnosis
        // is the field's own inline error: the lazy treatment every field's
        // error gets, rendered in the field's frame inside the Credentials
        // card — never the read-level notice above the page.
        const diagnosis = paragraphWithText(
            "That paste holds neither a bare code nor a URL with a code",
        );
        expect(cardWithTitle("Credentials").contains(diagnosis)).toBe(true);
        expect(
            diagnosis.parentElement?.contains(labeledInput("Callback URL or authorization code")),
        ).toBe(true);
        expect(document.body.textContent).not.toContain("Credentials replaced.");

        // The lazy-error gate: pasting something readable retracts the
        // diagnosis, and the completion is offered to the gateway again.
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await settle();
        expect(document.body.textContent).not.toContain(
            "That paste holds neither a bare code nor a URL with a code",
        );
    });
});
