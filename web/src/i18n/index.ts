import { appConfig } from "/src/config";
import { createLocalizationController } from "./controller";
import { languageDefaults, localeRegistry, type SupportedLocale } from "./registry";

export type { LanguagePreference, SupportedLocale } from "./registry";
export { localeRegistry } from "./registry";

// The generic is pinned so the controller's preference type is the registry's
// full tag set; `languageDefaults` names one default per language and is
// deliberately narrower than the registry.
const controller = createLocalizationController<SupportedLocale>({
    registry: localeRegistry,
    defaults: languageDefaults,
    readPreference: () => window.localStorage.getItem(appConfig.languageStorageKey),
    writePreference: (value) => window.localStorage.setItem(appConfig.languageStorageKey, value),
    browserLanguages: () =>
        navigator.languages?.length
            ? navigator.languages
            : [
                  navigator.language,
              ],
    eventTarget: window,
    root: document.documentElement,
    deadlineMs: appConfig.languageLoadDeadlineMs,
});
export const initializeLocalization = controller.initialize;
export const languagePreference = controller.preference;
export const activeLocale = controller.activeLocale;
export const translation = controller.translation;
export const languageChangeState = controller.changeState;
export const setLanguagePreference = controller.setPreference;
export const retryLanguageChange = controller.retry;

// The singleton lives for the page's lifetime, so it publishes no disposal binding: `dispose`
// stays on the controller the factory returns (see `./controller`).
