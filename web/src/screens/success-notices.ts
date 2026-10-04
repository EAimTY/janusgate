import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { discoveredMessage, savedNameMessage } from "/src/i18n/rich";
import type { ScreenNotice } from "./read";

type DiscoveryFacts = {
    readonly providerName: string;
    readonly added: number;
    readonly manual: number;
    readonly vanished: number;
};
/** Retains discovery facts; the standing notice renders them in the current language. */
export function createDiscoveryNotice(facts: DiscoveryFacts): ScreenNotice & {
    readonly type: "discovered";
    readonly facts: DiscoveryFacts;
} {
    return {
        type: "discovered",
        facts,
        tone: "ok",
        get title() {
            return discoveredMessage(translation(), facts.providerName, facts.added);
        },
        get detail() {
            const lines: string[] = [];
            if (facts.manual > 0)
                lines.push(
                    translation()(($) => $.notices.manualUntouched, {
                        count: facts.manual,
                        quantity: formatNumber(facts.manual),
                    }),
                );
            if (facts.vanished > 0)
                lines.push(
                    translation()(($) => $.notices.vanished, {
                        count: facts.vanished,
                        quantity: formatNumber(facts.vanished),
                    }),
                );
            return lines.length ? lines.join(" ") : null;
        },
    };
}
type ProviderSaveFacts = {
    readonly name: string;
    readonly isApiKeyReplaced: boolean;
};
export function providerSavedNotice(facts: ProviderSaveFacts): ScreenNotice & {
    readonly type: "provider-saved";
    readonly facts: ProviderSaveFacts;
} {
    return {
        type: "provider-saved",
        facts,
        tone: "ok",
        get title() {
            return savedNameMessage(translation(), facts.name);
        },
        get detail() {
            return facts.isApiKeyReplaced ? translation()(($) => $.notices.keyReplaced) : null;
        },
    };
}
type ApiKeySaveFacts = {
    readonly name: string;
    readonly isRotatingSecret: boolean;
    readonly isTurningOffLastEnabled: boolean;
};
export function apiKeySavedNotice(facts: ApiKeySaveFacts): ScreenNotice & {
    readonly type: "api-key-saved";
    readonly facts: ApiKeySaveFacts;
} {
    return {
        type: "api-key-saved",
        facts,
        tone: "ok",
        get title() {
            return savedNameMessage(translation(), facts.name);
        },
        get detail() {
            const lines: string[] = [];
            if (facts.isRotatingSecret)
                lines.push(translation()(($) => $.notices.previousSecretRevoked));
            if (facts.isTurningOffLastEnabled)
                lines.push(translation()(($) => $.notices.lastKeyDisabled));
            return lines.length ? lines.join(" ") : null;
        },
    };
}
