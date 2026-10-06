import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { discoveredMessage, savedNameMessage } from "/src/i18n/rich";
import type { ScreenNotice } from "/src/screen-state/notices";

type DiscoveryFacts = {
    readonly providerName: string;
    readonly added: number;
    readonly manual: number;
    readonly vanished: number;
};

/**
 * The provider page's discovery outcome (README §6.3): the headline counts
 * the models the upstream newly reports against the listing held before the
 * discovery, and the detail names the untouched manual rows and the vanished
 * rows the discovery kept. The facts are held, not rendered — the standing
 * notice reads them lazily, so it states them in the language current when
 * it renders.
 */
export function createDiscoveryNotice(facts: DiscoveryFacts): ScreenNotice {
    return {
        tone: "ok",
        get title() {
            return discoveredMessage(facts.providerName, facts.added);
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

/**
 * The provider edit page's save outcome (README §6.3): the saved account's
 * name, stating the replaced upstream key where the same save carried one —
 * the write-only field empties on the save, so this notice is the one place
 * the replacement is stated.
 */
export function providerSavedNotice(facts: ProviderSaveFacts): ScreenNotice {
    return {
        tone: "ok",
        get title() {
            return savedNameMessage(facts.name);
        },
        get detail() {
            return facts.isApiKeyReplaced ? translation()(($) => $.notices.keyReplaced) : null;
        },
    };
}
