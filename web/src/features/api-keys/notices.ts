import { translation } from "/src/i18n";
import { savedNameMessage } from "/src/i18n/rich";
import type { ScreenNotice } from "/src/screen-state/notices";

type ApiKeySaveFacts = {
    readonly name: string;
    readonly isRotatingSecret: boolean;
    readonly isTurningOffLastEnabled: boolean;
};

/**
 * The serving-key edit page's save outcome (`web/docs/architecture.md`): the saved key's
 * name, stating the rotation's consequence where the secret changed — the
 * previous secret stopped working on this save — composed with the
 * last-enabled line where the same save also turns off the last enabled key.
 */
export function apiKeySavedNotice(facts: ApiKeySaveFacts): ScreenNotice {
    return {
        tone: "ok",
        get title() {
            return savedNameMessage(facts.name);
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
