import { createSignal } from "solid-js";
import { appConfig } from "/src/config";

/** What the operator chose: an explicit theme, or the operating system's. */
export type ThemePreference = "light" | "dark" | "system";

/** What the app paints: the preference resolved against the media query. */
type ResolvedTheme = "light" | "dark";

function readPreference(): ThemePreference {
    try {
        const stored = window.localStorage.getItem(appConfig.themeStorageKey);
        return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
    } catch {
        return "system";
    }
}

function resolveTheme(preference: ThemePreference): ResolvedTheme {
    if (preference !== "system") {
        return preference;
    }

    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

function applyTheme(resolved: ResolvedTheme): void {
    document.documentElement.classList.toggle("dark", resolved === "dark");
    document.documentElement.style.colorScheme = resolved;
}

const [preference, setPreferenceSignal] = createSignal<ThemePreference>(readPreference());

function refresh(explicit: ThemePreference): void {
    applyTheme(resolveTheme(explicit));
}

/** Paints the stored preference before the first frame, then follows the OS. */
export function initTheme(): void {
    refresh(preference());

    try {
        window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => {
            if (preference() === "system") {
                refresh("system");
            }
        });
    } catch {
        // Older browsers without media-query events keep the first paint.
    }
}

/** The preference signal, for chrome that shows which state is in force. */
export function themePreference(): ThemePreference {
    return preference();
}

/**
 * The preference one click ahead of the top-bar control's cycle: light →
 * dark → system → light. The system state sits last so the first click from
 * a fresh visit — which starts there — moves to an explicit choice, and the
 * third click returns to following the OS.
 */
export function nextThemePreference(current: ThemePreference): ThemePreference {
    switch (current) {
        case "light":
            return "dark";
        case "dark":
            return "system";
        case "system":
            return "light";
    }
}

/** Stores a new preference and repaints immediately. */
function setThemePreference(next: ThemePreference): void {
    setPreferenceSignal(next);

    try {
        window.localStorage.setItem(appConfig.themeStorageKey, next);
    } catch {
        // Private browsing may refuse storage; the paint still applies.
    }

    refresh(next);
}

/** Advances the preference one step through light → dark → system and repaints. */
export function cycleTheme(): void {
    setThemePreference(nextThemePreference(preference()));
}
