import { afterEach } from "vitest";
import type { ManagementApi } from "../management-api/client";
import { ManagementApiError } from "../management-api/errors";
import type {
    ApiKeyResponse,
    ApiLogResponse,
    ModelResponse,
    ModelRouteResponse,
    ProviderResponse,
    UsageBucket,
} from "../management-api/generated/v0";
import { ErrorCode } from "../management-api/generated/v0";
import type { OauthProviderRow } from "../management-api/vocabulary";

/**
 * Test support the suites share: the full management API stub the screen
 * mounts run against, the entity-listing fragment its suites answer fixture
 * rows from, the shared entity-row fixtures their listings are built from,
 * the shared usage-row and day-bucket fixtures their pages are built from,
 * the gateway failures their stubs reject with, the held-call helper that
 * keeps a route's promise in flight until the test ends it, the answers a
 * test ends a held call with, the promise drains the suites' exchanges rely
 * on, the suite-scoped disposal list their mounts register into, and the
 * DOM lookups, keystrokes, and select choices their user interactions drive.
 */

/** Rejects the way every route a screen test did not expect must: loudly. */
function refusedRoute(route: keyof ManagementApi): Promise<never> {
    return Promise.reject(new Error(`route not expected in test: ${route}`));
}

/**
 * The full route table a screen test mounts against: the given overrides
 * answer, and every route they leave out rejects with a refusal naming the
 * route, so an unexpected call fails the test loudly instead of hanging it.
 * The table is typed against the real `ManagementApi`, so a route the
 * gateway adds or renames breaks this module — and only this module — at
 * compile time.
 */
export function stubManagementApi(overrides: Partial<ManagementApi>): ManagementApi {
    return {
        listProviderPools: overrides.listProviderPools ?? (() => refusedRoute("listProviderPools")),
        createProviderPool:
            overrides.createProviderPool ?? (() => refusedRoute("createProviderPool")),
        updateProviderPool:
            overrides.updateProviderPool ?? (() => refusedRoute("updateProviderPool")),
        deleteProviderPool:
            overrides.deleteProviderPool ?? (() => refusedRoute("deleteProviderPool")),
        listModelRoutes: overrides.listModelRoutes ?? (() => refusedRoute("listModelRoutes")),
        createModelRoute: overrides.createModelRoute ?? (() => refusedRoute("createModelRoute")),
        updateModelRoute: overrides.updateModelRoute ?? (() => refusedRoute("updateModelRoute")),
        deleteModelRoute: overrides.deleteModelRoute ?? (() => refusedRoute("deleteModelRoute")),
        getProviderAvailability:
            overrides.getProviderAvailability ?? (() => refusedRoute("getProviderAvailability")),
        resetProviderAvailability:
            overrides.resetProviderAvailability ??
            (() => refusedRoute("resetProviderAvailability")),

        listProviderDefinitions:
            overrides.listProviderDefinitions ?? (() => refusedRoute("listProviderDefinitions")),
        createProviderDefinition:
            overrides.createProviderDefinition ?? (() => refusedRoute("createProviderDefinition")),
        updateProviderDefinition:
            overrides.updateProviderDefinition ?? (() => refusedRoute("updateProviderDefinition")),
        getProviderQuota: overrides.getProviderQuota ?? (() => refusedRoute("getProviderQuota")),
        listProviders: overrides.listProviders ?? (() => refusedRoute("listProviders")),
        createProvider: overrides.createProvider ?? (() => refusedRoute("createProvider")),
        updateProvider: overrides.updateProvider ?? (() => refusedRoute("updateProvider")),
        deleteProvider: overrides.deleteProvider ?? (() => refusedRoute("deleteProvider")),
        startProviderOauth:
            overrides.startProviderOauth ?? (() => refusedRoute("startProviderOauth")),
        completeProviderOauth:
            overrides.completeProviderOauth ?? (() => refusedRoute("completeProviderOauth")),
        reauthorizeProvider:
            overrides.reauthorizeProvider ?? (() => refusedRoute("reauthorizeProvider")),
        listModels: overrides.listModels ?? (() => refusedRoute("listModels")),
        discoverModels: overrides.discoverModels ?? (() => refusedRoute("discoverModels")),
        updateModel: overrides.updateModel ?? (() => refusedRoute("updateModel")),
        createModel: overrides.createModel ?? (() => refusedRoute("createModel")),
        deleteModel: overrides.deleteModel ?? (() => refusedRoute("deleteModel")),
        listUsage: overrides.listUsage ?? (() => refusedRoute("listUsage")),
        queryUsage: overrides.queryUsage ?? (() => refusedRoute("queryUsage")),
        listApiKeys: overrides.listApiKeys ?? (() => refusedRoute("listApiKeys")),
        createApiKey: overrides.createApiKey ?? (() => refusedRoute("createApiKey")),
        updateApiKey: overrides.updateApiKey ?? (() => refusedRoute("updateApiKey")),
        deleteApiKey: overrides.deleteApiKey ?? (() => refusedRoute("deleteApiKey")),
    };
}

/**
 * The entity-listing fragment of the route table: each of the four listings
 * answers from the given rows — a kind left out answers empty — wrapped the
 * way the wire answers it. Suites spread this fragment over their own
 * remaining overrides inside `stubManagementApi`, keeping their other stub
 * members local.
 */
export function stubListings(rows: {
    readonly providers?: ReadonlyArray<ProviderResponse>;
    readonly models?: ReadonlyArray<ModelResponse>;
    readonly apiKeys?: ReadonlyArray<ApiKeyResponse>;
    readonly modelRoutes?: ReadonlyArray<ModelRouteResponse>;
    readonly pools?: ReadonlyArray<import("/src/management-api/generated/v0").ProviderPoolResponse>;
}): Partial<ManagementApi> {
    return {
        listProviderPools: () =>
            Promise.resolve({
                provider_pools: [
                    ...(rows.pools ?? []),
                ],
            }),
        listModelRoutes: () =>
            Promise.resolve({
                model_routes: [
                    ...(rows.modelRoutes ?? []),
                ],
            }),
        listProviders: () =>
            Promise.resolve({
                providers: [
                    ...(rows.providers ?? []),
                ],
            }),
        listModels: () =>
            Promise.resolve({
                models: [
                    ...(rows.models ?? []),
                ],
            }),
        listApiKeys: () =>
            Promise.resolve({
                api_keys: [
                    ...(rows.apiKeys ?? []),
                ],
            }),
    };
}

/** The static-key member of the provider row union. */
type ApiKeyProviderRow = Extract<
    ProviderResponse,
    {
        type: "api_key";
    }
>;

/**
 * The static-key row the shared provider fixture defaults to: the seed both
 * union members grow from, since every column but the kind and the OAuth
 * expiry carries the same type on either side.
 */
const defaultProviderRow: ApiKeyProviderRow = {
    model_prefix: "",
    type: "api_key",
    id: 1,
    name: "local",
    is_enabled: true,
    provider_definition_name: "generic-openai-chat-completions",
    provider_definition_version: 1,
    format: "openai_chat_completion",
    base_url: "https://upstream.example.com/v1",
};

/** The columns a suite may override while the row stays a static-key provider. */
type ApiKeyProviderRowOverrides = Partial<Omit<ApiKeyProviderRow, "type">> & {
    type?: "api_key";
};

/**
 * The columns a suite may override on an OAuth provider row: the kind switch
 * is required, so an expiry cannot be set without declaring the kind, and a
 * switch without its expiry does not type-check either.
 */
type OauthProviderRowOverrides = Partial<Omit<OauthProviderRow, "type">> & {
    type: "oauth";
};

/**
 * One `providers:list` row the suites share: the canonical row is a static-key
 * provider, and the overrides adjust the columns a suite's assertions read. A
 * row overridden into the OAuth kind answers at its member type, so a suite
 * deriving a second OAuth row from it by spread keeps type-checking.
 */
export function providerRow(overrides?: ApiKeyProviderRowOverrides): ProviderResponse;
export function providerRow(overrides: OauthProviderRowOverrides): OauthProviderRow;
export function providerRow(
    overrides: ApiKeyProviderRowOverrides | OauthProviderRowOverrides = {},
): ProviderResponse {
    // The kind is destructured ahead of the spread because the union does not
    // narrow through the member check: the rest object is what either branch
    // spreads, and each branch's explicit kind and expiry keep the row a
    // valid union member whatever the rest carries.
    const { type: kind, ...rest } = overrides;

    if (kind === "oauth") {
        return {
            ...defaultProviderRow,
            type: "oauth",
            expires_at: null,
            ...rest,
        };
    }

    return {
        ...defaultProviderRow,
        ...rest,
    };
}

/**
 * One `models:list` row the suites share: the routing model of the canonical
 * provider row, with the overrides adjusting the columns a suite's assertions
 * read.
 */
export function modelRow(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 9,
        provider_id: 1,
        provider: "local",
        upstream_id: "qwen3:32b",

        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
        ...overrides,
    };
}

/**
 * One `api-keys:list` row the suites share: the canonical issued key, with
 * the overrides adjusting the columns a suite's assertions read.
 */
export function apiKeyRow(overrides: Partial<ApiKeyResponse> = {}): ApiKeyResponse {
    return {
        id: 7,
        name: "service-a",
        key: "sk-live",
        is_enabled: true,
        created_at: 1_700_000_000_000_000,
        access_control: {
            default_access: "deny",
            owner_defaults: [],
            route_overrides: [],
        },
        ...overrides,
    };
}

/** The base instant the shared usage row's stamp offsets, marking rows by it. */
export const API_LOG_CURSOR_BASE = 1_700_000_000_000_000;

/**
 * One `usage:list` row the suites share: the stamp is the base instant plus
 * the offset marking the row — a page's continuation cursor derives from it —
 * the entity ids are fixed, and the overrides adjust the columns a suite's
 * assertions read.
 */
export function apiLogRow(offset: number, overrides: Partial<ApiLogResponse> = {}): ApiLogResponse {
    return {
        started_at: API_LOG_CURSOR_BASE + offset,
        model_route_id: 17,
        upstream_http_status: 200,
        api_key_id: 7,
        provider_id: 1,
        model_id: 9,
        endpoint_type: "openai_chat_completions",
        is_stream: false,
        http_status: 200,
        outcome: "completed",
        duration: 120,
        input_tokens: 10,
        output_tokens: 20,
        cached_tokens: null,
        cache_write_tokens: null,
        reasoning_tokens: null,
        ...overrides,
    };
}

/**
 * One `usage:query` day bucket the suites share: the counted day the usage
 * answers render, with the overrides adjusting the columns a suite's
 * assertions read.
 */
export function dayBucket(overrides: Partial<UsageBucket> = {}): UsageBucket {
    return {
        key: {
            day: "2026-09-01",
        },
        requests: 3,
        input_tokens: 120,
        output_tokens: 45,
        cached_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        ...overrides,
    };
}

/**
 * One gateway-rendered failure a stub rejects with: the gateway's message
 * verbatim, carrying the errno and HTTP status the test names.
 */
export function gatewayFailure(
    message: string,
    errno: ErrorCode,
    httpStatus: number,
): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno,
        httpStatus,
    });
}

/** The gone-row failure every refused row address answers: errno 3, HTTP 404. */
export function goneFailure(message: string): ManagementApiError {
    return gatewayFailure(message, ErrorCode.NotFound, 404);
}

/** The taken-name failure every collision answers: errno 4, HTTP 409. */
export function conflictFailure(message: string): ManagementApiError {
    return gatewayFailure(message, ErrorCode.Conflict, 409);
}

/**
 * One held call a test ends by hand: `answer` resolves it, `refuse` rejects
 * it with the cause the test names, and — where the call's request carries
 * assertions — `request` is the request the call carried.
 */
export type HeldCall<TValue, TRequest = undefined> = {
    readonly request?: TRequest;
    readonly answer: (result: TValue) => void;
    readonly refuse: (cause: unknown) => void;
};

/**
 * One route call held back until the test ends it, rejecting with the
 * transport's abort failure the moment its cycle's signal fires — the same
 * shape a real in-flight fetch dies with. The ways to end it land in the
 * given holder as one `HeldCall`; where the caller passes the call's
 * request, the record carries it for the assertions that read one.
 */
export function heldCall<TValue>(
    signal: AbortSignal | undefined,
    held: Array<HeldCall<TValue>>,
): Promise<TValue>;
export function heldCall<TRequest, TValue>(
    signal: AbortSignal | undefined,
    held: Array<HeldCall<TValue, TRequest>>,
    request: TRequest,
): Promise<TValue>;
export function heldCall<TValue, TRequest = undefined>(
    signal: AbortSignal | undefined,
    held: Array<HeldCall<TValue, TRequest>>,
    request?: TRequest,
): Promise<TValue> {
    return new Promise<TValue>((resolve, reject) => {
        held.push({
            ...(request === undefined
                ? {}
                : {
                      request,
                  }),
            answer: resolve,
            refuse: reject,
        });
        signal?.addEventListener(
            "abort",
            () => {
                reject(new DOMException("This read cycle was aborted.", "AbortError"));
            },
            {
                once: true,
            },
        );
    });
}

/**
 * Answers the most recent held call of one route — the newest cycle's call.
 * Throws when the route holds no call, so an answer aimed at a cycle that
 * never started fails the test loudly instead of hanging it.
 */
export function answerLast<TValue, TRequest = undefined>(
    pending: Array<HeldCall<TValue, TRequest>>,
    value: TValue,
): void {
    const held = pending.at(-1);

    if (held === undefined) {
        throw new Error("No held call to answer");
    }

    held.answer(value);
}

/**
 * Answers one held call by position — a superseded cycle's own, late answer.
 * Throws when no call stands at the position, so an answer aimed at nothing
 * fails the test loudly instead of hanging it.
 */
export function answerAt<TValue, TRequest = undefined>(
    pending: Array<HeldCall<TValue, TRequest>>,
    position: number,
    value: TValue,
): void {
    const held = pending.at(position);

    if (held === undefined) {
        throw new Error(`No held call at position ${position}`);
    }

    held.answer(value);
}

/** Drains the pending promises and scheduled work of one screen exchange. */
export async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

/**
 * Drains exactly one macrotask tick: the round a settled Solid signal write
 * needs before a test can observe it. The unit suites over bare signals
 * prefer this one round — their writes cascade no deeper; only a screen
 * mount's exchange spans enough awaiting to need the fuller `settle`.
 */
export function drainWrites(): Promise<void> {
    return new Promise((resolve) => {
        setTimeout(resolve, 0);
    });
}

/**
 * The disposal list of one suite scope: what a test pushes here is disposed
 * newest first after the test ends, leaving the list empty for the next one.
 * The `afterEach` draining it registers at the suite scope the call sits in —
 * a `describe` callback's or the file's — so the call belongs exactly where
 * the suite declares its list.
 */
export function createDisposals(): Array<() => void> {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    return disposes;
}

/** The button whose visible label contains the given text. */
export function buttonWithText(text: string): HTMLButtonElement {
    for (const candidate of document.querySelectorAll("button")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate as HTMLButtonElement;
        }
    }

    throw new Error(`No button reading ${text}`);
}

/**
 * The labels whose visible text is exactly the given text, in document
 * order — the one label walk every label lookup resolves through. Exact
 * text, never a substring match: a `contains` walk answers a longer label
 * that merely embeds the asked-for text. Distinct controls can share a
 * visible text while pairing differently — a dimension switch's label and
 * a filter select's label both read `Outcome` — so each caller resolves
 * its own pairing over the owners and picks the first that pairs its way.
 */
function labelsWithText(label: string): Array<HTMLLabelElement> {
    return Array.from(document.querySelectorAll("label")).filter(
        (candidate) => candidate.textContent === label,
    );
}

/** The input the given field label names, through the label's `for` pairing. */
export function labeledInput(label: string): HTMLInputElement {
    for (const owner of labelsWithText(label)) {
        const id = owner.getAttribute("for");
        const input = id === null ? null : document.getElementById(id);

        if (input instanceof HTMLInputElement) {
            return input;
        }
    }

    throw new Error(`No input labeled ${label}`);
}

/** The select the given field label names, through the label's `for` pairing. */
export function labeledSelect(label: string): HTMLSelectElement {
    for (const owner of labelsWithText(label)) {
        const id = owner.getAttribute("for");
        const select = id === null ? null : document.getElementById(id);

        if (select instanceof HTMLSelectElement) {
            return select;
        }
    }

    throw new Error(`No select labeled ${label}`);
}

/**
 * The checkbox the given label text wraps — the toggle-label pairing, which
 * carries no `for` attribute, so the lookup takes the label's own input and
 * holds it to the checkbox type.
 */
export function labeledCheckbox(label: string): HTMLInputElement {
    for (const owner of labelsWithText(label)) {
        const control = owner.querySelector("input");

        if (control instanceof HTMLInputElement && control.type === "checkbox") {
            return control;
        }
    }

    throw new Error(`No checkbox labeled ${label}`);
}

/** The search input the given aria-label names, as every listing's is. */
export function searchInput(ariaLabel: string): HTMLInputElement {
    const input = document.querySelector(`input[aria-label='${ariaLabel}']`);

    if (input instanceof HTMLInputElement) {
        return input;
    }

    throw new Error(`No search input labeled ${ariaLabel}`);
}

/** One option's value, label, and selected state, as the DOM holds them. */
export function optionRepr(option: HTMLOptionElement): {
    value: string;
    label: string;
    isSelected: boolean;
} {
    return {
        value: option.value,
        label: option.textContent ?? "",
        isSelected: option.selected,
    };
}

/** The switch the given aria-label names. */
export function switchWithLabel(label: string): HTMLButtonElement {
    const candidate = document.querySelector(`button[role='switch'][aria-label='${label}']`);

    if (candidate === null) {
        throw new Error(`No switch labeled ${label}`);
    }

    return candidate as HTMLButtonElement;
}

/** The label element the given switch's visible text stands in. */
export function switchLabel(label: string): HTMLLabelElement {
    const owner = labelsWithText(label).at(0);

    if (owner === undefined) {
        throw new Error(`No switch label ${label}`);
    }

    return owner;
}

/** The page's whole text content, as the assertions read it. */
export function bodyText(): string {
    return document.body.textContent ?? "";
}

/** The card section whose header reads the given title. */
export function cardWithTitle(title: string): HTMLElement {
    for (const candidate of document.querySelectorAll("section")) {
        if (candidate.querySelector("h2")?.textContent === title) {
            return candidate;
        }
    }

    throw new Error(`No card titled ${title}`);
}

/** The paragraph whose text contains the given text, failing when none renders. */
export function paragraphWithText(text: string): HTMLParagraphElement {
    for (const candidate of document.querySelectorAll("p")) {
        if (candidate.textContent?.includes(text) === true) {
            return candidate;
        }
    }

    throw new Error(`No paragraph reading ${text}`);
}

/** The anchor addressing the given href. */
export function linkTo(href: string): HTMLAnchorElement {
    for (const candidate of document.querySelectorAll("a")) {
        if (candidate.getAttribute("href") === href) {
            return candidate;
        }
    }

    throw new Error(`No link to ${href}`);
}

/** Types into an input or textarea the way a browser delivers an edit. */
export function typeInto(input: HTMLInputElement | HTMLTextAreaElement, value: string): void {
    input.value = value;
    input.dispatchEvent(
        new Event("input", {
            bubbles: true,
        }),
    );
}

/**
 * Chooses the select the given field label names the way a browser delivers
 * a choice: the option is set, then the change event fires and bubbles the
 * way a real one does, reaching the select's own change handling.
 */
export function chooseSelect(label: string, value: string): void {
    const select = labeledSelect(label);
    select.value = value;
    select.dispatchEvent(
        new Event("change", {
            bubbles: true,
        }),
    );
}
