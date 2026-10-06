/**
 * Application-wide tunables. This module is the single home for every
 * non-secret configurable value; connection secrets themselves live only in
 * `localStorage` under the key below and are never baked into the build.
 */
export const appConfig = {
    languageStorageKey: "janusgate-web:language",
    languageLoadDeadlineMs: 15_000,
    connectionStorageKey: "janusgate-web:connection",
    /** Milliseconds a connection probe may run before it counts as failed; the setup diagnosis's reachability refinement shares the budget. */
    connectionProbeTimeoutMs: 15_000,
    /** Read as a literal by the inline theme bootstrap in `index.html` — keep the two in sync. */
    themeStorageKey: "janusgate-web:theme",
    /** Milliseconds between two device-flow completion polls. */
    oauthDevicePollIntervalMs: 3000,
} as const;
