import { credentialExpiryLine, nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { lapsedCredentialStanding } from "/src/management-api/serving";
import { isOAuthProviderRow, type OauthProviderRow } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { noStoredExpirySentence } from "../providers/credential-presentation";

/**
 * The overview's attention derivations (README §7): the pure calculations
 * that turn the entity listings into the "Needs attention" lines — which
 * providers need a human, why, and the action that answers the cause. The
 * screen renders the lines; this module decides there is one to render.
 */

/** One listed attention line: its provider, the human reason it needs one, and the action that answers it. */
export type ProviderAttention = {
    readonly provider: ProviderResponse;
    readonly attention: {
        readonly label: string;
        readonly detail: string;
        readonly action: {
            readonly href: string;
            readonly label: string;
        };
    };
};

/**
 * One enabled provider with no ready inventory, together with the
 * attention its cause calls for.
 */
export type ProviderInventoryAttention = {
    readonly provider: ProviderResponse;
    readonly attention: {
        readonly reason: "no-models" | "no-ready-models";
        readonly action: {
            readonly href: string;
            readonly label: string;
        };
    };
};

/**
 * The attention an enabled provider with no ready inventory carries,
 * chosen by the cause its rows name: a provider holding no model rows at all
 * is missing its discovery — the provider's page runs it — while a provider
 * whose rows exist but none serve holds only switched-off or vanished rows,
 * which discovery cannot repair; its models page states each row's own cause.
 */
export function inventoryGapAttention(
    models: ReadonlyArray<ModelResponse>,
    providerId: number,
): {
    readonly reason: "no-models" | "no-ready-models";
    readonly action: {
        readonly href: string;
        readonly label: string;
    };
} {
    const hasNoModels = !models.some((model) => model.provider_id === providerId);

    return hasNoModels
        ? {
              reason: "no-models",
              action: {
                  get label() {
                      return translation()(($) => $.screens.homeScreen.openAndDiscover);
                  },
                  href: hrefs.providerDetail(providerId),
              },
          }
        : {
              reason: "no-ready-models",
              action: {
                  get label() {
                      return translation()(($) => $.screens.homeScreen.reviewModels);
                  },
                  href: hrefs.models(providerId),
              },
          };
}

/**
 * The lapsed-credential wording itself: expired, or never fresh — `null`
 * while the credential still holds. Takes the OAuth member of the row union,
 * since the expiry is an OAuth row's column alone.
 */
function lapsedCredentialAttention(provider: OauthProviderRow): {
    readonly label: string;
    readonly detail: string;
} | null {
    // The gate is `serving.ts`'s one lapsed composition over the row. The
    // missing-expiry column below is the never-fresh standing's own
    // definition in `credentialStanding`, so past the gate the branch is
    // already fixed and the non-null column feeds the expired line's
    // absolute instant.
    if (lapsedCredentialStanding(provider) === null) {
        return null;
    }

    if (provider.expires_at === null) {
        return {
            get label() {
                return translation()(($) => $.screens.homeScreen.credentialNotFresh);
            },
            get detail() {
                return noStoredExpirySentence();
            },
        };
    }

    return {
        get label() {
            return translation()(($) => $.screens.homeScreen.credentialExpired);
        },
        get detail() {
            return credentialExpiryLine(provider.expires_at!, nowMicros());
        },
    };
}

/**
 * Why an OAuth provider needs a human: expired, or never fresh. The row's
 * action follows the provider's parked state: reauthorization lives on the
 * edit page's Credentials card, but a disabled provider is the operator's
 * own choice, so its row names the parked state and opens the provider's
 * detail page — read-only itself, its *Edit provider* door reaching the
 * edit page's Settings card where the Enabled switch lives — instead of
 * ordering a reconnect the operator deliberately deferred. The credential
 * counts either way: a parked provider re-enabled with lapsed credentials
 * fails every request.
 */
export function credentialAttention(
    provider: ProviderResponse,
): ProviderAttention["attention"] | null {
    if (!isOAuthProviderRow(provider)) {
        return null;
    }

    const lapsed = lapsedCredentialAttention(provider);

    if (lapsed === null) {
        return null;
    }

    return provider.is_enabled
        ? {
              get label() {
                  return lapsed.label;
              },
              get detail() {
                  return lapsed.detail;
              },
              action: {
                  href: hrefs.providerEdit(provider.id),
                  get label() {
                      return translation()(($) => $.screens.homeScreen.reconnect);
                  },
              },
          }
        : {
              get label() {
                  return lapsed.label;
              },
              get detail() {
                  return translation()(($) => $.dynamicMessages.disabledCredential, {
                      expiry: lapsed.detail,
                  });
              },
              action: {
                  href: hrefs.providerDetail(provider.id),
                  get label() {
                      return translation()(($) => $.screens.homeScreen.openProvider);
                  },
              },
          };
}
