import { createRoot } from "solid-js";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";
import type { ActionName } from "/src/management-api/diagnosis";
import { ManagementApiError } from "/src/management-api/errors";
import { ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import type { ScreenNotice } from "./notices";
import {
    createScreenRead,
    isSkeletonWindowOpen,
    type ReadCall,
    ReadCallFailure,
    readAll,
    readOne,
} from "./read";

/**
 * The screen read harness: a read cycle's calls running in parallel under
 * one signal, the abort ending the cycle ahead of any failure, the first
 * failed call in call order becoming the cycle's diagnosis (a nested read's
 * failure adopted whole), and the harness's two-origin notice policy — a
 * failed cycle replacing the notice, a successful cycle clearing the read's
 * own origin while shielding an action's stated outcome on kept screens.
 */

/** Advances past owned scopes and staged writes. */
async function tick(): Promise<void> {
    for (let round = 0; round < 4; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

const GONE = new ManagementApiError("gateway", "no such row", {
    errno: ErrorCode.NotFound,
    httpStatus: 404,
});

/** One read call answering the given value, recording its signal. */
function ok<TResult>(value: TResult, seen: AbortSignal[] = []): ReadCall<TResult> {
    return {
        run: async (signal) => {
            seen.push(signal);
            return value;
        },
    };
}

/** One read call rejecting with the given cause. */
function failing(cause: unknown, action?: ActionName): ReadCall<never> {
    return {
        action,
        run: async () => {
            throw cause;
        },
    };
}

beforeEach(() => {
    window.history.replaceState(null, "", "/");
});

afterEach(() => {
    window.history.replaceState(null, "", "/");
});

describe("readAll", () => {
    test("answers every result in call order, one signal shared by the calls", async () => {
        const seen: AbortSignal[] = [];

        const [first, second] = await readAll(
            new AbortController().signal,
            ok("a", seen),
            ok(2, seen),
        );

        expect(first).toBe("a");
        expect(second).toBe(2);
        expect(seen).toHaveLength(2);
        expect(seen[0]).toBe(seen[1]);
    });

    test("throws the cycle's AbortError ahead of any failure", async () => {
        const controller = new AbortController();
        const cycle = readAll(
            controller.signal,
            failing(new DOMException("aborted", "AbortError")),
            failing(new Error("boom")),
        );

        await expect(cycle).rejects.toThrowError("This read cycle was aborted.");
        void controller;
    });

    test("makes the first failed call in call order the cycle's ReadCallFailure", async () => {
        const cycle = readAll(
            new AbortController().signal,
            ok(1),
            failing(GONE, "delete-provider"),
        );

        const failure = await cycle.then(
            () => null,
            (error: unknown) => error,
        );

        expect(failure).toBeInstanceOf(ReadCallFailure);
        if (!(failure instanceof ReadCallFailure)) throw new Error("expected a ReadCallFailure");
        expect(failure.notice.detail).toBe("no such row");
    });

    test("adopts a nested read's ReadCallFailure whole instead of re-wrapping it", async () => {
        const inner: ReadCall<unknown> = {
            run: async (signal) => readAll(signal, failing(GONE, "list-usage")),
        };

        const failure = await readAll(new AbortController().signal, inner).then(
            () => null,
            (error: unknown) => error,
        );

        expect(failure).toBeInstanceOf(ReadCallFailure);
        if (!(failure instanceof ReadCallFailure)) throw new Error("expected a ReadCallFailure");
        expect(failure.notice.detail).toBe("no such row");
    });
});

describe("readOne", () => {
    test("runs one call as a cycle of its own", async () => {
        await expect(readOne(new AbortController().signal, ok("solo"))).resolves.toBe("solo");
    });
});

describe("isSkeletonWindowOpen", () => {
    test("opens only while a read is in flight, nothing has ever read, and nothing stands", () => {
        expect(isSkeletonWindowOpen(true, false, null)).toBe(true);
        expect(isSkeletonWindowOpen(false, false, null)).toBe(false);
        expect(isSkeletonWindowOpen(true, true, null)).toBe(false);
        expect(
            isSkeletonWindowOpen(true, false, {
                tone: "bad",
                title: "x",
            }),
        ).toBe(false);
    });
});

describe("createScreenRead", () => {
    function mount(options: Parameters<typeof createScreenRead>[0]): {
        read: ReturnType<typeof createScreenRead>;
        dispose: () => void;
    } {
        let disposeRoot: (() => void) | null = null;
        const read = createRoot((dispose) => {
            disposeRoot = dispose;
            return createScreenRead(options);
        });

        return {
            read,
            dispose: () => disposeRoot?.(),
        };
    }

    test("runs the load once on mount and latches the read", async () => {
        let loads = 0;
        const { read, dispose } = mount({
            load: async () => {
                loads += 1;
            },
        });
        await tick();

        expect(loads).toBe(1);
        expect(read.hasRead()).toBe(true);
        expect(read.isReading()).toBe(false);
        expect(read.notice()).toBeNull();
        dispose();
    });

    test("states a failed cycle's diagnosis as the read-origin notice", async () => {
        const { read, dispose } = mount({
            load: async (signal) => {
                await readAll(signal, failing(GONE, "delete-provider"));
            },
        });
        await tick();

        expect(read.hasRead()).toBe(false);
        const notice = read.notice();
        expect(notice).not.toBeNull();
        expect(notice?.tone).toBe("bad");
        dispose();
    });

    test("a successful cycle clears a read failure but shields an action's notice on kept screens", async () => {
        let isFirstLoad = true;
        const { read, dispose } = mount({
            load: async (signal) => {
                if (isFirstLoad) {
                    isFirstLoad = false;
                    await readAll(signal, failing(new Error("network down"), "list-usage"));
                }
            },
            isNoticeKeptOnSuccess: true,
        });
        await tick();
        expect(read.notice()).not.toBeNull();

        // An action's outcome stated between the cycles survives the reload.
        const outcome: ScreenNotice = {
            tone: "ok",
            title: "saved",
        };
        read.showNotice(outcome);
        read.reload();
        await tick();

        expect(read.hasRead()).toBe(true);
        expect(read.notice()).toEqual(outcome);
        dispose();
    });

    test("a successful cycle clears the read failure on screens that keep no notice", async () => {
        let isFirstLoad = true;
        const { read, dispose } = mount({
            load: async (signal) => {
                if (isFirstLoad) {
                    isFirstLoad = false;
                    await readAll(signal, failing(new Error("network down")));
                }
            },
        });
        await tick();
        expect(read.notice()).not.toBeNull();

        read.reload();
        await tick();

        expect(read.notice()).toBeNull();
        expect(read.hasRead()).toBe(true);
        dispose();
    });

    test("an explicit dismissal lifts whatever stands", async () => {
        const { read, dispose } = mount({
            load: async (signal) => {
                await readAll(signal, failing(new Error("down")));
            },
        });
        await tick();
        expect(read.notice()).not.toBeNull();

        read.showNotice(null);
        await tick();

        expect(read.notice()).toBeNull();
        dispose();
    });

    test("reports an action's outcome: ok passes the caller its success half", async () => {
        const { read, dispose } = mount({
            load: async () => {},
        });
        await tick();

        const isOk = read.reportAction(
            {
                status: "ok",
                result: 7,
            },
            {
                action: "update-provider",
            },
        );

        expect(isOk).toBe(true);
        // A successful action states nothing by itself.
        expect(read.notice()).toBeNull();
        dispose();
    });

    test("reports a failed action onto the notice and runs onGone for a gone diagnosis", async () => {
        const onGone = vi.fn();
        const { read, dispose } = mount({
            load: async () => {},
        });
        await tick();

        const isOk = read.reportAction(
            {
                status: "failed",
                cause: GONE,
            },
            {
                action: "delete-provider",
            },
            {
                onGone,
            },
        );
        await tick();

        expect(isOk).toBe(false);
        expect(onGone).toHaveBeenCalled();
        expect(read.notice()?.tone).toBe("bad");
        dispose();
    });

    test("reports an aborted action as neither success nor failure", async () => {
        const { read, dispose } = mount({
            load: async () => {},
        });
        await tick();

        const isOk = read.reportAction(
            {
                status: "aborted",
            },
            {
                action: "update-provider",
            },
        );

        expect(isOk).toBe(false);
        expect(read.notice()).toBeNull();
        dispose();
    });

    test("re-runs the load when the refresh pulse moves, superseding the cycle in flight", async () => {
        let loads = 0;
        const { read, dispose } = mount({
            load: async () => {
                loads += 1;
            },
        });
        await tick();
        expect(loads).toBe(1);

        requestRefresh();
        await tick();

        expect(loads).toBe(2);
        expect(read.isDisposed()).toBe(false);
        dispose();
    });

    test("a disposed screen's late failure installs nothing", async () => {
        let _loads = 0;
        const { read, dispose } = mount({
            load: async () => {
                _loads += 1;
                await new Promise<void>(() => {
                    // Never settles: the cycle is still in flight when the screen goes away.
                });
            },
        });
        await tick();
        dispose();

        await tick();
        expect(read.isDisposed()).toBe(true);
        expect(read.notice()).toBeNull();
    });
});
