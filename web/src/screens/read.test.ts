import { createRoot, createSignal } from "solid-js";
import { afterEach, describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";
import { requestRefresh } from "/src/refresh-state";
import {
    createEditRow,
    createHeldRow,
    createPinnedRow,
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

    it("carries a nested read's leaf diagnosis through a composite call instead of re-wrapping it", async () => {
        const controller = new AbortController();

        // The composite shape `readUsageEntities` builds: an outer read call
        // whose own run() runs an inner readAll over the entity listings.
        const composite: ReadCall<unknown> = {
            run: (signal) =>
                readAll(
                    signal,
                    {
                        run: () =>
                            Promise.resolve({
                                providers: [],
                            }),
                    },
                    {
                        run: () =>
                            Promise.reject(
                                gatewayFailure(ErrorCode.NotFound, "the keys listing is gone"),
                            ),
                    },
                ),
        };

        const failure = await readAll(controller.signal, composite).catch(
            (error: unknown) => error,
        );

        expect(failure).toBeInstanceOf(ReadCallFailure);
        expect((failure as ReadCallFailure).notice.title).toBe("The gateway holds no such row.");
        expect((failure as ReadCallFailure).notice.detail).toBe("the keys listing is gone");
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

    it("installs nothing from a superseded cycle's late success", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);

        requestRefresh();
        await waitForCycles(state, 2);
        expect(state.cycles[0]?.signal.aborted).toBe(true);

        // Cycle 1's load resolves only after its supersession: the dead
        // cycle must not drop `isReading` under the live one or install
        // anything it read.
        state.cycles[0]?.succeed();
        await settle();
        expect(read.hasRead()).toBe(false);
        expect(read.isReading()).toBe(true);

        state.cycles[1]?.succeed();
        await settle();
        expect(read.hasRead()).toBe(true);
        expect(read.isReading()).toBe(false);
        expect(read.notice()).toBeNull();
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

    it("holds the skeleton window open exactly while the first read is in flight", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);
        expect(read.isFirstRead()).toBe(true);

        state.cycles[0]?.succeed();
        await settle();
        expect(read.hasRead()).toBe(true);
        expect(read.isFirstRead()).toBe(false);
    });

    it("closes the skeleton window behind a failed first read, also while the re-read runs", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
        });
        await waitForCycles(state, 1);
        state.cycles[0]?.fail();
        await settle();
        expect(read.notice()).not.toBeNull();
        expect(read.isFirstRead()).toBe(false);

        requestRefresh();
        await waitForCycles(state, 2);

        // The re-read is in flight and no read has succeeded yet, but the
        // standing failure keeps the window closed: the skeleton never
        // replaces a stated failure.
        expect(read.isReading()).toBe(true);
        expect(read.isFirstRead()).toBe(false);

        state.cycles[1]?.succeed();
        await settle();
        expect(read.isFirstRead()).toBe(false);
        expect(read.notice()).toBeNull();
    });

    it("keeps a kept-notice screen's first-read failure visible while a re-read runs", async () => {
        const state = manualLoad();
        const { read } = mountedRead({
            load: state.load,
            isNoticeKeptOnSuccess: true,
        });
        await waitForCycles(state, 1);
        state.cycles[0]?.fail();
        await settle();

        requestRefresh();
        await waitForCycles(state, 2);

        // On an isNoticeKeptOnSuccess screen the first-read failure is also
        // an action's reported outcome: it must stay stated while the
        // re-read runs, never swapped for the skeleton.
        expect(read.isReading()).toBe(true);
        expect(read.isFirstRead()).toBe(false);
        expect(read.notice()).not.toBeNull();
    });

    it("renders a nested read's leaf diagnosis as the standing notice", async () => {
        const { read } = mountedRead({
            load: async (signal) => {
                await readAll(signal, {
                    run: (cycle) =>
                        readAll(
                            cycle,
                            {
                                run: () =>
                                    Promise.resolve({
                                        providers: [],
                                    }),
                            },
                            {
                                run: () =>
                                    Promise.reject(
                                        gatewayFailure(
                                            ErrorCode.NotFound,
                                            "the keys listing is gone",
                                        ),
                                    ),
                            },
                        ),
                });
            },
        });
        await settle();

        expect(read.notice()).toEqual({
            tone: "bad",
            title: "The gateway holds no such row.",
            detail: "the keys listing is gone",
            fix: null,
        });
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
        type: "api_key",
        provider_definition_name: "generic-openai-responses",
        provider_definition_version: 1,
        format: "openai_response",
        id: 3,
        name: "alpha",
        model_prefix: "alpha-",
        is_enabled: true,
        base_url: "https://alpha.example",
    };

    const oauthProvider: ProviderResponse = {
        type: "oauth",
        provider_definition_name: "codex",
        provider_definition_version: 1,
        format: "openai_response",
        base_url: null,
        id: 4,
        name: "codex",
        model_prefix: "codex-",
        is_enabled: true,
        expires_at: 1_700_000_000_000_000,
    };

    const modelRow: ModelResponse = {
        id: 11,
        provider_id: 3,
        provider: "alpha",
        upstream_id: "gpt-x",
        public_id: "alpha-gpt-x",
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
                type: "api_key",
                provider_definition_name: "other",
                provider_definition_version: 1,
                format: "openai_chat_completion",
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
                model_prefix: "beta-",
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
                public_id: "beta-gpt-x",
            }),
        ).toBe(false);
        expect(
            isSameModelRow(modelRow, {
                ...modelRow,
                id_override: "beta-gpt-x",
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

    it("answers null while the form is dirty when the read says the row is gone", async () => {
        const { held, setLatest, setIsFormDirty, dispose } = mountedKey();

        setLatest(key);
        await settle();
        setIsFormDirty(true);
        await settle();

        // Deletion ends the form whatever the dirt: a gone read passes null
        // through, and nothing is held against it.
        setLatest(null);
        await settle();

        expect(held.row()).toBeNull();
        expect(held.isRowHeld()).toBe(false);

        dispose();
    });
});

describe("reportAction", () => {
    afterEach(() => {
        for (const dispose of mountedDisposes.reverse()) {
            dispose();
        }
        mountedDisposes.length = 0;
    });

    it("diagnoses a failed action into the notice and runs onGone only when the row is gone", async () => {
        const { read } = mountedRead({
            load: async () => {},
        });
        await settle();

        let goneReloads = 0;
        const goneOutcome = await performManagementCall(() =>
            Promise.reject(gatewayFailure(ErrorCode.NotFound, "the row is gone")),
        );
        const reported = read.reportAction(
            goneOutcome,
            {
                action: "update-provider",
            },
            {
                onGone: () => {
                    goneReloads += 1;
                },
            },
        );
        await settle();

        expect(reported).toBe(false);
        expect(goneReloads).toBe(1);
        expect(read.notice()).toEqual({
            tone: "bad",
            title: "The gateway holds no such row.",
            detail: "the row is gone",
            fix: null,
        });

        // A failure that is not a gone row states its diagnosis but reloads
        // nothing: the screen's own state stands.
        const conflictOutcome = await performManagementCall(() =>
            Promise.reject(gatewayFailure(ErrorCode.Conflict, "name taken")),
        );
        read.reportAction(
            conflictOutcome,
            {
                action: "update-provider",
            },
            {
                onGone: () => {
                    goneReloads += 1;
                },
            },
        );
        await settle();

        expect(goneReloads).toBe(1);
        expect(read.notice()?.title).toContain("an identifier is already taken");
    });

    it("answers true for a succeeded call and states nothing for an aborted one", async () => {
        const { read } = mountedRead({
            load: async () => {},
        });
        await settle();

        const succeeded = read.reportAction(
            await performManagementCall(() =>
                Promise.resolve({
                    model: undefined,
                }),
            ),
            {
                action: "update-model",
            },
        );
        await settle();

        expect(succeeded).toBe(true);
        expect(read.notice()).toBeNull();

        const reported = read.reportAction(await performManagementCall(() => abortFailure()), {
            action: "update-model",
        });
        await settle();

        expect(reported).toBe(false);
        expect(read.notice()).toBeNull();
    });
});

describe("createEditRow", () => {
    function mountedEditRow() {
        const [latest, setLatest] = createSignal<ApiKeyResponse | null>(null);
        const box: {
            edit?: ReturnType<typeof createEditRow<ApiKeyResponse>>;
        } = {};
        const dispose = createRoot((teardown) => {
            box.edit = createEditRow(latest, isSameApiKeyRow);
            return teardown;
        });

        if (box.edit === undefined) {
            throw new Error("The edit row did not mount");
        }

        return {
            edit: box.edit,
            setLatest,
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

    it("holds the row from a succeeded save until its reload lands, whatever the listing answers", async () => {
        const { edit, setLatest, dispose } = mountedEditRow();

        setLatest(key);
        await settle();
        expect(edit.row()).toBe(key);

        // The save succeeded; the listing still answers the pre-save rows,
        // and the held row must not adopt them even though the form is clean.
        edit.reportSaved();
        const savedRow: ApiKeyResponse = {
            ...key,
            name: "renamed-elsewhere",
        };
        setLatest(savedRow);
        await settle();

        expect(edit.row()).toBe(key);
        expect(edit.isRowHeld()).toBe(true);

        // The save's reload lands: its reads clear the latch and the fresh
        // listing is adopted.
        edit.reportReloaded();
        await settle();

        expect(edit.row()).toBe(savedRow);
        expect(edit.isRowHeld()).toBe(false);

        dispose();
    });
});

describe("createPinnedRow", () => {
    function mountedProvider() {
        const [latest, setLatest] = createSignal<ProviderResponse | null>(null);
        const box: {
            pinned?: ReturnType<typeof createPinnedRow<ProviderResponse>>;
        } = {};
        const dispose = createRoot((teardown) => {
            box.pinned = createPinnedRow(latest, isSameProviderRow);
            return teardown;
        });

        if (box.pinned === undefined) {
            throw new Error("The pinned row did not mount");
        }

        return {
            pinned: box.pinned,
            setLatest,
            dispose,
        };
    }

    const provider: ProviderResponse = {
        type: "api_key",
        provider_definition_name: "generic-openai-responses",
        provider_definition_version: 1,
        format: "openai_response",
        id: 3,
        name: "alpha",
        model_prefix: "alpha-",
        is_enabled: true,
        base_url: "https://alpha.example",
    };

    afterEach(() => {
        for (const dispose of mountedDisposes.reverse()) {
            dispose();
        }
        mountedDisposes.length = 0;
    });

    it("keeps a re-answered row's identity, adopts a changed one, and passes null through", async () => {
        const { pinned, setLatest, dispose } = mountedProvider();

        setLatest(provider);
        await settle();
        const pinnedRow = pinned();
        expect(pinnedRow).toBe(provider);

        // A re-parsed answer with the same content keeps the object.
        setLatest({
            ...provider,
        });
        await settle();
        expect(pinned()).toBe(pinnedRow);

        const renamed: ProviderResponse = {
            ...provider,
            name: "beta",
        };
        setLatest(renamed);
        await settle();
        expect(pinned()).toBe(renamed);

        setLatest(null);
        await settle();
        expect(pinned()).toBeNull();

        dispose();
    });
});
