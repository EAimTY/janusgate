import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

/**
 * The theme preference cycle: the stored preference resolved on the first
 * read (anything else reading as system), the click cycle light → dark →
 * system → light repainting the document at once, the OS's move followed
 * only while system stands, and a refusing store leaving the paint intact.
 */

/** A Map-backed Storage standing in for the browser store this environment does not provide. */
function memoryStorage(): Storage {
    const entries = new Map<string, string>();

    return {
        get length(): number {
            return entries.size;
        },
        clear: (): void => {
            entries.clear();
        },
        getItem: (key: string): string | null => entries.get(key) ?? null,
        key: (index: number): string | null => Array.from(entries.keys())[index] ?? null,
        removeItem: (key: string): void => {
            entries.delete(key);
        },
        setItem: (key: string, value: string): void => {
            entries.set(key, value);
        },
    };
}

/** The dark-mode media query mock one test file shares, with its listeners. */
function mediaQueryMock() {
    const listeners = new Set<() => void>();
    const query = {
        matches: false,
        addEventListener: (_kind: string, listener: () => void) => {
            listeners.add(listener);
        },
        removeEventListener: (_kind: string, listener: () => void) => {
            listeners.delete(listener);
        },
    };
    return {
        query: query as MediaQueryList,
        flip(isDark: boolean): void {
            query.matches = isDark;
            for (const listener of listeners) {
                listener();
            }
        },
    };
}

type ThemeModule = typeof import("./theme");

/** Solid 2 schedules signal writes; the preference reads settle through a tick. */
async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

beforeEach(() => {
    vi.stubGlobal("localStorage", memoryStorage());
    document.documentElement.className = "";
    document.documentElement.style.colorScheme = "";
});

afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
});

describe("theme preference", () => {
    test("starts following the system when storage holds nothing", async () => {
        const darkQuery = mediaQueryMock();
        window.matchMedia = () => darkQuery.query;
        const theme = await loadTheme();
        theme.initTheme();

        expect(theme.themePreference()).toBe("system");
        expect(document.documentElement.classList.contains("dark")).toBe(false);
        expect(document.documentElement.style.colorScheme).toBe("light");
    });

    test("resolves a stored explicit preference and paints it before the first frame", async () => {
        const darkQuery = mediaQueryMock();
        darkQuery.flip(true);
        window.matchMedia = () => darkQuery.query;
        window.localStorage.setItem("janusgate-web:theme", "light");
        const theme = await loadTheme();
        theme.initTheme();

        // The explicit choice outranks the OS's dark system.
        expect(theme.themePreference()).toBe("light");
        expect(document.documentElement.classList.contains("dark")).toBe(false);
    });

    test("reads a foreign stored value as system", async () => {
        window.matchMedia = () => mediaQueryMock().query;
        window.localStorage.setItem("janusgate-web:theme", "sepia");
        const theme = await loadTheme();
        theme.initTheme();

        expect(theme.themePreference()).toBe("system");
    });

    test("cycles light → dark → system → light, storing and repainting at each click", async () => {
        window.matchMedia = () => mediaQueryMock().query;
        const theme = await loadTheme();
        theme.initTheme();

        // The cycle starts from system: the first click moves to an explicit choice.
        theme.cycleTheme();
        await tick();
        expect(theme.themePreference()).toBe("light");
        expect(window.localStorage.getItem("janusgate-web:theme")).toBe("light");
        expect(document.documentElement.classList.contains("dark")).toBe(false);

        theme.cycleTheme();
        await tick();
        expect(theme.themePreference()).toBe("dark");
        expect(window.localStorage.getItem("janusgate-web:theme")).toBe("dark");
        expect(document.documentElement.classList.contains("dark")).toBe(true);
        expect(document.documentElement.style.colorScheme).toBe("dark");

        theme.cycleTheme();
        await tick();
        expect(theme.themePreference()).toBe("system");
        expect(window.localStorage.getItem("janusgate-web:theme")).toBe("system");

        theme.cycleTheme();
        await tick();
        expect(theme.themePreference()).toBe("light");
    });

    test("follows the OS's move only while system stands", async () => {
        const darkQuery = mediaQueryMock();
        window.matchMedia = () => darkQuery.query;
        const theme = await loadTheme();
        theme.initTheme();

        // The OS turns dark under the system preference: the paint follows.
        darkQuery.flip(true);
        expect(document.documentElement.classList.contains("dark")).toBe(true);

        // An explicit dark choice pins the paint against further OS moves.
        theme.cycleTheme();
        await tick();
        theme.cycleTheme();
        await tick();
        expect(theme.themePreference()).toBe("dark");
        darkQuery.flip(false);
        expect(document.documentElement.classList.contains("dark")).toBe(true);
    });

    test("keeps the paint when storage refuses the write", async () => {
        window.matchMedia = () => mediaQueryMock().query;
        vi.stubGlobal("localStorage", {
            ...memoryStorage(),
            setItem: () => {
                throw new Error("quota exceeded");
            },
        });
        const theme = await loadTheme();
        theme.initTheme();

        expect(() => theme.cycleTheme()).not.toThrow();
        await tick();
        expect(theme.themePreference()).toBe("light");
        expect(document.documentElement.style.colorScheme).toBe("light");
    });
});

/** Re-imports the module fresh, so its module-global signal starts over. */
async function loadTheme(): Promise<ThemeModule> {
    vi.resetModules();
    return import("./theme");
}
