import { beforeEach, describe, expect, it } from "vitest";
import { drainWrites } from "./test-support/management-api";
import { cycleTheme, nextThemePreference, themePreference } from "./theme";

// jsdom implements neither matchMedia nor storage. The module reads storage
// through its own absence-guard; the media query is a browser guarantee the
// DOM here does not carry, so the stub stands in for an OS that prefers
// light — the system preference resolves light, and the dark paint only
// appears under the explicit dark choice.
window.matchMedia = (media: string): MediaQueryList => ({
    matches: false,
    media,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
});

// Solid settles a signal write on a microtask, so a click's new preference
// becomes observable only after a drain of the queue. The repaint does not
// wait: it applies the clicked value directly.

describe("nextThemePreference", () => {
    it("cycles light → dark → system and wraps to light", () => {
        expect(nextThemePreference("light")).toBe("dark");
        expect(nextThemePreference("dark")).toBe("system");
        expect(nextThemePreference("system")).toBe("light");
    });
});

describe("cycleTheme", () => {
    // The preference signal is module state shared by this suite's tests,
    // and it reads storage once at import — clearing the key here cannot
    // reset it. Walking the cycle back to system starts every test where a
    // fresh browser starts: no explicit choice in force.
    beforeEach(async () => {
        while (themePreference() !== "system") {
            cycleTheme();
            await drainWrites();
        }
    });

    it("advances one state per click and repaints the resolved theme", async () => {
        expect(themePreference()).toBe("system");

        cycleTheme();
        expect(document.documentElement.classList.contains("dark")).toBe(false);
        await drainWrites();
        expect(themePreference()).toBe("light");

        cycleTheme();
        expect(document.documentElement.classList.contains("dark")).toBe(true);
        await drainWrites();
        expect(themePreference()).toBe("dark");

        cycleTheme();
        await drainWrites();
        expect(themePreference()).toBe("system");
        // The system preference resolves against the stub's light OS, so the
        // dark paint is gone.
        expect(document.documentElement.classList.contains("dark")).toBe(false);
    });

    it("departs system and returns to it, keeping the third state reachable", async () => {
        // The reason the cycle exists: an operator who toggles once away
        // from following the OS must be able to follow it again.
        cycleTheme();
        await drainWrites();
        expect(themePreference()).toBe("light");

        cycleTheme();
        await drainWrites();
        expect(themePreference()).toBe("dark");

        cycleTheme();
        await drainWrites();
        expect(themePreference()).toBe("system");
    });
});
