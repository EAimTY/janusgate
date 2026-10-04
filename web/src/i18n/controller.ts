import { createInstance, type TFunction } from "i18next";
import { createSignal, flush, latest } from "solid-js";
import english from "./catalogs/en";
import { resolveBrowserLocale } from "./registry";
import type { Catalog, LocaleEntry } from "./types";

export type LanguageChangeState<T extends string = string> =
    | {
          readonly type: "idle";
      }
    | {
          readonly type: "loading";
          readonly preference: "system" | T;
          readonly locale: T;
      }
    | {
          readonly type: "failed";
          readonly preference: "system" | T;
          readonly locale: T;
          readonly reason: "load" | "timeout";
          readonly isStartupRecovery: boolean;
      };
type Activation<T extends string> = {
    readonly preference: "system" | T;
    readonly locale: T;
    readonly translator: TFunction<"translation">;
};
type Snapshot<T extends string> = {
    readonly activation: Activation<T>;
    readonly request: LanguageChangeState<T>;
};
type LoadResult =
    | {
          readonly type: "loaded";
      }
    | {
          readonly type: "failed";
          readonly reason: "load" | "timeout";
      }
    | {
          readonly type: "disposed";
      };

/** Owns one activation signal and request identity; independent of routes and gateway state. */
export function createLocalizationController<T extends string>(options: {
    readonly registry: Readonly<Record<T, LocaleEntry>>;
    readonly defaults: Readonly<Record<string, T>>;
    readonly readPreference: () => string | null;
    readonly writePreference: (preference: "system" | T) => void;
    readonly browserLanguages: () => readonly string[];
    readonly eventTarget: EventTarget;
    readonly root: Pick<HTMLElement, "lang" | "dir">;
    readonly deadlineMs: number;
}): {
    initialize(): Promise<void>;
    preference(): "system" | T;
    activeLocale(): T;
    translation(): TFunction<"translation">;
    changeState(): LanguageChangeState<T>;
    setPreference(next: "system" | T): Promise<void>;
    retry(): Promise<void>;
    dispose(): void;
} {
    // Object.keys loses key types; the registry's closed generic owns every key.
    const tags = Object.keys(options.registry) as T[];
    const foundEnglish = tags.find((tag) => tag === "en");
    if (foundEnglish === undefined) throw new Error("Locale registry requires English");
    const englishTag: T = foundEnglish;
    const instance = createInstance();
    const initialization = instance.init({
        lng: englishTag,
        ns: [
            "translation",
        ],
        defaultNS: "translation",
        supportedLngs: tags,
        load: "currentOnly",
        fallbackLng: false,
        fallbackNS: false,
        returnNull: false,
        returnObjects: false,
        saveMissing: false,
        interpolation: {
            escapeValue: false,
        },
        resources: {
            [englishTag]: {
                translation: english,
            },
        },
        initAsync: false,
    });
    const [snapshot, setSnapshot] = createSignal<Snapshot<T>>({
        activation: {
            preference: "system",
            locale: englishTag,
            translator: instance.getFixedT(englishTag, "translation"),
        },
        request: {
            type: "idle",
        },
    });
    const loaded = new Set<T>([
        englishTag,
    ]);
    const pending = new Map<T, Promise<LoadResult>>();
    const cancellations = new Set<() => void>();
    let requestId = 0;
    let isDisposed = false;
    let isInitialized = false;
    function load(locale: T): Promise<LoadResult> {
        if (loaded.has(locale))
            return Promise.resolve({
                type: "loaded",
            });
        const existing = pending.get(locale);
        if (existing !== undefined) return existing;
        const attempt = new Promise<LoadResult>((resolve) => {
            let isSettled = false;
            const settle = (result: LoadResult): void => {
                if (isSettled) return;
                isSettled = true;
                clearTimeout(timer);
                cancellations.delete(cancel);
                resolve(result);
            };
            const cancel = (): void =>
                settle({
                    type: "disposed",
                });
            const timer = setTimeout(
                () =>
                    settle({
                        type: "failed",
                        reason: "timeout",
                    }),
                options.deadlineMs,
            );
            cancellations.add(cancel);
            // Only loader rejection is recoverable. Resource registration errors
            // remain programmer errors, outside this failure handler.
            void options.registry[locale].load().then(
                (catalog: Catalog) => {
                    if (!isDisposed && !loaded.has(locale)) {
                        instance.addResourceBundle(locale, "translation", catalog);
                        loaded.add(locale);
                    }
                    settle({
                        type: "loaded",
                    });
                },
                () =>
                    settle({
                        type: "failed",
                        reason: "load",
                    }),
            );
        });
        pending.set(locale, attempt);
        void attempt.then(() => {
            if (pending.get(locale) === attempt) pending.delete(locale);
        });
        return attempt;
    }
    async function activate(preference: "system" | T, isStartup = false): Promise<void> {
        if (isDisposed) return;
        const id = ++requestId;
        const locale =
            preference === "system"
                ? resolveBrowserLocale(options.browserLanguages(), tags, options.defaults)
                : preference;
        setSnapshot({
            ...latest(snapshot),
            request: {
                type: "loading",
                preference,
                locale,
            },
        });
        // Publish intent before another synchronous browser event can inspect it.
        // Solid 2 schedules imperative signal writes; latest reads settled nodes.
        flush();
        const result = await load(locale);
        if (isDisposed || id !== requestId || result.type === "disposed") return;
        if (result.type === "failed") {
            if (isStartup) {
                options.root.lang = englishTag;
                options.root.dir = instance.dir(englishTag);
            }
            setSnapshot((previous) => ({
                activation: isStartup
                    ? {
                          ...previous.activation,
                          preference,
                      }
                    : previous.activation,
                request: {
                    type: "failed",
                    preference,
                    locale,
                    reason: result.reason,
                    isStartupRecovery: isStartup,
                },
            }));
            return;
        }
        const translator = instance.getFixedT(locale, "translation");
        // Solid 2 batches signal writes automatically. A single snapshot write
        // commits translator, preference, locale and idle state atomically.
        options.root.lang = locale;
        options.root.dir = instance.dir(locale);
        setSnapshot({
            activation: {
                preference,
                locale,
                translator,
            },
            request: {
                type: "idle",
            },
        });
        try {
            options.writePreference(preference);
        } catch {
            /* Persistence is best effort. */
        }
    }
    const onBrowserChange = (): void => {
        const current = latest(snapshot);
        const intent =
            current.request.type === "loading"
                ? current.request.preference
                : current.activation.preference;
        if (intent === "system") void activate("system");
    };
    return {
        initialize: async () => {
            if (isInitialized || isDisposed) return;
            isInitialized = true;
            await initialization;
            let stored: string | null = null;
            try {
                stored = options.readPreference();
            } catch {
                /* Refused storage means system. */
            }
            const preference = tags.find((tag) => tag === stored) ?? "system";
            options.eventTarget.addEventListener("languagechange", onBrowserChange);
            await activate(preference, true);
        },
        preference: () => snapshot().activation.preference,
        activeLocale: () => snapshot().activation.locale,
        translation: () => snapshot().activation.translator,
        changeState: () => snapshot().request,
        setPreference: (next) => activate(next),
        retry: () => {
            const state = snapshot().request;
            return state.type === "failed"
                ? activate(state.preference, state.isStartupRecovery)
                : Promise.resolve();
        },
        dispose: () => {
            isDisposed = true;
            requestId += 1;
            options.eventTarget.removeEventListener("languagechange", onBrowserChange);
            for (const cancel of cancellations) cancel();
            pending.clear();
        },
    };
}
