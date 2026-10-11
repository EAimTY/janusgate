import { describe, expect, test } from "vitest";
import { createLocalizationController } from "./controller";
import type { Catalog, LocaleEntry } from "./types";

/**
 * The localization controller's activation machine: English bootstrapping
 * synchronously, the stored or system preference resolved at startup, a
 * preference switch loading its catalog once through the deadline (a loader
 * rejection and a deadline reading as distinct failures), the retry
 * re-running the failed activation, the browser's own language move followed
 * only while system stands, and disposal cancelling everything in flight.
 */

/** A Map-backed Storage standing in for the browser store this environment does not provide. */
function memoryStorage(): {
    store: Map<string, string>;
    read: () => string | null;
    write: (value: string) => void;
} {
    const store = new Map<string, string>();
    return {
        store,
        read: () => store.get("janusgate-web:language") ?? null,
        write: (value) => {
            store.set("janusgate-web:language", value);
        },
    };
}

/** A controllable two-locale registry: English resolved, one loader of the test's own. */
function registryWith(loader: () => Promise<Catalog>): Record<"en" | "ja", LocaleEntry> {
    return {
        en: {
            autonym: "English",
            load: async () => ({
                shell: {
                    refresh: "Refresh",
                },
            }),
        },
        ja: {
            autonym: "日本語",
            load: loader,
        },
    };
}

/** The DOM stand-ins the controller reads and writes, emitting real events. */
function dom() {
    const eventTarget = new EventTarget();
    return {
        root: {
            lang: "en",
            dir: "ltr",
        },
        eventTarget,
        emit: (kind: string): void => {
            eventTarget.dispatchEvent(new Event(kind));
        },
    };
}

/** Advances staged writes and answered promises. */
async function tick(): Promise<void> {
    for (let round = 0; round < 4; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

/** Waits out the loader deadline in real time and flushes the promise queues. */
async function advance(ms: number): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, ms));
    await tick();
}

describe("createLocalizationController", () => {
    test("bootstraps English synchronously and resolves the system preference at startup", async () => {
        const storage = memoryStorage();
        const { root, eventTarget } = dom();
        const controller = createLocalizationController({
            registry: registryWith(async () => ({
                shell: {
                    refresh: "更新",
                },
            })),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "ja",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });

        // Before initialization the English bootstrap answers.
        expect(controller.activeLocale()).toBe("en");

        await controller.initialize();

        expect(controller.preference()).toBe("system");
        expect(controller.activeLocale()).toBe("ja");
        expect(root.lang).toBe("ja");
        expect(controller.translation()(($) => $.shell.refresh)).toBe("更新");
        expect(controller.changeState()).toEqual({
            type: "idle",
        });
    });

    test("activates a stored preference and persists a switch best-effort", async () => {
        const storage = memoryStorage();
        storage.store.set("janusgate-web:language", "en");
        const { root, eventTarget } = dom();
        let loadRuns = 0;
        const controller = createLocalizationController({
            registry: registryWith(async () => {
                loadRuns += 1;
                return {
                    shell: {
                        refresh: "更新",
                    },
                };
            }),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: (_value) => {
                throw new Error("refused");
            },
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });
        await controller.initialize();

        await controller.setPreference("ja");

        expect(controller.preference()).toBe("ja");
        expect(controller.activeLocale()).toBe("ja");
        expect(loadRuns).toBe(1);
        // The refused persistence left the in-memory preference in force.
        expect(controller.preference()).toBe("ja");
    });

    test("loads each locale once, answering a repeat switch from the loaded set", async () => {
        const storage = memoryStorage();
        const { root, eventTarget } = dom();
        let loadRuns = 0;
        const controller = createLocalizationController({
            registry: registryWith(async () => {
                loadRuns += 1;
                return {
                    shell: {
                        refresh: "更新",
                    },
                };
            }),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });
        await controller.initialize();

        await controller.setPreference("ja");
        await controller.setPreference("en");
        await controller.setPreference("ja");

        expect(loadRuns).toBe(1);
        expect(controller.activeLocale()).toBe("ja");
    });

    test("states a loader rejection as a load failure and re-runs it on retry", async () => {
        const storage = memoryStorage();
        const { root, eventTarget } = dom();
        let attempts = 0;
        const controller = createLocalizationController({
            registry: registryWith(async () => {
                attempts += 1;
                if (attempts === 1) {
                    throw new Error("network gone");
                }
                return {
                    shell: {
                        refresh: "更新",
                    },
                };
            }),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });
        await controller.initialize();

        await controller.setPreference("ja");

        expect(controller.changeState()).toEqual({
            type: "failed",
            preference: "ja",
            locale: "ja",
            reason: "load",
            isStartupRecovery: false,
        });
        // The activation never landed: English keeps answering.
        expect(controller.activeLocale()).toBe("en");

        await controller.retry();

        expect(controller.activeLocale()).toBe("ja");
        expect(controller.changeState()).toEqual({
            type: "idle",
        });
    });

    test("states a stalled catalog as a timeout failure, the deadline never read as an answer", async () => {
        const storage = memoryStorage();
        const { root, eventTarget } = dom();
        const controller = createLocalizationController({
            registry: registryWith(
                () =>
                    new Promise<Catalog>(() => {
                        // Never settles.
                    }),
            ),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 20,
        });
        await controller.initialize();

        const switched = controller.setPreference("ja");
        await advance(100);

        await switched;
        expect(controller.changeState()).toMatchObject({
            type: "failed",
            locale: "ja",
            reason: "timeout",
        });
    });

    test("retries a startup failure against the stored preference with the English recovery standing", async () => {
        const storage = memoryStorage();
        storage.store.set("janusgate-web:language", "ja");
        const { root, eventTarget } = dom();
        let attempts = 0;
        const controller = createLocalizationController({
            registry: registryWith(async () => {
                attempts += 1;
                if (attempts === 1) {
                    throw new Error("offline");
                }
                return {
                    shell: {
                        refresh: "更新",
                    },
                };
            }),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });

        await controller.initialize();

        // The startup recovery keeps English answering and states the failed startup.
        expect(controller.activeLocale()).toBe("en");
        expect(root.lang).toBe("en");
        expect(controller.changeState()).toMatchObject({
            type: "failed",
            reason: "load",
            isStartupRecovery: true,
        });

        await controller.retry();

        expect(controller.activeLocale()).toBe("ja");
        expect(controller.preference()).toBe("ja");
    });

    test("follows the browser's language move only while system stands", async () => {
        const storage = memoryStorage();
        const { root, eventTarget, emit } = dom();
        const controller = createLocalizationController({
            registry: registryWith(async () => ({
                shell: {
                    refresh: "更新",
                },
            })),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "ja",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });
        await controller.initialize();
        expect(controller.activeLocale()).toBe("ja");

        await controller.setPreference("en");

        // The browser moves to Japanese: the explicit preference stands.
        emit("languagechange");

        expect(controller.preference()).toBe("en");
        expect(controller.activeLocale()).toBe("en");
    });

    test("a superseded switch never lands: the newest request owns the machine", async () => {
        const storage = memoryStorage();
        const { root, eventTarget } = dom();
        const settles: Array<(catalog: Catalog) => void> = [];
        const controller = createLocalizationController({
            registry: registryWith(
                () =>
                    new Promise<Catalog>((resolve) => {
                        settles.push(resolve);
                    }),
            ),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 60_000,
        });
        await controller.initialize();

        const first = controller.setPreference("ja");
        const second = controller.setPreference("ja");
        settles[1]?.({
            shell: {
                refresh: "更新",
            },
        });
        settles[0]?.({
            shell: {
                refresh: "更新",
            },
        });
        await Promise.all([
            first,
            second,
        ]);
        await tick();

        expect(controller.activeLocale()).toBe("ja");
    });

    test("dispose cancels a pending load and detaches the browser listener", async () => {
        const storage = memoryStorage();
        const { root, eventTarget, emit } = dom();
        const controller = createLocalizationController({
            registry: registryWith(async () => ({
                shell: {
                    refresh: "更新",
                },
            })),
            defaults: {
                en: "en",
                ja: "ja",
            },
            readPreference: storage.read,
            writePreference: storage.write,
            browserLanguages: () => [
                "en",
            ],
            eventTarget,
            root,
            deadlineMs: 5_000,
        });
        await controller.initialize();

        controller.dispose();

        // A switch after disposal activates nothing and answers nothing new.
        await controller.setPreference("ja");
        expect(controller.activeLocale()).toBe("en");
        expect(() => emit("languagechange")).not.toThrow();
    });
});
