import { appConfig } from "/src/config";
import { createLocalizationController } from "./controller";
import { languageDefaults, localeRegistry } from "./registry";

export type { LanguagePreference, SupportedLocale } from "./registry";
export { localeRegistry } from "./registry";

const controller = createLocalizationController({
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
export const disposeLocalization = controller.dispose;
