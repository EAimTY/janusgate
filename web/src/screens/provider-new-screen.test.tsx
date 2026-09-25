import { render } from "@solidjs/web";
import { afterEach, describe, expect, it, vi } from "vitest";
import { appConfig } from "/src/config";
import type { ManagementApi } from "/src/management-api/client";
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
import { PROVIDER_PRESETS } from "/src/management-api/presets";
import { hrefs } from "/src/router";
import { ProviderNewScreen } from "/src/screens/provider-new-screen";

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
            providers: [];
        }>;
        readonly definitions?: () => Promise<{
            provider_definitions: DefinitionResponse[];
        }>;
    } = {},
): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
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
        updateProviderDefinition: refused,
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
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: refused,
        completeProviderOauth: refused,
        reauthorizeProvider: refused,
        listModels: refused,
        discoverModels: refused,
        updateModel: refused,
        createModel: refused,
        deleteModel: refused,
        listUsage: refused,
        queryUsage: refused,
        listApiKeys: refused,
        createApiKey: refused,
        updateApiKey: refused,
        deleteApiKey: refused,
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

/** Types into an input or textarea the way a browser delivers an edit. */
function typeInto(field: HTMLInputElement | HTMLTextAreaElement, value: string): void {
    field.value = value;
    field.dispatchEvent(
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
        typeInto(labeledInput("API key"), "sk-test");
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
        typeInto(labeledInput("API key"), "sk-test");
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
        expect(labels).toContain("API key");

        typeInto(labeledInput("API key"), "sk-zen");
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
        typeInto(labeledInput("API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        expect(sent).toHaveLength(1);
        expect(sent[0]?.provider_definition_name).toBe("generic-openai-responses");
        expect(sent[0]?.provider_definition_version).toBe(2);
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
        typeInto(labeledInput("API key"), "sk-test");
        await settle();
        buttonWithText("Create provider").click();
        await settle();

        // The conflict is the definition write's — its name is the preset id,
        // nothing this page edits — so the fix names the moved stored version,
        // never the provider-name recovery a provider conflict carries.
        expect(document.body.textContent).toContain("definition name already stored");
        expect(document.body.textContent).toContain("The stored version moved");
        expect(document.body.textContent).toContain("reload this page and submit again");
        expect(document.body.textContent).not.toContain("Choose a provider name");
        expect(sent).toHaveLength(0);
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
const createdProvider: ProviderResponse = {
    type: "oauth",
    expires_at: 1_700_000_100_000_000,
    id: 7,
    name: "kimi",
    model_prefix: "kimi-",
    is_enabled: true,
    provider_definition_name: "kimi-code",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: null,
};

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
    readonly start?: (request: StartProviderOauthRequest) => StartProviderOauthResponse;
    readonly complete?: (
        request: CompleteProviderOauthRequest,
    ) => Promise<CompleteProviderOauthResponse>;
}): ManagementApi {
    const refused = (): Promise<never> => Promise.reject(new Error("route not expected in test"));

    return {
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
        updateProviderDefinition: refused,
        listProviders: () =>
            Promise.resolve({
                providers: [],
            }),
        createProvider: refused,
        updateProvider: refused,
        deleteProvider: refused,
        startProviderOauth: (request) =>
            options.start === undefined ? refused() : Promise.resolve(options.start(request)),
        completeProviderOauth: (request) =>
            options.complete === undefined ? refused() : options.complete(request),
        reauthorizeProvider: refused,
        listModels: refused,
        discoverModels: refused,
        updateModel: refused,
        createModel: refused,
        deleteModel: refused,
        listUsage: refused,
        queryUsage: refused,
        listApiKeys: refused,
        createApiKey: refused,
        updateApiKey: refused,
        deleteApiKey: refused,
    };
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

    it("stops the loop and states the diagnosis when a poll fails gone", async () => {
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

        // The failed poll is the loop's end, and the failure is stated where
        // the screen states them — never swallowed into more polling.
        expect(document.body.textContent).toContain("The gateway holds no such row.");
        expect(document.body.textContent).toContain("no such flow");
        expect(buttonWithText("Start the authorization").disabled).toBe(false);

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
