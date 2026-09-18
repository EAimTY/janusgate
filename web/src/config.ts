/**
 * Application-wide tunables. This module is the single home for every
 * non-secret configurable value; connection secrets themselves live only in
 * `localStorage` under the key below and are never baked into the build.
 */
export const appConfig = {
    connectionStorageKey: "janusgate-web:connection",
    themeStorageKey: "janusgate-web:theme",
} as const;
