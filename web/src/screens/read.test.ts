import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";
import { requestRefresh } from "/src/refresh-state";
import {
    createHeldRow,
    createScreenRead,
    isSameApiKeyRow,
    isSameModelRow,
    isSameProviderRow,
    type ReadCall,
    ReadCallFailure,
    readAll,
    readOne,
} from "./read";

function gatewayFailure(errno: ErrorCode, message: string): ManagementApiError {
    return new ManagementApiError("gateway", message, {
        errno,
        httpStatus: 404,
    });
}

function abortFailure(): Promise<never> {
    return Promise.reject(new DOMException("The operation was aborted.", "AbortError"));
}

function call<TResult>(result: Promise<TResult>): ReadCall<TResult> {
    return {
        run: () => result,
    };
}

describe("readAll", () => {
    it("answers every call's result in call order", async () => {
        const controller = new AbortController();

        const [first, second] = await readAll(
            controller.signal,
            call(
                Promise.resolve({
                    providers: [],
                }),
            ),
            call(
                Promise.resolve({
                    models: [],
                }),
            ),
        );

        expect(first).toEqual({
            providers: [],
        });
        expect(second).toEqual({
            models: [],
        });
    });

    it("fails with the first failed call's diagnosis when several calls fail", async () => {
        const controller = new AbortController();

        const failure = await readAll(
            controller.signal,
            {
                run: () => Promise.reject(gatewayFailure(ErrorCode.NotFound, "gone row")),
            },
            {
                run: () => Promise.reject(gatewayFailure(ErrorCode.Conflict, "taken name")),
            },
        ).catch((error: unknown) => error);

        expect(failure).toBeInstanceOf(ReadCallFailure);
        expect((failure as ReadCallFailure).notice.tone).toBe("bad");
        expect((failure as ReadCallFailure).notice.detail).toBe("gone row");
    });

    it("treats an aborted call as the cycle's end, ahead of any failure", async () => {
        const controller = new AbortController();

        const failure = await readAll(controller.signal, call(abortFailure()), {
            run: () => Promise.reject(gatewayFailure(ErrorCode.NotFound, "gone row")),
        }).catch((error: unknown) => error);

        expect((failure as Error).name).toBe("AbortError");
    });
});

describe("readOne", () => {
    it("answers the call's result", async () => {
        const controller = new AbortController();

        const result = await readOne(
            controller.signal,
            call(
                Promise.resolve({
                    buckets: [],
                }),
            ),
        );

        expect(result).toEqual({
            buckets: [],
        });
    });

    it("fails with the call's diagnosis, carrying its action into the fix", async () => {
        const controller = new AbortController();

        const failure = await readOne(controller.signal, {
            action: "query-usage",
            run: () => Promise.reject(gatewayFailure(ErrorCode.UpstreamError, "upstream down")),
        }).catch((error: unknown) => error);

        expect(failure).toBeInstanceOf(ReadCallFailure);
        expect((failure as ReadCallFailure).notice.detail).toBe("upstream down");
        expect((failure as ReadCallFailure).notice.fix).toContain("credentials");
    });

    it("propagates an abort as control flow", async () => {
        const controller = new AbortController();

        const failure = await readOne(controller.signal, call(abortFailure())).catch(
            (error: unknown) => error,
        );

        expect((failure as Error).name).toBe("AbortError");
    });
});

/** The harnesses mounted by the running test; every one is disposed after it. */
const mountedDisposes: Array<() => void> = [];

/** Mounts one harness outside a component, registered for disposal. */
function mountedRead(options: Parameters<typeof createScreenRead>[0]) {
    const box: {
        read?: ReturnType<typeof createScreenRead>;
    } = {};
    const dispose = createRoot((teardown) => {
        box.read = createScreenRead(options);
        return teardown;
    });

    if (box.read === undefined) {
        throw new Error("The harness did not mount");
    }

    mountedDisposes.push(dispose);

    return {
        read: box.read,
        dispose,
    };
}

/** Drains the pending promises and scheduled work of one harness exchange. */
async function settle(): Promise<void> {
    for (let round = 0; round < 10; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

/** Waits until the load has started the asked number of cycles. */
async function waitForCycles(state: ReturnType<typeof manualLoad>, count: number): Promise<void> {
    for (let round = 0; round < 100 && state.cycles.length < count; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }

    if (state.cycles.length < count) {
        throw new Error(`Only ${state.cycles.length} of ${count} read cycles started`);
    }
}

/** A load whose cycles are settled by hand, exposing each cycle's signal. */
function manualLoad(): {
    load: (signal: AbortSignal) => Promise<void>;
    cycles: Array<{
        signal: AbortSignal;
        succeed: () => void;
        fail: () => void;
    }>;
} {
    const cycles: Array<{
        signal: AbortSignal;
        succeed: () => void;
        fail: () => void;
    }> = [];

    return {
        cycles,
        load: (signal) =>
            new Promise<void>((resolve, reject) => {
                cycles.push({
                    signal,
                    succeed: () => {
                        resolve();
                    },
                    fail: () => {
                        reject(new ReadCallFailure(gatewayFailure(ErrorCode.NotFound, "late")));
                    },
                });
            }),
    };
}

describe("createScreenRead", () => {
    afterEach(() => {
        for (const dispose of mountedDisposes.reverse()) {
            dispose();
        }
        mountedDisposes.length = 0;
    });

    it("installs nothing from a superseded cycle's late failure", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);
        expect(read.isReading()).toBe(true);

        requestRefresh();
        await waitForCycles(state, 2);
        expect(state.cycles[0]?.signal.aborted).toBe(true);

        state.cycles[1]?.succeed();
        await settle();
        expect(read.hasRead()).toBe(true);
        expect(read.notice()).toBeNull();

        state.cycles[0]?.fail();
        await settle();
        expect(read.notice()).toBeNull();
        expect(read.isReading()).toBe(false);
    });

    it("keeps an action notice across a successful read when the policy says so", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
            isNoticeKeptOnSuccess: true,
        });
        await waitForCycles(state, 1);
        state.cycles[0]?.succeed();
        await settle();

        read.showNotice({
            tone: "ok",
            title: "Saved.",
        });

        requestRefresh();
        await waitForCycles(state, 2);
        state.cycles[1]?.succeed();
        await settle();

        expect(state.cycles.length).toBe(2);
        expect(read.notice()).toEqual({
            tone: "ok",
            title: "Saved.",
        });
    });

    it("clears the standing notice on a successful read without the keep policy", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);
        state.cycles[0]?.succeed();
        await settle();

        read.showNotice({
            tone: "ok",
            title: "Saved.",
        });

        requestRefresh();
        await waitForCycles(state, 2);
        state.cycles[1]?.succeed();
        await settle();

        expect(state.cycles.length).toBe(2);
        expect(read.notice()).toBeNull();
    });

    it("leaves reload inert once the screen is disposed", async () => {
        const state = manualLoad();
        const { read, dispose } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);
        state.cycles[0]?.succeed();
        await settle();
        const settledCycleCount = state.cycles.length;

        dispose();
        await settle();

        read.reload();
        await settle();

        expect(state.cycles.length).toBe(settledCycleCount);
    });

    it("re-runs on a route-identity change and aborts the prior cycle", async () => {
        const [source, setSource] = createSignal(1);
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
            reReadOn: source,
        });
        await waitForCycles(state, 1);

        setSource(2);
        await waitForCycles(state, 2);
        expect(state.cycles[0]?.signal.aborted).toBe(true);

        state.cycles[1]?.succeed();
        await settle();
        expect(read.hasRead()).toBe(true);
        expect(read.notice()).toBeNull();

        state.cycles[0]?.fail();
        await settle();
        expect(read.notice()).toBeNull();
    });
});

describe("row-identity predicates", () => {
    const staticProvider: ProviderResponse = {
        type: "openai_response",
        id: 3,
        name: "alpha",
        model_prefix: "alpha/",
        is_enabled: true,
        base_url: "https://alpha.example",
    };

    const oauthProvider: ProviderResponse = {
        type: "codex_oauth",
        id: 4,
        name: "codex",
        model_prefix: "codex/",
        is_enabled: true,
        expires_at: 1_700_000_000_000_000,
    };

    const modelRow: ModelResponse = {
        id: 11,
        provider_id: 3,
        provider: "alpha",
        upstream_id: "gpt-x",
        public_id: "alpha/gpt-x",
        id_override: null,
        is_enabled: true,
        discovery_state: "discovered",
    };

    const apiKeyRow: ApiKeyResponse = {
        id: 7,
        name: "service-a",
        key: "sk-live",
        is_enabled: true,
        created_at: 1_700_000_000_000_000,
    };

    it("keeps a static provider row's identity only while every field stands", () => {
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
            }),
        ).toBe(true);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                type: "openai_chat_completion",
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                id: 9,
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                name: "beta",
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                model_prefix: "beta/",
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                is_enabled: false,
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...staticProvider,
                base_url: "https://beta.example",
            }),
        ).toBe(false);
    });

    it("keeps an OAuth provider row's identity on its expiry and breaks across the union", () => {
        expect(
            isSameProviderRow(oauthProvider, {
                ...oauthProvider,
            }),
        ).toBe(true);
        expect(
            isSameProviderRow(oauthProvider, {
                ...oauthProvider,
                expires_at: null,
            }),
        ).toBe(false);
        // Same scalar fields, the other union member: never the same row.
        expect(
            isSameProviderRow(oauthProvider, {
                ...staticProvider,
                id: oauthProvider.id,
            }),
        ).toBe(false);
        expect(
            isSameProviderRow(staticProvider, {
                ...oauthProvider,
                id: staticProvider.id,
            }),
        ).toBe(false);
        expect(isOAuthProviderRow(oauthProvider)).toBe(true);
    });

    it("keeps a model row's identity only while every field stands", () => {
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
            }),
        ).toBe(true);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                provider_id: 9,
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                provider: "beta",
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                upstream_id: "gpt-y",
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                public_id: "beta/gpt-x",
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                id_override: "beta/gpt-x",
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                is_enabled: false,
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                discovery_state: "manual",
            }),
        ).toBe(false);
    });

    it("keeps an API-key row's identity only while every field stands", () => {
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
            }),
        ).toBe(true);
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
                name: "service-b",
            }),
        ).toBe(false);
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
                key: "sk-next",
            }),
        ).toBe(false);
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
                is_enabled: false,
            }),
        ).toBe(false);
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
                created_at: 1,
            }),
        ).toBe(false);
        expect(
            isSameApiKeyRow(apiKeyRow, {
                ...apiKeyRow,
                id: 8,
            }),
        ).toBe(false);
    });
});

describe("createHeldRow", () => {
    function mountedKey() {
        const [latest, setLatest] = createSignal<ApiKeyResponse | null>(null);
        const [isFormDirty, setIsFormDirty] = createSignal(false);
        const box: {
            held?: ReturnType<typeof createHeldRow<ApiKeyResponse>>;
        } = {};
        const dispose = createRoot((teardown) => {
            box.held = createHeldRow(latest, isFormDirty, isSameApiKeyRow);
            return teardown;
        });

        if (box.held === undefined) {
            throw new Error("The held row did not mount");
        }

        return {
            held: box.held,
            setLatest,
            setIsFormDirty,
            dispose,
        };
    }

    const key: ApiKeyResponse = {
        id: 7,
        name: "service-a",
        key: "sk-live",
        is_enabled: true,
        created_at: 1,
    };

    afterEach(() => {
        for (const dispose of mountedDisposes.reverse()) {
            dispose();
        }
        mountedDisposes.length = 0;
    });

    it("holds the held row against a changed read while the form is dirty, and says so", async () => {
        const { held, setLatest, setIsFormDirty, dispose } = mountedKey();

        expect(held.row()).toBeNull();
        expect(held.isRowHeld()).toBe(false);

        setLatest(key);
        await settle();
        expect(held.row()).toBe(key);

        setIsFormDirty(true);
        const renamed = {
            ...key,
            name: "renamed-elsewhere",
        };
        setLatest(renamed);
        await settle();

        expect(held.row()).toBe(key);
        expect(held.isRowHeld()).toBe(true);

        setIsFormDirty(false);
        await settle();
        expect(held.row()).toBe(renamed);
        expect(held.isRowHeld()).toBe(false);

        dispose();
    });

    it("keeps the row's identity across a read that answers the same content", async () => {
        const { held, setLatest, setIsFormDirty, dispose } = mountedKey();

        setLatest(key);
        await settle();
        const heldRow = held.row();
        setLatest({
            ...key,
        });
        await settle();

        expect(held.row()).toBe(heldRow);
        expect(held.isRowHeld()).toBe(false);

        // Dirt alone never flips the row or the hold: an equal read is not a
        // gateway change, whatever the form is doing.
        setIsFormDirty(true);
        await settle();
        expect(held.row()).toBe(heldRow);
        expect(held.isRowHeld()).toBe(false);

        dispose();
    });
});
