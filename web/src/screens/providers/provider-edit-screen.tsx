import type { JSX } from "@solidjs/web";
import { type Accessor, createMemo, createSignal, Show } from "solid-js";
import { readProviderCatalog } from "/src/features/inventory/read";
import { isSameProviderRow } from "/src/features/inventory/row-equality";
import { noStoredExpirySentence } from "/src/features/providers/credential-presentation";
import { providerSavedNotice } from "/src/features/providers/notices";
import { createOauthFlow, type OauthFlow } from "/src/features/providers/oauth-flow";
import { CompletionForm, DeviceStep } from "/src/features/providers/oauth-flow-view";
import {
    CompatibleConnectionFields,
    ProviderEnabledSwitch,
    ProviderIdentityFields,
} from "/src/features/providers/provider-form";
import { credentialExpiryLine, nowMicros } from "/src/format";
import { translation } from "/src/i18n";
import { formatNumber } from "/src/i18n/formatters";
import { deleteProviderMessage, editNameMessage } from "/src/i18n/rich";
import { toUpdateProviderRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    bearerSecretError,
    isBaseUrl,
    isBearerSecret,
    isRoutingToken,
    routingTokenError,
} from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { lapsedCredentialStanding } from "/src/management-api/serving";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { createDeleteAction, createEditForm } from "/src/screen-state/edit-form";
import { createScreenRead, readOne, type ScreenRead } from "/src/screen-state/read";
import { createEditRow } from "/src/screen-state/row-state";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { StatedNotice } from "/src/ui/read-notice";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

/**
 * One provider's edit page: its settings, its credentials, and its deletion.
 * The settings form opens prefilled, tracks dirt, and saves the row's
 * complete state back. Reading the provider — its models and its usage —
 * lives on the detail page.
 */
export function ProviderEditScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);

    const latestProvider = createMemo(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
    );

    // The held row with its save-settling latch: an open form with unsaved
    // edits holds its row against refreshes, so an operator's input can never
    // silently vanish, and a succeeded save holds it until its own reload
    // lands (README §6.3).
    const edit = createEditRow(latestProvider, isSameProviderRow);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const catalog = await readOne(signal, readProviderCatalog(props.api, props.providerId));

            setProviders(catalog.providers);
            setModels(catalog.models);
            edit.reportReloaded();
        },
    });

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the hold stands — and the hold is stated.
    const { row: provider, isRowHeld } = edit;

    // The OAuth flow's shared state machine, held at the screen's scope —
    // above the row-keyed content: the screen content remounts on a row the
    // gateway rewrote (the token-refresh lease moves `expires_at`, which
    // alone flips the row's identity), and a flow held inside that content
    // would be discarded silently — the pasted code gone, the gateway's
    // pending flow row orphaned. The flow's failures state themselves inside
    // the Credentials card (`oauth.failure`, rendered there), never on the
    // screen's read-level notice; a succeeded completion states the
    // replacement and reloads the listing, and a gone flow state re-reads it
    // beside the fragment's own reset. The machine is created for every
    // provider but reached only from the OAuth credentials card, and the
    // begin addresses the screen's own provider — the held row is found by
    // this very id.
    const oauth = createOauthFlow({
        api: props.api,
        startAction: "start-oauth",
        completeAction: "reauthorize",
        onGone: read.reload,
        begin: async () => ({
            status: "called" as const,
            outcome: await performManagementCall(() =>
                props.api.reauthorizeProvider({
                    provider_id: props.providerId,
                }),
            ),
        }),
        onSucceeded: () => {
            read.showNotice({
                tone: "ok",
                get title() {
                    return translation()(($) => $.notices.credentialsReplaced);
                },
                get detail() {
                    return translation()(($) => $.notices.expiryReplaced);
                },
            });
            read.reload();
        },
    });

    return (
        <ScreenFrame
            read={read}
            row={provider}
            firstReadHeader={
                <PageHeader
                    title={translation()(($) => $.screens.providerEditScreen.editProvider)}
                    backHref={hrefs.providerDetail(props.providerId)}
                    backLabel={translation()(($) => $.screens.providerEditScreen.provider)}
                />
            }
            notFound={
                <RowNotFound
                    kind="provider"
                    headerTitle={translation()(($) => $.screens.providerEditScreen.editProvider)}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={editNameMessage(row.name)}
                        backHref={hrefs.providerDetail(row.id)}
                        backLabel={translation()(($) => $.screens.providerEditScreen.provider)}
                    />

                    <HoldNotice
                        isHeld={isRowHeld()}
                        subject="provider"
                    />

                    <DetailsCard
                        api={props.api}
                        provider={row}
                        read={read}
                        onDirtyChange={edit.reportFormDirty}
                        onSaved={edit.reportSaved}
                    />

                    <Show
                        when={isOAuthProviderRow(row) ? row : false}
                        keyed
                    >
                        {(oauthRow) => (
                            <CredentialsCard
                                provider={oauthRow}
                                latestProvider={latestProvider}
                                oauth={oauth}
                            />
                        )}
                    </Show>

                    <DangerCard
                        api={props.api}
                        provider={row}
                        modelCount={models().length}
                        read={read}
                    />
                </>
            )}
        </ScreenFrame>
    );
}

/**
 * The settings one row seeds the settings form with: the field values the row
 * as the gateway last returned opens the form on. The form's init and its
 * Revert read this one derivation — reverting is re-seeding from the baseline.
 */
function seedFrom(row: ProviderResponse): {
    readonly isEnabled: boolean;
    readonly name: string;
    readonly modelPrefix: string;
    readonly baseUrl: string | null;
} {
    return {
        isEnabled: row.is_enabled,
        name: row.name,
        modelPrefix: row.model_prefix,
        // The row's stored base URL seeds the field whatever kind minted it
        // — an OAuth row carries the same nullable column — but this form
        // exposes no base-URL editor for an OAuth row, so its seed is the
        // empty string of an absent field, never a statement about the row.
        baseUrl: isOAuthProviderRow(row) ? "" : row.base_url,
    };
}

/**
 * The row the form's current values describe, restated over the row the
 * gateway last returned: `providers:update` is a full-state write, so the
 * update rewrote exactly these fields, and this restatement is the form's new
 * truth once the save answers. The secret is never part of it — it is
 * write-only — and an OAuth row's base URL is never part of it either, the
 * form exposing no editor whose value could be restated.
 */
function withEdits(
    row: ProviderResponse,
    edits: {
        readonly name: string;
        readonly modelPrefix: string;
        readonly isEnabled: boolean;
        readonly baseUrl: string | null;
    },
): ProviderResponse {
    const restated = {
        ...row,
        name: edits.name,
        model_prefix: edits.modelPrefix,
        is_enabled: edits.isEnabled,
    };

    return isOAuthProviderRow(row)
        ? restated
        : {
              ...restated,
              base_url: edits.baseUrl,
          };
}

/**
 * The inline error a base-carrying row's cleared base URL field states. The
 * row's stored `base_url` proves its definition evaluates requests against a
 * base, so the fault is emptiness, not composition — the composition message
 * would name a spacing fault over a field holding nothing — and the fix is
 * the stored address back, or the whole form back onto the row (`Revert`).
 */
const emptyBaseUrlError = (): string =>
    translation()(
        ($) =>
            $.screens.providerEditScreen
                .thisProvidersDefinitionEvaluatesRequestsAgainstABaseURLRestoreOneOrRevert,
    );

/**
 * The provider's editable settings. Saving writes the complete readable state.
 */
function DetailsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly read: ScreenRead;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the values it sent, because the write answers `{}` and the
    // resulting row is otherwise known only to this form. The API key is
    // never part of it: the secret is write-only.
    const [baseline, setBaseline] = createSignal(props.provider);
    const seed = seedFrom(props.provider);
    const [isEnabled, setIsEnabled] = createSignal(seed.isEnabled);
    const [name, setName] = createSignal(seed.name);
    const [modelPrefix, setModelPrefix] = createSignal(seed.modelPrefix);
    const [baseUrl, setBaseUrl] = createSignal(seed.baseUrl);
    const [apiKey, setApiKey] = createSignal("");

    const isStatic = (): boolean => !isOAuthProviderRow(baseline());

    const trimmedBaseUrl = (): string => (baseUrl() ?? "").trim();

    /** The base URL the wire carries: the trimmed field, or `null` when it is
     * empty — reachable only over the legal-empty rule below, since a save
     * never runs while the field shows an error. */
    const resolvedBaseUrl = (): string | null => {
        const trimmed = trimmedBaseUrl();
        return trimmed === "" ? null : trimmed;
    };

    /** Whether the trimmed field is a legal wire value: a routable URL, or
     * empty exactly when the stored row's `null` proves the definition never
     * references the base (gateway README §7) — in which case no URL may be
     * demanded here nor invented for the row. */
    const isBaseUrlLegal = (): boolean => {
        const trimmed = trimmedBaseUrl();
        return isBaseUrl(trimmed) || (trimmed === "" && baseline().base_url === null);
    };

    /** Inline composition errors, shown where the value was typed (gateway README §7). */

    /**
     * The inline error the field states where the value was typed: a cleared
     * field over a base-carrying row names the row's emptiness rule, a typed
     * but unroutable value the composition rule. The field cannot open on
     * either — the gateway stores only legal addresses, so first paint is
     * always legal and the error answers the operator's own edit.
     */
    const baseUrlError = (): string | null => {
        if (isBaseUrlLegal()) {
            return null;
        }

        return trimmedBaseUrl() === "" ? emptyBaseUrlError() : baseUrlMessage();
    };

    const apiKeyError = (): string | null => bearerSecretError(apiKey(), "apiKey");

    /** The connection fields are submittable when every typed value is routable. The key stays
     * empty to keep the stored secret; the URL follows the same legal rule the
     * field states inline, so a null-base row saves without growing a URL. */
    const isConnectionValid = (): boolean =>
        !isStatic() || (isBaseUrlLegal() && (apiKey() === "" || isBearerSecret(apiKey())));

    const isDirty = (): boolean => {
        const baselineRow = baseline();

        return (
            isEnabled() !== baselineRow.is_enabled ||
            name() !== baselineRow.name ||
            modelPrefix() !== baselineRow.model_prefix ||
            (isStatic() && (trimmedBaseUrl() !== (baselineRow.base_url ?? "") || apiKey() !== ""))
        );
    };

    // A name is the administrator's choice, stored verbatim (gateway README
    // §7): the only rule is non-empty — an emptied field must not reach the
    // gateway as a 400 — and nothing is trimmed, because rejecting more than
    // the gateway would block a legal name.
    const isNamePresent = (): boolean => name() !== "";

    const canSave = (): boolean =>
        isDirty() &&
        isNamePresent() &&
        (modelPrefix() === "" || isRoutingToken(modelPrefix())) &&
        isConnectionValid();

    const revert = (): void => {
        const seed = seedFrom(baseline());

        setIsEnabled(seed.isEnabled);
        setName(seed.name);
        setModelPrefix(seed.modelPrefix);
        setBaseUrl(seed.baseUrl);
        setApiKey("");
        // Reverting resolves a held row onto the gateway's current version.
        props.read.reload();
    };

    const form = createEditForm({
        read: props.read,
        action: "update-provider",
        isDirty,
        canSave,
        perform: () =>
            props.api.updateProvider(
                toUpdateProviderRequest(props.provider, {
                    name: name(),
                    model_prefix: modelPrefix(),
                    is_enabled: isEnabled(),
                    ...(isStatic()
                        ? {
                              base_url: resolvedBaseUrl(),
                              ...(apiKey() === ""
                                  ? {}
                                  : {
                                        api_key: apiKey(),
                                    }),
                          }
                        : {}),
                }),
            ),
        onSucceeded: () => {
            // The replacement is read before the field is emptied below:
            // empty means the stored secret stands, typed means this save
            // replaced it — and the write-only field can never show that
            // again, so the notice is the replacement's only statement.
            const isApiKeyReplaced = apiKey() !== "";

            // The write's complete desired state: the update rewrote exactly
            // these fields, so the row they describe is this form's new truth.
            setBaseline(
                withEdits(baseline(), {
                    name: name(),
                    modelPrefix: modelPrefix(),
                    isEnabled: isEnabled(),
                    baseUrl: resolvedBaseUrl(),
                }),
            );
            setApiKey("");

            return providerSavedNotice({
                name: name(),
                isApiKeyReplaced,
            });
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader title={translation()(($) => $.screens.providerEditScreen.settings)} />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={form.submit}
            >
                <ProviderEnabledSwitch
                    isEnabled={isEnabled()}
                    onToggle={setIsEnabled}
                    isDisabled={form.isSaving()}
                />

                <ProviderIdentityFields
                    name={name()}
                    namePlaceholder={props.provider.name}
                    modelPrefix={modelPrefix()}
                    onModelPrefixInput={setModelPrefix}
                    prefixError={routingTokenError(modelPrefix(), "modelPrefix")}
                    onNameInput={setName}
                    nameError={
                        isNamePresent()
                            ? null
                            : translation()(
                                  ($) => $.screens.providerEditScreen.aProviderNeedsANameMessage,
                              )
                    }
                    isDisabled={form.isSaving()}
                />

                <Show when={isStatic()}>
                    <CompatibleConnectionFields
                        baseUrl={baseUrl() ?? ""}
                        apiKey={apiKey()}
                        onBaseUrlInput={setBaseUrl}
                        onApiKeyInput={setApiKey}
                        apiKeyLabel={translation()(($) => $.remainingText.replaceTheUpstreamAPIKey)}
                        apiKeyHint={translation()(
                            ($) => $.remainingText.writeOnlyTheGatewayNeverReturnsItSoLeavingThis,
                        )}
                        baseUrlError={baseUrlError()}
                        apiKeyError={apiKeyError()}
                        isDisabled={form.isSaving()}
                    />
                </Show>

                <DirtyFormFooter
                    canSave={canSave()}
                    isSaving={form.isSaving()}
                    isDirty={isDirty()}
                    onRevert={revert}
                />
            </form>
        </Card>
    );
}

/**
 * An OAuth provider's credential health and its reauthorization flow. The
 * flow's state machine is the screen's (`oauth`), so a keyed remount of this
 * card on a rewritten row re-renders the live flow's step from the state
 * above instead of discarding it, and the flow's failures state themselves
 * here, beside the step and buttons that took the action. The expiry line and
 * the reconnect offer are display-only facts, so they track the gateway's
 * latest row rather than the row this card is mounted on: a succeeded flow's
 * reload lands its fresh credential beside the flow's own ok notice even
 * while the dirty settings form holds the older row — and a current
 * credential is never left stating it lapsed, nor offered a second flow.
 */
function CredentialsCard(props: {
    readonly provider: ProviderResponse;
    readonly latestProvider: Accessor<ProviderResponse | null>;
    readonly oauth: OauthFlow;
}): JSX.Element {
    // The row the card's display-only facts read: the listing's latest answer,
    // never the row the dirty settings form holds — the hold exists to keep
    // the form's edits, and it must not pin the credential's stated health to
    // a row the gateway has already replaced. The held row stands in only for
    // the `null` the accessor's type admits: an answer without this row hands
    // the screen to its not-found branch, which unmounts this card. The card's
    // own identity — the row the keyed remount keys on and the actions address
    // — stays the held one above.
    const credentialRow = (): ProviderResponse => props.latestProvider() ?? props.provider;

    const expiryLine = (): string => {
        const row = credentialRow();

        if (!isOAuthProviderRow(row) || row.expires_at === null) {
            return noStoredExpirySentence();
        }

        return credentialExpiryLine(row.expires_at, nowMicros());
    };

    // The reconnect offer gates on `serving.ts`'s lapsed verdict read over
    // the gateway's latest row, so a current credential is never invited
    // into a second authorization flow.
    const isReconnectOffered = (): boolean => lapsedCredentialStanding(credentialRow()) !== null;

    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.screens.providerEditScreen.credentials)}
                description={translation()(
                    ($) =>
                        $.screens.providerEditScreen
                            .oauthCredentialsExpireReconnectingReplacesThemMessage,
                )}
            />
            <div class="mt-4 flex max-w-2xl flex-col gap-4">
                <p class="text-[13.5px] text-ink-dim">{expiryLine()}</p>

                {/* The flow's failures state themselves where its step and
                    buttons stand — a refused begin or exchange, a failed
                    outcome poll, a flow's expiry — never on the screen's
                    read-level notice above the page. */}
                <StatedNotice notice={props.oauth.failure} />

                <Show
                    when={props.oauth.flow()}
                    keyed
                    fallback={
                        <Show when={isReconnectOffered()}>
                            <div>
                                <Button
                                    tone="primary"
                                    glyph="key"
                                    isBusy={props.oauth.isStarting()}
                                    onClick={() => void props.oauth.start()}
                                >
                                    {props.oauth.isStarting()
                                        ? translation()(
                                              ($) => $.screens.providerEditScreen.starting,
                                          )
                                        : translation()(
                                              ($) =>
                                                  $.screens.providerEditScreen.reconnectTheAccount,
                                          )}
                                </Button>
                            </div>
                        </Show>
                    }
                >
                    {(startedFlow) =>
                        startedFlow.grant === "device" ? (
                            <DeviceStep
                                flow={startedFlow}
                                onComplete={props.oauth.completeDevice}
                                onStartOver={props.oauth.reset}
                            />
                        ) : (
                            <CompletionForm
                                flow={props.oauth}
                                startedFlow={startedFlow}
                                busyLabel={translation()(
                                    ($) => $.screens.providerEditScreen.replacing,
                                )}
                                submitLabel={translation()(
                                    ($) => $.screens.providerEditScreen.replaceTheCredentials,
                                )}
                                class="flex flex-col gap-4"
                            />
                        )
                    }
                </Show>
            </div>
        </Card>
    );
}

/** Deletion, confirmed where it states its blast radius. */
function DangerCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly modelCount: number;
    readonly read: ScreenRead;
}): JSX.Element {
    const confirmDelete = createDeleteAction({
        read: props.read,
        action: "delete-provider",
        perform: () =>
            props.api.deleteProvider({
                provider_id: props.provider.id,
            }),
        onDeleted: () => navigate(hrefs.providers()),
    });

    // Physical deletion removes direct routes; pooled routes retain their
    // authored targets and may continue through their remaining accounts.
    // The wording reads the props inline, so the reads land in the card's
    // tracking scope instead of this component's setup.
    return (
        <ConfirmDeleteCard
            title={translation()(($) => $.screens.providerEditScreen.deleteThisProvider)}
            description={
                props.modelCount === 0
                    ? translation()(
                          ($) =>
                              $.screens.providerEditScreen
                                  .deletingRemovesTheAccountAndItsPoolMembershipsMessage,
                      )
                    : translation()(($) => $.counts.providerDeleted, {
                          count: props.modelCount,
                          total: formatNumber(props.modelCount),
                      })
            }
            question={deleteProviderMessage(props.provider.name)}
            onConfirm={confirmDelete}
        />
    );
}
