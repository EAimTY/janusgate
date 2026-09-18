import { createSignal } from "solid-js";
import { appConfig } from "/src/config";

/** What the operator chose: an explicit theme, or the operating system's. */
export type ThemePreference = "light" | "dark" | "system";

/** What the app paints: the preference resolved against the media query. */
export type ResolvedTheme = "light" | "dark";

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
const [resolved, setResolved] = createSignal<ResolvedTheme>(resolveTheme(preference()));

function refresh(explicit: ThemePreference): void {
    const next = resolveTheme(explicit);

    setResolved(next);
    applyTheme(next);
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

/** The operator's preference signal, for the toggle control. */
export function themePreference(): ThemePreference {
    return preference();
}

/** The resolved theme signal, for theme-aware chrome. */
export function resolvedTheme(): ResolvedTheme {
    return resolved();
}

/** Stores a new preference and repaints immediately. */
export function setThemePreference(next: ThemePreference): void {
    setPreferenceSignal(next);

    try {
        window.localStorage.setItem(appConfig.themeStorageKey, next);
    } catch {
        // Private browsing may refuse storage; the paint still applies.
    }

    refresh(next);
}

/** Flips between the two painted themes, keeping an explicit choice. */
export function toggleTheme(): void {
    setThemePreference(resolved() === "dark" ? "light" : "dark");
}
