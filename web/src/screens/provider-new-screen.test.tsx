import { render } from "@solidjs/web";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { appConfig } from "/src/config";
import type { ManagementApi } from "/src/management-api/client";
import type { DefinitionPayload } from "/src/management-api/definition-payload";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    CompleteProviderOauthRequest,
    CompleteProviderOauthResponse,
    CreateProviderRequest,
    DefinitionResponse,
    ProviderResponse,
    StartProviderOauthRequest,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { PROVIDER_PRESETS, type ProviderPreset } from "/src/presets";
import { refreshStamp, requestRefresh, resetRefreshStamp } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { ProviderNewScreen } from "/src/screens/provider-new-screen";
import {
    buttonWithText,
    cardWithTitle,
    labeledInput,
    paragraphWithText,
    providerRow,
    settle,
    stubManagementApi,
    switchWithLabel,
    typeInto,
} from "./screen-test-support";

/**
 * A management API standing in for the gateway: `providers:create` records
 * its request for assertions on what the form sent; every other route
 * refuses, so an unexpected call fails the test loudly. The mount reads can
 * be overridden per test — including with factories that reject, so a failed
 * listing's diagnosis is stated on the screen (the factory keeps the
 * rejection from existing before a handler is attached).
 */
function screenApi(
    sent: CreateProviderRequest[] = [],
    options: {
        readonly providers?: () => Promise<{
            providers: ProviderResponse[];
        }>;
        readonly definitions?: () => Promise<{
            provider_definitions: DefinitionResponse[];
        }>;
    } = {},
): ManagementApi {
    return stubManagementApi({
        listProviderDefinitions: () =>
            options.definitions?.() ??
            Promise.resolve({
                provider_definitions: [],
            }),
        createProviderDefinition: () =>
            Promise.resolve({
                provider_definition: {
                    name: "generic-openai-responses",
                    version: 1,
                    format: "openai_response",
                    inference: {
                        url: [
                            {
                                type: "base",
                            },
                            {
                                type: "literal",
                                value: "/responses",
                            },
                        ],
                        upstream_streaming: "match_client",
                    },
                    credential: {
                        type: "api_key",
                    },
                    created_at: 1_700_000_000_000_000,
                },
            }),
        listProviders: () =>
            options.providers?.() ??
            Promise.resolve({
                providers: [],
            }),
        createProvider: (request) => {
            sent.push(request);
            return Promise.resolve({
                provider: {
                    type: "api_key",
                    id: 1,
                    name: request.name,
                    model_prefix: request.model_prefix,
                    is_enabled: true,
                    provider_definition_name: request.provider_definition_name,
                    provider_definition_version: request.provider_definition_version,
                    format: "openai_response",
                    base_url: request.base_url,
                },
            });
        },
    });
}

/** One stored definition row of the given name and version, payload included. */
function storedDefinitionRow(name: string, version: number): DefinitionResponse {
    return {
        name,
        version,
        created_at: 1_700_000_000_000_000,
        format: "openai_response",
        inference: {
            url: [
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/responses",
                },
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
    };
}

/** How many flow-lock reason lines the screen currently states. */
function flowLockLines(): number {
    const text = document.body.textContent ?? "";
    return (
        text.split(
            "Locked while the authorization is live — the provider is created from what the flow's start already sent.",
        ).length - 1
    );
}

/** Opens the definition disclosure and answers its editor. */
function definitionEditor(): HTMLTextAreaElement {
    const disclosure = document.querySelector("details");

    if (disclosure === null) {
        throw new Error("No definition disclosure");
    }

    if (!disclosure.open) {
        disclosure.querySelector("summary")?.dispatchEvent(
            new MouseEvent("click", {
                bubbles: true,
            }),
        );
    }

    const editor = disclosure.querySelector("textarea");

    if (!(editor instanceof HTMLTextAreaElement)) {
        throw new Error("No definition editor");
    }

    return editor;
}

/**
 * The `toggle` event a browser pairs with the disclosure's state change.
 * The stub DOM flips `open` on a summary click but delivers the event late
 * and unreliably, so the tests that depend on the screen's tracking of the
 * state simulate the browser's event deterministically here.
 */
function fireDefinitionToggle(): void {
    const disclosure = document.querySelector("details");

    if (disclosure === null) {
        throw new Error("No definition disclosure");
    }

    disclosure.dispatchEvent(new Event("toggle"));
}

/** The Claude Code recipe edited to address its requests through the base. */
function baseAddressedPayload(): DefinitionPayload {
    const preset = PROVIDER_PRESETS.find((candidate) => candidate.id === "claude-code");
    if (preset === undefined) {
        throw new Error("the claude-code preset is in the catalog");
    }
    return {
        ...preset.payload,
        inference: {
            ...preset.payload.inference,
            url: [
                {
                    type: "base",
                },
            ],
        },
    };
}

/** The catalog preset of the given id, failing the test when absent. */
function presetById(id: string): ProviderPreset {
    const preset = PROVIDER_PRESETS.find((candidate) => candidate.id === id);

    if (preset === undefined) {
        throw new Error(`no preset ${id} in the catalog`);
    }

    return preset;
}

describe("ProviderNewScreen creation", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("sends a typed custom prefix byte-for-byte in the create request", async () => {
        const sent: CreateProviderRequest[] = [];
        disposes.push(render(() => <ProviderNewScreen api={screenApi(sent)} />, document.body));
        await settle();

        // The typed prefix carries its own trailing dash, and the field
        // promises it is used exactly as typed (gateway README §7 stores the
        // prefix verbatim and recomposes every unoverridden public ID from
        // exactly what the write states) — creation must match the edit
        // path's byte-for-byte rule.
        switchWithLabel("Custom model prefix").click();
        await settle();
        typeInto(labeledInput("Model prefix"), "beta-");
        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("Upstream API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.model_prefix).toBe("beta-");
    });

    it("states the base-URL error only after the field was typed in", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        // A pristine form is not an error: the preset needs a base URL, but
        // the required-ness is not stated as alert-red before any input —
        // the same gate every other screen's fields open behind.
        expect(document.body.textContent).not.toContain(
            "Base URL must be an http:// or https:// address",
        );

        typeInto(labeledInput("Upstream base URL"), "not an address");
        await settle();
        expect(document.body.textContent).toContain(
            "Base URL must be an http:// or https:// address",
        );
    });

    it("keeps the definition JSON behind a closed disclosure and resets it to the preset", async () => {
        const presetJson = JSON.stringify(PROVIDER_PRESETS[0].payload, null, 2);
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        const disclosure = document.querySelector("details");
        expect(disclosure).not.toBeNull();
        expect(disclosure?.open).toBe(false);

        disclosure?.querySelector("summary")?.dispatchEvent(
            new MouseEvent("click", {
                bubbles: true,
            }),
        );
        expect(disclosure?.open).toBe(true);
        expect(definitionEditor().value).toBe(presetJson);

        typeInto(definitionEditor(), "{}");
        await settle();
        expect(document.body.textContent).toContain("definition.format is not an upstream format");

        buttonWithText("Reset to preset").click();
        await settle();
        expect(definitionEditor().value).toBe(presetJson);
        expect(document.body.textContent).not.toContain(
            "definition.format is not an upstream format",
        );
    });

    it("names the offending path of an invalid edit and disables submission", async () => {
        const sent: CreateProviderRequest[] = [];
        disposes.push(render(() => <ProviderNewScreen api={screenApi(sent)} />, document.body));
        await settle();

        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("Upstream API key"), "sk-test");
        await settle();
        expect(buttonWithText("Create provider").disabled).toBe(false);

        typeInto(definitionEditor(), '{"format": 7}');
        await settle();
        expect(document.body.textContent).toContain(
            "definition.format is not an upstream format: 7",
        );
        expect(buttonWithText("Create provider").disabled).toBe(true);

        expect(sent).toHaveLength(0);
    });

    it("names the invalid recipe outside the closed disclosure and keeps the full error inside", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        // A valid definition states no marker.
        expect(document.body.textContent).not.toContain("The connection recipe is invalid");

        typeInto(definitionEditor(), '{"format": 7}');
        fireDefinitionToggle();
        await settle();
        expect(document.body.textContent).toContain(
            "definition.format is not an upstream format: 7",
        );

        // The disclosure closed over the full error: the marker outside it
        // names the dead primary buttons' cause and points at the disclosure,
        // instead of leaving the submit disabled with no visible why.
        document
            .querySelector("details")
            ?.querySelector("summary")
            ?.dispatchEvent(
                new MouseEvent("click", {
                    bubbles: true,
                }),
            );
        fireDefinitionToggle();
        await settle();
        expect(document.querySelector("details")?.open).toBe(false);
        expect(document.body.textContent).toContain("The connection recipe is invalid");
        expect(document.body.textContent).toContain(
            "open the Advanced disclosure above for the offending path",
        );
        expect(document.body.textContent).toContain(
            "definition.format is not an upstream format: 7",
        );
    });

    it("states the invalid recipe by its inline path error alone while the disclosure stands open", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        // The recipe breaks while the disclosure stands open: the pointer
        // outside instructs opening what is already open, so with the editor
        // and the path error both in view the pointer must yield and the
        // inline error alone is the statement.
        definitionEditor();
        fireDefinitionToggle();
        typeInto(definitionEditor(), '{"format": 7}');
        await settle();

        expect(document.body.textContent).toContain(
            "definition.format is not an upstream format: 7",
        );
        expect(document.body.textContent).not.toContain("The connection recipe is invalid");

        // Closing the disclosure brings the pointer back — the instruction
        // becomes followable exactly when the path error left the view.
        document
            .querySelector("details")
            ?.querySelector("summary")
            ?.dispatchEvent(
                new MouseEvent("click", {
                    bubbles: true,
                }),
            );
        fireDefinitionToggle();
        await settle();
        expect(document.querySelector("details")?.open).toBe(false);
        expect(document.body.textContent).toContain("The connection recipe is invalid");
    });

    it("renders the selected preset's provenance", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();
        expect(document.body.textContent).toContain(
            "platform.openai.com/docs/api-reference/responses",
        );

        buttonWithText("Claude Code").click();
        await settle();
        expect(document.body.textContent).toContain(
            "verify against the live CLI before production use",
        );
    });

    it("hides the base URL for a preset whose definition never references it", async () => {
        const sent: CreateProviderRequest[] = [];
        disposes.push(render(() => <ProviderNewScreen api={screenApi(sent)} />, document.body));
        await settle();

        buttonWithText("OpenCode Zen — Chat Completions").click();
        await settle();

        const labels = [
            ...document.querySelectorAll("label"),
        ].map((label) => label.textContent);
        expect(labels).not.toContain("Upstream base URL");
        expect(labels).toContain("Upstream API key");

        typeInto(labeledInput("Upstream API key"), "sk-zen");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.base_url).toBe(null);
    });

    it("binds and states the latest stored version of the preset's definition", async () => {
        const sent: CreateProviderRequest[] = [];
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi(sent, {
                            definitions: () =>
                                Promise.resolve({
                                    provider_definitions: [
                                        storedDefinitionRow("generic-openai-responses", 1),
                                        storedDefinitionRow("generic-openai-responses", 2),
                                    ],
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(document.body.textContent).toContain("Stored as version 2.");

        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("Upstream API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.provider_definition_name).toBe("generic-openai-responses");
        expect(sent[0]?.provider_definition_version).toBe(2);
    });

    it("withholds the stored-version claims until the boot read answers", async () => {
        const releases: Array<(response: { provider_definitions: DefinitionResponse[] }) => void> =
            [];
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            definitions: () =>
                                new Promise((resolve) => {
                                    releases.push(resolve);
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // While the boot read is in flight the page asserts nothing about the
        // stored versions — neither the absence claim nor the unreadable one,
        // the same wait the stored case's line already keeps. The rest of the
        // card stands, as the page's no-skeleton design has it.
        expect(document.body.textContent).not.toContain("Not stored yet");
        expect(document.body.textContent).not.toContain("The stored versions could not be read.");
        expect(document.body.textContent).toContain("Advanced: the connection recipe");

        // The read answers absent: the absence claim is now established, and
        // only then stated.
        heldDefinitionsRelease(
            releases,
            0,
        )({
            provider_definitions: [],
        });
        await settle();
        expect(document.body.textContent).toContain(
            "Not stored yet — submitting creates version 1.",
        );
    });

    it("states a failed providers listing where the screen's failures stand", async () => {
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () => Promise.reject(new Error("the listing refused")),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(document.body.textContent).toContain("the listing refused");

        // A boot failure is the page-top notice's alone: it renders above
        // the cards, never inside one — the one channel the flow's own
        // failures may not reach.
        expect(paragraphWithText("the listing refused").closest("section")).toBeNull();
    });

    it("states a failed definitions listing where the screen's failures stand", async () => {
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            definitions: () => Promise.reject(new Error("the definitions refused")),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(document.body.textContent).toContain("the definitions refused");
        expect(document.body.textContent).toContain("The stored versions could not be read.");
        expect(document.body.textContent).not.toContain("Not stored yet");
    });

    it("states the first declared call's failure when both boot reads fail", async () => {
        // Holders rather than bare `let`s: the assignment happens inside the
        // stubbed listings below, and a bare variable would be narrowed to
        // `null` at the reject sites.
        const providersReject: {
            current: ((error: unknown) => void) | null;
        } = {
            current: null,
        };
        const definitionsReject: {
            current: ((error: unknown) => void) | null;
        } = {
            current: null,
        };
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () =>
                                new Promise((_resolve, reject) => {
                                    providersReject.current = reject;
                                }),
                            definitions: () =>
                                new Promise((_resolve, reject) => {
                                    definitionsReject.current = reject;
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // Both calls fail — the providers listing settling first, the
        // definitions listing last — so the diagnosis must name the first
        // declared call's failure, the shared harness's `readAll` contract,
        // never whichever continuation settled last.
        providersReject.current?.(new Error("the listing refused"));
        definitionsReject.current?.(new Error("the definitions refused"));
        await settle();

        expect(document.body.textContent).toContain("the listing refused");
        expect(document.body.textContent).not.toContain("the definitions refused");
        // The definitions call's own side state stands beside the notice.
        expect(document.body.textContent).toContain("The stored versions could not be read.");
    });

    it("diagnoses a definition-write conflict as the definition's own recovery", async () => {
        const sent: CreateProviderRequest[] = [];
        const api: ManagementApi = {
            ...screenApi(sent),
            createProviderDefinition: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "definition name already stored", {
                        errno: ErrorCode.Conflict,
                        httpStatus: 409,
                    }),
                ),
        };
        disposes.push(render(() => <ProviderNewScreen api={api} />, document.body));
        await settle();

        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("Upstream API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        // The conflict is the definition write's — its name is the preset id,
        // nothing this page edits — so the fix names the moved stored version,
        // never the provider-name recovery a provider conflict carries.
        expect(document.body.textContent).toContain("definition name already stored");
        expect(document.body.textContent).toContain("The stored version moved");
        expect(document.body.textContent).toContain(
            "use Refresh in the bar above; it re-reads the stored versions and leaves this form untouched — then submit again",
        );
        expect(document.body.textContent).not.toContain("Choose a provider name");
        expect(sent).toHaveLength(0);
    });

    it("states a refused creation's diagnosis with no Read again, and it survives a boot re-read", async () => {
        const sent: CreateProviderRequest[] = [];
        const api: ManagementApi = {
            ...screenApi(sent),
            createProviderDefinition: () =>
                Promise.reject(
                    new ManagementApiError("gateway", "definition name already stored", {
                        errno: ErrorCode.Conflict,
                        httpStatus: 409,
                    }),
                ),
        };
        disposes.push(render(() => <ProviderNewScreen api={api} />, document.body));
        await settle();

        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        typeInto(labeledInput("Upstream API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        // The refused ensure's diagnosis stands on the page-top notice — the
        // creation action's own channel — and no Read again stands beside it:
        // the boot read is not what failed, and rerunning it would not retry
        // the refused ensure.
        expect(document.body.textContent).toContain("definition name already stored");
        expect(
            [
                ...document.querySelectorAll("button"),
            ].some((candidate) => candidate.textContent?.includes("Read again") === true),
        ).toBe(false);

        // The refresh pulse re-runs the boot read, which answers — and
        // clears only a boot-origin failure: the creation's diagnosis
        // survives a re-read that never retried the action.
        requestRefresh();
        await settle();

        expect(document.body.textContent).toContain("definition name already stored");
        expect(sent).toHaveLength(0);
    });
});

describe("ProviderNewScreen name seeding", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("seeds the untouched name field with the picked preset's own suggestion", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        // The boot read answers an empty gateway: the fresh page's seed is
        // the default preset's bare suggestion.
        expect(labeledInput("Name").value).toBe(PROVIDER_PRESETS[0].suggestedName);

        buttonWithText("Claude Code").click();
        await settle();
        expect(labeledInput("Name").value).toBe(presetById("claude-code").suggestedName);

        // A second untouched pick seeds its own preset's suggestion, never
        // the previous pick's: the field takes the write's own inputs, not a
        // memo read back before the scheduler flushed the preset switch.
        buttonWithText("Devin").click();
        await settle();
        expect(labeledInput("Name").value).toBe(presetById("devin").suggestedName);
    });

    it("seeds the suffixed suggestion when the boot read answers a taken base name", async () => {
        const baseName = PROVIDER_PRESETS[0].suggestedName;
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () =>
                                Promise.resolve({
                                    providers: [
                                        providerRowNamed(baseName),
                                    ],
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The boot read's seed computes over the names it just wrote: the
        // committed taken-names set still reads empty at write time, so a
        // memo read-back would seed the bare, already-taken base — a
        // guaranteed conflict on submit.
        expect(labeledInput("Name").value).toBe(`${baseName}-1`);
    });
});

describe("ProviderNewScreen preset switching over an edited recipe", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("switches presets instantly while the definition stands untouched", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        buttonWithText("Claude Code").click();
        await settle();

        // The common path gains nothing: no confirm stands, and the editor
        // already holds the picked preset's payload.
        expect(document.body.textContent).not.toContain(
            "Your edited connection recipe will be replaced.",
        );
        expect(definitionEditor().value).toBe(
            JSON.stringify(presetById("claude-code").payload, null, 2),
        );
    });

    it("arms the pick behind a confirm in the Preset card while the definition stands edited", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        typeInto(definitionEditor(), "{}");
        await settle();
        buttonWithText("Claude Code").click();
        await settle();

        // The confirm states itself in the Preset card, naming the preset
        // the click armed — and applies nothing: the edited JSON stands
        // exactly as it was, its error still named, the selected preset's
        // provenance still the previous one.
        const presetCard = cardWithTitle("Preset");
        expect(presetCard.textContent).toContain("Switch to Claude Code (Claude account)?");
        expect(presetCard.textContent).toContain("Your edited connection recipe will be replaced.");
        expect(definitionEditor().value).toBe("{}");
        expect(document.body.textContent).toContain("definition.format is not an upstream format");
        expect(document.body.textContent).toContain(
            "platform.openai.com/docs/api-reference/responses",
        );

        // A later click moves the arming onto the new target: one confirm,
        // always naming the latest click's switch.
        buttonWithText("Devin").click();
        await settle();
        expect(cardWithTitle("Preset").textContent).toContain("Switch to Devin (Devin account)?");
        expect(definitionEditor().value).toBe("{}");
    });

    it("performs the switch only on the confirm and keeps the edit on cancel", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        typeInto(definitionEditor(), "{}");
        await settle();
        buttonWithText("Claude Code").click();
        await settle();

        // Cancel dissolves the arming and keeps the edit whole — and the
        // next pick arms again rather than switching.
        buttonWithText("Keep my edit").click();
        await settle();
        expect(document.body.textContent).not.toContain("Switch to Claude Code (Claude account)?");
        expect(definitionEditor().value).toBe("{}");

        buttonWithText("Claude Code").click();
        await settle();
        buttonWithText("Switch preset").click();
        await settle();

        // The confirm is the only switch: the edit gives way to the armed
        // preset's payload, the provenance line names what now stands, and
        // the switched-in preset is untouched — the next pick is instant.
        expect(document.body.textContent).not.toContain(
            "Your edited connection recipe will be replaced.",
        );
        expect(definitionEditor().value).toBe(
            JSON.stringify(presetById("claude-code").payload, null, 2),
        );
        expect(document.body.textContent).toContain(
            "verify against the live CLI before production use",
        );
        buttonWithText("Devin").click();
        await settle();
        expect(document.body.textContent).not.toContain("Switch to Devin (Devin account)?");
        expect(definitionEditor().value).toBe(JSON.stringify(presetById("devin").payload, null, 2));
    });

    it("dissolves the arming when the definition is reset to the preset", async () => {
        disposes.push(render(() => <ProviderNewScreen api={screenApi()} />, document.body));
        await settle();

        typeInto(definitionEditor(), "{}");
        await settle();
        buttonWithText("Claude Code").click();
        await settle();
        expect(document.body.textContent).toContain("Switch to Claude Code (Claude account)?");

        buttonWithText("Reset to preset").click();
        await settle();

        // With the edit gone there is nothing the arming guards; the confirm
        // goes with it instead of awaiting a switch the page would perform
        // instantly anyway.
        expect(document.body.textContent).not.toContain("Switch to Claude Code (Claude account)?");
    });

    it("dissolves the arming when an OAuth flow goes live", async () => {
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={oauthScreenApi({
                            start: () => authorizationFlow,
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // Claude Code picked over the pristine form; then a still-valid edit
        // touches the recipe, so the Kimi pick arms.
        buttonWithText("Claude Code").click();
        await settle();
        typeInto(definitionEditor(), `${definitionEditor().value}\n`);
        await settle();
        buttonWithText("Kimi Code").click();
        await settle();
        expect(document.body.textContent).toContain("Switch to Kimi Code (Kimi account)?");

        buttonWithText("Start the authorization").click();
        await settle();
        expect(document.body.textContent).toContain("Authorization URL");

        // The live flow locks the picker, so the arming could await nothing:
        // it dissolves with the lock instead of resurfacing stale once the
        // flow ends.
        expect(document.body.textContent).not.toContain("Switch to Kimi Code (Kimi account)?");
        expect(buttonWithText("Kimi Code").disabled).toBe(true);
    });

    it("holds the confirm's buttons while a creation call is in flight", async () => {
        // A holder rather than a bare `let`: the assignment happens inside
        // the stubbed start below, and a bare variable would be narrowed to
        // `null` at the release site.
        const release: {
            current: ((response: StartProviderOauthResponse) => void) | null;
        } = {
            current: null,
        };
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={oauthScreenApi({
                            start: () =>
                                new Promise<StartProviderOauthResponse>((resolve) => {
                                    release.current = resolve;
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The recipe edited (still valid) arms the Kimi pick, and the flow
        // start goes out over it — held in flight, so the page's shared busy
        // guard stands.
        buttonWithText("Claude Code").click();
        await settle();
        typeInto(definitionEditor(), `${definitionEditor().value}\n`);
        await settle();
        buttonWithText("Kimi Code").click();
        await settle();
        expect(document.body.textContent).toContain("Switch to Kimi Code (Kimi account)?");
        buttonWithText("Start the authorization").click();
        await settle();
        expect(document.body.textContent).toContain("Starting…");

        // The confirm row is the switch's only path, so it stands behind the
        // same busy guard as the picker, the editor, and the submit buttons:
        // neither its action nor its way back may run `applyPreset` under a
        // begin that is already ensuring the edited recipe.
        expect(buttonWithText("Switch preset").disabled).toBe(true);
        expect(buttonWithText("Keep my edit").disabled).toBe(true);

        release.current?.(authorizationFlow);
        await settle();
    });
});

/** A provider row of the given name, as the listing answers it. */
function providerRowNamed(name: string): ProviderResponse {
    return providerRow({
        id: 1,
        name,
        model_prefix: `${name}-`,
        provider_definition_name: "generic-openai-responses",
        format: "openai_response",
        base_url: null,
    });
}

/**
 * The release of the held listing call that arrived at the given index,
 * failing the test when no call is held there.
 */
function heldListingRelease(
    releases: ReadonlyArray<(response: { providers: ProviderResponse[] }) => void>,
    index: number,
): (response: { providers: ProviderResponse[] }) => void {
    const release = releases[index];

    if (release === undefined) {
        throw new Error(`No held listing call at index ${index}`);
    }

    return release;
}

/**
 * The release of the held definitions call that arrived at the given index,
 * failing the test when no call is held there.
 */
function heldDefinitionsRelease(
    releases: ReadonlyArray<(response: { provider_definitions: DefinitionResponse[] }) => void>,
    index: number,
): (response: { provider_definitions: DefinitionResponse[] }) => void {
    const release = releases[index];

    if (release === undefined) {
        throw new Error(`No held definitions call at index ${index}`);
    }

    return release;
}

describe("ProviderNewScreen refresh", () => {
    const disposes: Array<() => void> = [];

    beforeEach(() => {
        resetRefreshStamp();
    });

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("re-runs the boot reads on the refresh pulse and recovers a failed read", async () => {
        let isProvidersFailing = true;
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () =>
                                isProvidersFailing
                                    ? Promise.reject(new Error("the listing refused"))
                                    : Promise.resolve({
                                          providers: [
                                              providerRowNamed("openai"),
                                          ],
                                      }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The failed mount read stands on the notice, and nothing stamped
        // the shell: a cycle with a failed call is not a read that succeeded.
        expect(document.body.textContent).toContain("the listing refused");
        expect(refreshStamp()).toBeNull();

        // The pulse re-runs the boot reads: the listing answers this time,
        // the failed read's notice is recovered away, the untouched name
        // suggestion moves onto the fresh taken names, and the stamp lands.
        isProvidersFailing = false;
        requestRefresh();
        await settle();

        expect(document.body.textContent).not.toContain("the listing refused");
        expect(labeledInput("Name").placeholder).toBe("openai-1");
        expect(refreshStamp()).not.toBeNull();
    });

    it("recovers a failed boot read from the page's Read again button", async () => {
        let isProvidersFailing = true;
        let providerReads = 0;
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () => {
                                providerReads += 1;
                                return isProvidersFailing
                                    ? Promise.reject(new Error("the listing refused"))
                                    : Promise.resolve({
                                          providers: [
                                              providerRowNamed("openai"),
                                          ],
                                      });
                            },
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The failed mount read stands on the notice with the page's own way
        // back beside it — the same recovery the row screens' frame renders,
        // here for a page whose boot read owns no skeleton.
        expect(document.body.textContent).toContain("the listing refused");
        expect(buttonWithText("Read again").disabled).toBe(false);

        // The button re-runs the boot read in place: the listing answers this
        // time, the failed read's notice is recovered away, and the stamp
        // lands — a full page recovery, not a bare re-ask.
        isProvidersFailing = false;
        buttonWithText("Read again").click();
        await settle();

        expect(providerReads).toBe(2);
        expect(document.body.textContent).not.toContain("the listing refused");
        expect(labeledInput("Name").placeholder).toBe("openai-1");
        expect(refreshStamp()).not.toBeNull();
    });

    it("re-reads the stored definition versions on the refresh pulse", async () => {
        let storedVersion = 1;
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            definitions: () =>
                                Promise.resolve({
                                    provider_definitions: [
                                        storedDefinitionRow(
                                            "generic-openai-responses",
                                            storedVersion,
                                        ),
                                    ],
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        expect(document.body.textContent).toContain("Stored as version 1.");

        // A version registered on the gateway since the mount is bound — and
        // stated — by the refresh's re-read, like a fresh mount would.
        storedVersion = 2;
        requestRefresh();
        await settle();
        expect(document.body.textContent).toContain("Stored as version 2.");
    });

    it("supersedes a boot read cycle still in flight when refresh is pressed again", async () => {
        const releases: Array<(response: { providers: ProviderResponse[] }) => void> = [];
        disposes.push(
            render(
                () => (
                    <ProviderNewScreen
                        api={screenApi([], {
                            providers: () =>
                                new Promise((resolve) => {
                                    releases.push(resolve);
                                }),
                        })}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The mount cycle is still held; the pulse supersedes it, and the
        // second cycle's listing is the one that answers.
        requestRefresh();
        await settle();
        expect(releases).toHaveLength(2);
        heldListingRelease(
            releases,
            1,
        )({
            providers: [
                providerRowNamed("openai"),
            ],
        });
        await settle();
        expect(labeledInput("Name").placeholder).toBe("openai-1");

        // The superseded cycle's late answer installs nothing: the newer
        // cycle owns the screen.
        heldListingRelease(
            releases,
            0,
        )({
            providers: [],
        });
        await settle();
        expect(labeledInput("Name").placeholder).toBe("openai-1");
    });

    it("re-runs the boot reads without disturbing a live OAuth flow", async () => {
        let providerReads = 0;
        const api: ManagementApi = {
            ...oauthScreenApi({
                start: () => authorizationFlow,
            }),
            listProviders: () => {
                providerReads += 1;
                return Promise.resolve({
                    providers: [],
                });
            },
        };
        disposes.push(render(() => <ProviderNewScreen api={api} />, document.body));
        await settle();

        buttonWithText("Claude Code").click();
        await settle();
        buttonWithText("Start the authorization").click();
        await settle();
        expect(document.body.textContent).toContain("Authorization URL");

        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        const readsBeforeRefresh = providerReads;
        requestRefresh();
        await settle();

        // The reads re-ran under the pulse…
        expect(providerReads).toBe(readsBeforeRefresh + 1);

        // …and the live flow is exactly as the pulse found it: the start's
        // fields stay locked, the pasted callback survives, and the flow
        // step stands where the fresh start button would be.
        expect(labeledInput("Name").disabled).toBe(true);
        expect(labeledInput("Callback URL or authorization code").value).toBe("the-code");
        expect(
            [
                ...document.querySelectorAll("button"),
            ].some(
                (candidate) => candidate.textContent?.includes("Start the authorization") === true,
            ),
        ).toBe(false);
    });
});

/** An authorization-code flow the gateway has begun and the screen is showing. */
const authorizationFlow: StartProviderOauthResponse = {
    grant: "authorization_code",
    authorize_url: "https://claude.ai/oauth/authorize?state=state-1",
    state: "state-1",
    expires_at: 1_700_000_060_000_000,
};

/** A live device flow, expiring the given number of microseconds after the epoch. */
function deviceFlow(expiresAtMicros: number): Extract<
    StartProviderOauthResponse,
    {
        grant: "device";
    }
> {
    return {
        grant: "device",
        state: "state-d1",
        expires_at: expiresAtMicros,
        user_code: "WDJB-MJHT",
        verification_uri_complete: "https://auth.kimi.com/activate?user_code=WDJB-MJHT",
    };
}

/** The provider row a succeeded completion answers with. */
const createdProvider = providerRow({
    type: "oauth",
    expires_at: 1_700_000_100_000_000,
    id: 7,
    name: "kimi",
    model_prefix: "kimi-",
    provider_definition_name: "kimi-code",
    format: "openai_chat_completion",
    base_url: null,
});

function goneFailure(message: string): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno: ErrorCode.NotFound,
        httpStatus: 404,
    });
}

/**
 * A management API standing in for the gateway along the OAuth paths: the
 * flow-begin and completion answers come from the test, everything else
 * answers empty, and any route the flow never touches rejects loudly.
 */
function oauthScreenApi(options: {
    readonly start?:
        | ((
              request: StartProviderOauthRequest,
          ) => StartProviderOauthResponse | Promise<StartProviderOauthResponse>)
        | undefined;
    readonly complete?: (
        request: CompleteProviderOauthRequest,
    ) => Promise<CompleteProviderOauthResponse>;
}): ManagementApi {
    const flowBegin = options.start;
    const flowCompletion = options.complete;

    return stubManagementApi({
        listProviderDefinitions: () =>
            Promise.resolve({
                provider_definitions: [],
            }),
        createProviderDefinition: () =>
            Promise.resolve({
                provider_definition: {
                    name: "kimi-code",
                    version: 1,
                    format: "openai_chat_completion",
                    inference: {
                        url: [
                            {
                                type: "base",
                            },
                        ],
                        upstream_streaming: "match_client",
                    },
                    credential: {
                        type: "api_key",
                    },
                    created_at: 1_700_000_000_000_000,
                },
            }),
        listProviders: () =>
            Promise.resolve({
                providers: [],
            }),
        // A flow step the test did not provide stays unoverridden, so the
        // route refuses — the same loud failure the bare stub gives every
        // route the flow never touches.
        ...(flowBegin === undefined
            ? {}
            : {
                  startProviderOauth: (request: StartProviderOauthRequest) =>
                      Promise.resolve(flowBegin(request)),
              }),
        ...(flowCompletion === undefined
            ? {}
            : {
                  completeProviderOauth: (request: CompleteProviderOauthRequest) =>
                      flowCompletion(request),
              }),
    });
}

describe("ProviderNewScreen OAuth flow", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        vi.useRealTimers();
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mountScreen(api: ManagementApi): void {
        disposes.push(render(() => <ProviderNewScreen api={api} />, document.body));
    }

    /** Drains the pending promises and scheduled work while the fake clock runs. */
    async function flush(): Promise<void> {
        for (let round = 0; round < 10; round += 1) {
            await vi.advanceTimersByTimeAsync(0);
        }
    }

    it("states the gateway's failure and offers a fresh flow when the exchange is refused", async () => {
        vi.useFakeTimers();
        let completeRequest: CompleteProviderOauthRequest | null = null;
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
                complete: (request) => {
                    completeRequest = request;
                    return Promise.resolve({
                        status: "failed",
                        failure_code: "invalid_grant",
                        failure_message: "The upstream refused the exchange.",
                    });
                },
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(document.body.textContent).toContain("Authorization URL");

        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await flush();
        buttonWithText("Connect the account").click();
        await flush();

        expect(completeRequest).toEqual({
            grant: "authorization_code",
            state: "state-1",
            code: "the-code",
        });
        expect(document.body.textContent).toContain("The upstream refused the exchange.");
        expect(document.body.textContent).toContain("Start a fresh flow");
        // The refused exchange consumed the state (gateway README §7.2), so
        // the dead flow is gone and a fresh one can start.
        expect(buttonWithText("Start the authorization").disabled).toBe(false);

        // The exchange is a step of the flow the Connect card runs, so its
        // failure states itself on that card, beside the start button the
        // reset uncovered — not on the page-top notice above the preset
        // cards.
        expect(
            cardWithTitle("Connect the account").contains(
                paragraphWithText("The upstream refused the exchange."),
            ),
        ).toBe(true);
    });

    it("states the flow's own verdict when the completion answers gone", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
                complete: () => Promise.reject(goneFailure("no such flow")),
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await flush();
        buttonWithText("Connect the account").click();
        await flush();

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

        // The spent flow reset, so a fresh one can start, and the verdict
        // states itself on the Connect card beside the button that took the
        // action — never on the page-top notice.
        expect(buttonWithText("Start the authorization").disabled).toBe(false);
        expect(
            cardWithTitle("Connect the account").contains(paragraphWithText("no such flow")),
        ).toBe(true);
    });

    it("states an unreadable paste inline beside the paste field and keeps the flow live", async () => {
        vi.useFakeTimers();
        // No completion route is provided: an unreadable paste must be
        // refused in the browser, so the gateway is never asked.
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        typeInto(labeledInput("Callback URL or authorization code"), "not a code at all");
        await flush();
        buttonWithText("Connect the account").click();
        await flush();

        // The unreadable paste is the field's own mistake, so its diagnosis
        // is the field's own inline error: the lazy treatment every field's
        // error gets, rendered in the field's frame inside the flow card —
        // neither the page-top notice nor a card notice.
        const diagnosis = paragraphWithText(
            "That paste holds neither a bare code nor a URL with a code",
        );
        const connectCard = cardWithTitle("Connect the account");
        expect(connectCard.contains(diagnosis)).toBe(true);
        expect(
            diagnosis.parentElement?.contains(labeledInput("Callback URL or authorization code")),
        ).toBe(true);

        // The flow survives the mistaken paste: the step still stands, and
        // the lazy gate retracts the diagnosis once the paste reads.
        expect(document.body.textContent).toContain("Authorization URL");
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await flush();
        expect(document.body.textContent).not.toContain(
            "That paste holds neither a bare code nor a URL with a code",
        );
    });

    it("lands on the new provider's page when the completion succeeds", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
                complete: () =>
                    Promise.resolve({
                        status: "succeeded",
                        provider: createdProvider,
                    }),
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        typeInto(labeledInput("Callback URL or authorization code"), "the-code");
        await flush();
        buttonWithText("Connect the account").click();
        await flush();

        expect(window.location.pathname).toBe(hrefs.providerDetail(createdProvider.id));
    });

    it("locks the identity fields and the definition editor while a flow is live", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
            }),
        );
        await flush();

        // Without a live flow everything is open.
        switchWithLabel("Custom model prefix").click();
        await flush();
        expect(labeledInput("Name").disabled).toBe(false);
        expect(labeledInput("Model prefix").disabled).toBe(false);
        expect(definitionEditor().disabled).toBe(false);

        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(document.body.textContent).toContain("Authorization URL");

        // The completion carries only the state and the code — the provider
        // is created from what the flow's start already sent — so an edit to
        // the identity or the recipe while the flow is pending would look
        // accepted and do nothing. Everything the start sent locks, like the
        // preset picker.
        expect(labeledInput("Name").disabled).toBe(true);
        expect(switchWithLabel("Custom model prefix").disabled).toBe(true);
        expect(labeledInput("Model prefix").disabled).toBe(true);
        expect(definitionEditor().disabled).toBe(true);
    });

    it("states the flow lock's reason on each editable card and retracts it when the flow ends", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
            }),
        );
        await flush();

        // Without a live flow no card states a lock: the reason line exists
        // only for as long as the lock does.
        expect(flowLockLines()).toBe(0);

        buttonWithText("Claude Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(document.body.textContent).toContain("Authorization URL");

        // The preset card, the definition card, and the identity card each
        // state the reason exactly once — every grayed control's own card
        // names the lock, and no card repeats itself per control. This
        // body-text count pins only that the lines render while the flow is
        // live and retract when it ends; that the definition card's line
        // shows with its disclosure closed lives in the source placement,
        // where the card's lock line renders before the <details>.
        expect(flowLockLines()).toBe(3);

        buttonWithText("Start over").click();
        await flush();

        // The reset ends the lock, and the stated reason goes with it.
        expect(flowLockLines()).toBe(0);
        expect(labeledInput("Name").disabled).toBe(false);
    });

    it("keeps the upstream base URL on the page, disabled under the lock notice, while a flow stands live", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                start: () => authorizationFlow,
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();

        // The untouched Claude Code recipe pins its endpoints: no base-URL
        // field before the recipe addresses the base.
        expect(
            [
                ...document.querySelectorAll("label"),
            ].map((label) => label.textContent),
        ).not.toContain("Upstream base URL");

        // The recipe edited to address the base opens the field, and the
        // typed address is what the flow's start sends.
        typeInto(definitionEditor(), JSON.stringify(baseAddressedPayload()));
        await flush();
        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(document.body.textContent).toContain("Authorization URL");

        // The flow is live: the field stays on the page carrying the value
        // the start sent — disabled, with the same single-line lock reason
        // every locked card states naming why — so what went out remains
        // readable without being editable. The count is the preset,
        // definition, and identity cards' lines plus this field's own.
        const baseUrlInput = labeledInput("Upstream base URL");
        expect(baseUrlInput.disabled).toBe(true);
        expect(baseUrlInput.value).toBe("https://upstream.example.com");
        expect(flowLockLines()).toBe(4);
    });

    it("keeps the flow start disabled while the model prefix is invalid, sending nothing", async () => {
        vi.useFakeTimers();
        const calls: string[] = [];
        mountScreen(
            stubManagementApi({
                listProviderDefinitions: () =>
                    Promise.resolve({
                        provider_definitions: [],
                    }),
                createProviderDefinition: () => {
                    calls.push("definition");
                    return Promise.resolve({
                        provider_definition: storedDefinitionRow("claude-code", 1),
                    });
                },
                listProviders: () =>
                    Promise.resolve({
                        providers: [],
                    }),
                startProviderOauth: () => {
                    calls.push("start");
                    return Promise.resolve(authorizationFlow);
                },
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();

        // The contrast half of the gate: a valid prefix over a valid
        // definition leaves Start open — the state every dead-flow recovery
        // re-enters with, so a fresh flow can always begin again.
        expect(buttonWithText("Start the authorization").disabled).toBe(false);

        // An invalid custom prefix names itself inline and closes Start…
        switchWithLabel("Custom model prefix").click();
        await flush();
        typeInto(labeledInput("Model prefix"), "has space");
        await flush();
        expect(document.body.textContent).toContain(
            'Model prefix accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
        );
        expect(buttonWithText("Start the authorization").disabled).toBe(true);

        // …and the gate keeps the rejection in the browser: the click issues
        // neither the definition write nor the flow begin, so the gateway
        // never answers 400 and no immutable definition version registers as
        // a side effect of a refused start.
        buttonWithText("Start the authorization").click();
        await flush();
        expect(calls).toEqual([]);

        // Repairing the prefix reopens Start — the gate is live, not a latch.
        typeInto(labeledInput("Model prefix"), "repaired-");
        await flush();
        expect(buttonWithText("Start the authorization").disabled).toBe(false);
    });

    it("holds the flow start while the recipe references the base with none entered", async () => {
        vi.useFakeTimers();
        const calls: string[] = [];
        const starts: StartProviderOauthRequest[] = [];
        mountScreen(
            stubManagementApi({
                listProviderDefinitions: () =>
                    Promise.resolve({
                        provider_definitions: [],
                    }),
                createProviderDefinition: () => {
                    calls.push("definition");
                    return Promise.resolve({
                        provider_definition: storedDefinitionRow("claude-code", 1),
                    });
                },
                listProviders: () =>
                    Promise.resolve({
                        providers: [],
                    }),
                startProviderOauth: (request) => {
                    calls.push("start");
                    starts.push(request);
                    return Promise.resolve(authorizationFlow);
                },
            }),
        );
        await flush();
        buttonWithText("Claude Code").click();
        await flush();

        // The untouched Claude Code recipe pins its endpoints: no base-URL
        // field, and Start stands open.
        expect(
            [
                ...document.querySelectorAll("label"),
            ].map((label) => label.textContent),
        ).not.toContain("Upstream base URL");
        expect(buttonWithText("Start the authorization").disabled).toBe(false);

        // Editing the recipe to address the base opens the field, and the
        // requirement states itself the way every required field does — the
        // quiet hint from first paint, never alert-red — while Start is held.
        typeInto(definitionEditor(), JSON.stringify(baseAddressedPayload()));
        await flush();
        expect(document.body.textContent).toContain(
            "Required. The upstream address this definition's requests evaluate against",
        );
        expect(document.body.textContent).not.toContain(
            "Base URL must be an http:// or https:// address",
        );
        expect(buttonWithText("Start the authorization").disabled).toBe(true);
        buttonWithText("Start the authorization").click();
        await flush();
        expect(calls).toEqual([]);

        // Devin's recipe references the base from the picker on: the same
        // pristine contract with nothing typed — the hint, not an error —
        // and the gateway's refusal stays locked behind the held start. The
        // pick lands over the edited recipe, so it runs through the Preset
        // card's confirm before the Devin payload stands.
        buttonWithText("Devin").click();
        await flush();
        expect(document.body.textContent).toContain("Switch to Devin (Devin account)?");
        buttonWithText("Switch preset").click();
        await flush();
        expect(document.body.textContent).toContain(
            "Required. The upstream address this definition's requests evaluate against",
        );
        expect(document.body.textContent).not.toContain(
            "Base URL must be an http:// or https:// address",
        );
        expect(buttonWithText("Start the authorization").disabled).toBe(true);
        buttonWithText("Start the authorization").click();
        await flush();
        expect(calls).toEqual([]);

        // A typed but composition-invalid address keeps the hold and names
        // the rule where the value was typed…
        typeInto(labeledInput("Upstream base URL"), "not an address");
        await flush();
        expect(document.body.textContent).toContain(
            "Base URL must be an http:// or https:// address",
        );
        expect(buttonWithText("Start the authorization").disabled).toBe(true);

        // …and a valid one reopens the start, which then carries the base
        // URL the flow stores for the recipe's base-addressed requests.
        typeInto(labeledInput("Upstream base URL"), "https://upstream.example.com");
        await flush();
        expect(buttonWithText("Start the authorization").disabled).toBe(false);
        buttonWithText("Start the authorization").click();
        await flush();
        expect(calls).toEqual([
            "definition",
            "start",
        ]);
        expect(starts[0]?.base_url).toBe("https://upstream.example.com");
    });

    it("keeps polling a pending device flow on the configured interval", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        mountScreen(
            oauthScreenApi({
                start: () => deviceFlow((Date.now() + 600_000) * 1000),
                complete: () => {
                    completeCalls += 1;
                    return Promise.resolve({
                        status: "pending",
                    });
                },
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(completeCalls).toBe(1);

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(2);

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(3);
    });

    it("renders the device step without a verification link when the flow carries no URI", async () => {
        vi.useFakeTimers();
        mountScreen(
            oauthScreenApi({
                // Both verification URIs are optional on the wire (gateway
                // README §7.1): a flow without either leaves only the code.
                start: () => ({
                    grant: "device",
                    state: "state-d2",
                    expires_at: (Date.now() + 600_000) * 1000,
                    user_code: "WDJB-MJHT",
                }),
                complete: () =>
                    Promise.resolve({
                        status: "pending",
                    }),
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();

        // The user code alone remains: no anchor whose empty href would
        // navigate this app to itself, and no copy button for a URL that
        // does not exist.
        expect(document.body.textContent).toContain("WDJB-MJHT");
        expect(document.body.textContent).toContain("Verification code");
        expect(document.body.textContent).not.toContain("Open the verification page");
        expect(document.body.textContent).not.toContain("Copy the URL");
        for (const anchor of document.querySelectorAll("a")) {
            expect(anchor.getAttribute("href")).not.toBe("");
        }
    });

    it("stops the loop and states the flow's own verdict when a poll fails gone", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        mountScreen(
            oauthScreenApi({
                start: () => deviceFlow((Date.now() + 600_000) * 1000),
                complete: () => {
                    completeCalls += 1;
                    return Promise.reject(goneFailure("no such flow"));
                },
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();

        // The failed poll is the loop's end, and the gone answer states the
        // spent flow's own verdict — the flow named as the subject, the
        // gateway's message kept as the detail, the fresh-flow recovery as
        // the way forward — never the raw row diagnosis.
        expect(document.body.textContent).toContain(
            "This authorization flow is no longer completable — its state expired or was already used.",
        );
        expect(document.body.textContent).toContain("no such flow");
        expect(document.body.textContent).toContain("Start a fresh flow");
        expect(document.body.textContent).not.toContain("The gateway holds no such row.");
        expect(buttonWithText("Start the authorization").disabled).toBe(false);

        // The poll is the flow card's own step, so its diagnosis states
        // itself on that card — never swallowed into more polling, and never
        // on the page-top notice.
        expect(
            cardWithTitle("Connect the account").contains(paragraphWithText("no such flow")),
        ).toBe(true);

        const callsAfterFailure = completeCalls;
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs * 3);
        expect(completeCalls).toBe(callsAfterFailure);
    });

    it("keeps polling through a transient poll failure and clears its notice when a poll recovers", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        let isFailing = true;
        mountScreen(
            oauthScreenApi({
                start: () => deviceFlow((Date.now() + 600_000) * 1000),
                complete: () => {
                    completeCalls += 1;
                    if (isFailing) {
                        return Promise.reject(
                            new ManagementApiError("gateway", "upstream wobbled", {
                                errno: ErrorCode.UpstreamError,
                                httpStatus: 502,
                            }),
                        );
                    }

                    return Promise.resolve({
                        status: "pending",
                    });
                },
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();

        // A failed poll that did not prove the flow gone states the gateway's
        // message without claiming the flow is dead — no fresh-flow recovery —
        // and the loop re-arms: the gateway still holds the flow pending.
        expect(document.body.textContent).toContain("upstream wobbled");
        expect(document.body.textContent).not.toContain("Start a fresh flow");
        expect(document.body.textContent).toContain("Verification code");

        // The riding-out failure states itself on the flow card, beside the
        // polling step it belongs to.
        expect(
            cardWithTitle("Connect the account").contains(paragraphWithText("upstream wobbled")),
        ).toBe(true);

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(2);
        expect(document.body.textContent).toContain("upstream wobbled");

        // A poll that answers again makes the stated failure stale: the
        // notice clears and the loop keeps its interval.
        isFailing = false;
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(3);
        expect(document.body.textContent).not.toContain("upstream wobbled");

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(4);
    });

    it("stops the loop with the fresh-flow hint only after repeated consecutive failed polls", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        mountScreen(
            oauthScreenApi({
                start: () => deviceFlow((Date.now() + 600_000) * 1000),
                complete: () => {
                    completeCalls += 1;
                    return Promise.reject(
                        new ManagementApiError("gateway", "upstream wobbled", {
                            errno: ErrorCode.UpstreamError,
                            httpStatus: 502,
                        }),
                    );
                },
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();

        // The first failures ride out: the loop re-arms after each.
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(2);
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(3);

        // The run of failures exhausts the loop: the last failure is stated
        // with the fresh-flow recovery, the flow step stays for "Start over",
        // and no further ask is scheduled.
        expect(document.body.textContent).toContain("upstream wobbled");
        expect(document.body.textContent).toContain("Start a fresh flow");
        expect(document.body.textContent).toContain("Verification code");

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs * 3);
        expect(completeCalls).toBe(3);
    });

    it("stops polling at the flow's expiry and offers a fresh flow", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        mountScreen(
            oauthScreenApi({
                start: () => deviceFlow((Date.now() + 5_000) * 1000),
                complete: () => {
                    completeCalls += 1;
                    return Promise.resolve({
                        status: "pending",
                    });
                },
            }),
        );
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(completeCalls).toBe(1);

        // One ask while the flow is still live keeps the loop going.
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(2);

        // Past the expiry the loop stops and the failure states it, instead of
        // asking the gateway for an answer it can no longer give.
        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs);
        expect(completeCalls).toBe(2);
        expect(document.body.textContent).toContain("The flow expired.");
        expect(buttonWithText("Start the authorization").disabled).toBe(false);
    });

    it("neither re-arms the poll nor navigates once the screen is gone", async () => {
        vi.useFakeTimers();
        let completeCalls = 0;
        // A holder rather than a bare `let`: the assignment happens inside the
        // stubbed completion below, and a bare variable would be narrowed to
        // `null` at the release site.
        const release: {
            current: ((response: CompleteProviderOauthResponse) => void) | null;
        } = {
            current: null,
        };
        const api = oauthScreenApi({
            start: () => deviceFlow((Date.now() + 600_000) * 1000),
            complete: () => {
                completeCalls += 1;
                return new Promise<CompleteProviderOauthResponse>((resolve) => {
                    release.current = resolve;
                });
            },
        });
        const dispose = render(() => <ProviderNewScreen api={api} />, document.body);
        disposes.push(dispose);
        await flush();
        buttonWithText("Kimi Code").click();
        await flush();
        buttonWithText("Start the authorization").click();
        await flush();
        expect(completeCalls).toBe(1);

        // The poll is still in flight when the visitor leaves the screen; its
        // late resolution must neither schedule another ask nor act on the
        // answer.
        const pathnameBefore = window.location.pathname;
        dispose();
        release.current?.({
            status: "pending",
        });
        await flush();

        await vi.advanceTimersByTimeAsync(appConfig.oauthDevicePollIntervalMs * 3);
        expect(completeCalls).toBe(1);
        expect(window.location.pathname).toBe(pathnameBefore);
    });
});
