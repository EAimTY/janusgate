import { flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import english from "./catalogs/en";
import { createLocalizationController } from "./controller";

function held<T>(): {
    promise: Promise<T>;
    resolve(value: T): void;
    reject(reason: unknown): void;
} {
    let resolve!: (value: T) => void;
    let reject!: (reason: unknown) => void;
    const promise = new Promise<T>((yes, no) => {
        resolve = yes;
        reject = no;
    });
    return {
        promise,
        resolve,
        reject,
    };
}
const disposals: Array<() => void> = [];
afterEach(() => {
    for (const dispose of disposals.splice(0)) dispose();
    vi.useRealTimers();
});
function setup(loader: () => Promise<typeof english>, stored: string | null = null) {
    const writes: string[] = [];
    let languages = [
        "en",
    ];
    const controller = createLocalizationController({
        registry: {
            en: {
                autonym: "English",
                load: async () => english,
            },
            "zh-CN": {
                autonym: "简体中文",
                load: loader,
            },
        },
        defaults: {
            en: "en",
            zh: "zh-CN",
        },
        readPreference: () => stored,
        writePreference: (value) => {
            writes.push(value);
        },
        browserLanguages: () => languages,
        eventTarget: window,
        root: document.documentElement,
        deadlineMs: 15_000,
    });
    disposals.push(controller.dispose);
    return {
        controller,
        writes,
        browser: (next: string[]) => {
            languages = next;
            window.dispatchEvent(new Event("languagechange"));
            flush();
        },
    };
}
describe("localization activation ownership", () => {
    it("deduplicates a pending locale and only commits the newest request", async () => {
        const load = held<typeof english>();
        const loader = vi.fn(() => load.promise);
        const { controller, writes } = setup(loader);
        await controller.initialize();
        const first = controller.setPreference("zh-CN");
        const second = controller.setPreference("zh-CN");
        expect(loader).toHaveBeenCalledTimes(1);
        load.resolve(english);
        await Promise.all([
            first,
            second,
        ]);
        expect(controller.activeLocale()).toBe("zh-CN");
        expect(writes).toEqual([
            "system",
            "zh-CN",
        ]);
        expect(document.documentElement.lang).toBe("zh-CN");
    });
    it.each([
        "success",
        "failure",
    ])("an English selection supersedes late Chinese %s", async (outcome) => {
        const load = held<typeof english>();
        const { controller, writes } = setup(() => load.promise);
        await controller.initialize();
        const pending = controller.setPreference("zh-CN");
        await controller.setPreference("en");
        if (outcome === "success") load.resolve(english);
        else load.reject(new Error("offline"));
        await pending;
        expect(controller.activeLocale()).toBe("en");
        expect(controller.changeState()).toEqual({
            type: "idle",
        });
        expect(writes).toEqual([
            "system",
            "en",
        ]);
    });
    it("recovers startup in English with original intent, then retries", async () => {
        const loader = vi
            .fn()
            .mockRejectedValueOnce(new Error("offline"))
            .mockResolvedValue(english);
        const { controller, writes } = setup(loader, "zh-CN");
        await controller.initialize();
        expect(controller.activeLocale()).toBe("en");
        expect(controller.preference()).toBe("zh-CN");
        expect(controller.changeState()).toMatchObject({
            type: "failed",
            preference: "zh-CN",
            isStartupRecovery: true,
        });
        expect(writes).toEqual([]);
        await controller.retry();
        expect(controller.activeLocale()).toBe("zh-CN");
        expect(loader).toHaveBeenCalledTimes(2);
    });
    it("follows system intent while loading, but ignores browser changes for explicit intent", async () => {
        const load = held<typeof english>();
        const { controller, browser } = setup(() => load.promise);
        await controller.initialize();
        browser([
            "zh-SG",
        ]);
        expect(controller.changeState()).toMatchObject({
            type: "loading",
            preference: "system",
        });
        browser([
            "en-US",
        ]);
        await Promise.resolve();
        await Promise.resolve();
        load.resolve(english);
        await Promise.resolve();
        await Promise.resolve();
        expect(controller.activeLocale()).toBe("en");
        await controller.setPreference("zh-CN");
        browser([
            "en",
        ]);
        expect(controller.activeLocale()).toBe("zh-CN");
    });
    it("times out, permits a new load, and prevents late cleanup from evicting that retry", async () => {
        vi.useFakeTimers();
        const old = held<typeof english>();
        const fresh = held<typeof english>();
        const loader = vi
            .fn()
            .mockImplementationOnce(() => old.promise)
            .mockImplementationOnce(() => fresh.promise);
        const { controller } = setup(loader);
        await controller.initialize();
        const pending = controller.setPreference("zh-CN");
        await vi.advanceTimersByTimeAsync(15_000);
        await pending;
        expect(controller.changeState()).toMatchObject({
            type: "failed",
            reason: "timeout",
        });
        const retry = controller.retry();
        old.reject(new Error("late"));
        await Promise.resolve();
        await Promise.resolve();
        const newest = controller.setPreference("zh-CN");
        expect(loader).toHaveBeenCalledTimes(2);
        fresh.resolve(english);
        await Promise.all([
            retry,
            newest,
        ]);
        expect(controller.activeLocale()).toBe("zh-CN");
        expect(vi.getTimerCount()).toBe(0);
    });
    it("disposes listeners and timers and prevents pending activation", async () => {
        vi.useFakeTimers();
        const load = held<typeof english>();
        const { controller, browser } = setup(() => load.promise);
        await controller.initialize();
        const pending = controller.setPreference("zh-CN");
        controller.dispose();
        load.resolve(english);
        await pending;
        browser([
            "zh-CN",
        ]);
        expect(controller.activeLocale()).toBe("en");
        expect(vi.getTimerCount()).toBe(0);
    });
    it("keeps successful in-memory activation when storage refuses reads and writes", async () => {
        const controller = createLocalizationController({
            registry: {
                en: {
                    autonym: "English",
                    load: async () => english,
                },
                "zh-CN": {
                    autonym: "简体中文",
                    load: async () => english,
                },
            },
            defaults: {
                en: "en",
                zh: "zh-CN",
            },
            readPreference: () => {
                throw new Error("refused");
            },
            writePreference: () => {
                throw new Error("refused");
            },
            browserLanguages: () => [
                "en",
            ],
            eventTarget: window,
            root: document.documentElement,
            deadlineMs: 15_000,
        });
        disposals.push(controller.dispose);
        await controller.initialize();
        expect(controller.preference()).toBe("system");
        await controller.setPreference("zh-CN");
        expect(controller.preference()).toBe("zh-CN");
    });
    it("ignores same-turn browser changes while an explicit selection is pending", async () => {
        const load = held<typeof english>();
        const { controller, browser } = setup(() => load.promise);
        await controller.initialize();
        const pending = controller.setPreference("zh-CN");
        browser([
            "en-US",
        ]);
        load.resolve(english);
        await pending;
        expect(controller.preference()).toBe("zh-CN");
        expect(controller.activeLocale()).toBe("zh-CN");
    });
    it("retains the committed preference after a normal load failure and retries the requested intent", async () => {
        const loader = vi
            .fn()
            .mockRejectedValueOnce(new Error("offline"))
            .mockResolvedValue(english);
        const { controller, writes } = setup(loader, "en");
        await controller.initialize();
        await controller.setPreference("zh-CN");
        expect(controller.preference()).toBe("en");
        expect(controller.changeState()).toMatchObject({
            type: "failed",
            preference: "zh-CN",
            isStartupRecovery: false,
        });
        expect(writes).toEqual([
            "en",
        ]);
        await controller.retry();
        expect(controller.preference()).toBe("zh-CN");
        expect(writes).toEqual([
            "en",
            "zh-CN",
        ]);
    });
    it("treats an unregistered saved value as system preference", async () => {
        const { controller } = setup(async () => english, "fr-unknown");
        await controller.initialize();
        expect(controller.preference()).toBe("system");
        expect(controller.activeLocale()).toBe("en");
    });
    it("caches a successful catalog across repeated locale switches", async () => {
        const loader = vi.fn(async () => english);
        const { controller } = setup(loader);
        await controller.initialize();
        await controller.setPreference("zh-CN");
        await controller.setPreference("en");
        await controller.setPreference("zh-CN");
        expect(controller.activeLocale()).toBe("zh-CN");
        expect(loader).toHaveBeenCalledTimes(1);
    });
    it("warms the cache from a late timed-out success without activating it", async () => {
        vi.useFakeTimers();
        const load = held<typeof english>();
        const loader = vi.fn(() => load.promise);
        const { controller, writes } = setup(loader, "en");
        await controller.initialize();
        const pending = controller.setPreference("zh-CN");
        await vi.advanceTimersByTimeAsync(15_000);
        await pending;
        load.resolve(english);
        await Promise.resolve();
        await Promise.resolve();
        expect(controller.activeLocale()).toBe("en");
        expect(controller.changeState().type).toBe("failed");
        expect(writes).toEqual([
            "en",
        ]);
        await controller.retry();
        expect(controller.activeLocale()).toBe("zh-CN");
        expect(loader).toHaveBeenCalledTimes(1);
        expect(vi.getTimerCount()).toBe(0);
    });
});
