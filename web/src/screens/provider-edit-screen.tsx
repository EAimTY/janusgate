import type { JSX } from "@solidjs/web";
import { type Accessor, createMemo, createSignal, Show } from "solid-js";
import { credentialExpiryLine, nowMicros, pluralNoun } from "/src/format";
import { toUpdateProviderRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    bearerSecretError,
    isBaseUrl,
    isBearerSecret,
    modelPrefixError,
} from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { lapsedCredentialStanding } from "/src/management-api/serving";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import {
    createEditRow,
    createScreenRead,
    isSameProviderRow,
    readOne,
    readProviderCatalog,
    type ScreenRead,
} from "/src/screens/read";
import { StatedNotice } from "/src/screens/read-notice";
import { ScreenFrame } from "/src/screens/screen-frame";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import {
    CompatibleConnectionFields,
    ProviderEnabledSwitch,
    ProviderIdentityFields,
} from "/src/ui/provider-form";
import { NO_STORED_EXPIRY_SENTENCE } from "/src/ui/providers-table";
import { createDeleteAction, createEditForm } from "./edit-form";
import { createOauthFlow, type OauthFlow } from "./oauth-flow";
import { CompletionForm, DeviceStep } from "./oauth-flow-view";
import { defaultModelPrefix } from "./provider-defaults";
import { RowNotFound } from "./row-not-found";

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
                title: "Credentials replaced.",
                detail: "The provider's listing now carries the new expiry.",
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
                    title="Edit provider"
                    backHref={hrefs.providerDetail(props.providerId)}
                    backLabel="Provider"
                />
            }
            notFound={
                <RowNotFound
                    kind="provider"
                    headerTitle="Edit provider"
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={`Edit ${row.name}`}
                        backHref={hrefs.providerDetail(row.id)}
                        backLabel="Provider"
                    />

                    <HoldNotice
                        isHeld={isRowHeld()}
                        subject="provider"
                    />

                    <DetailsCard
                        api={props.api}
                        provider={row}
                        models={models()}
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
    readonly isCustomPrefix: boolean;
    readonly customPrefix: string;
    readonly baseUrl: string | null;
} {
    // A stored prefix equal to the default is the default, so the switch
    // opens off; anything else is a hand-set value worth keeping.
    const isCustomPrefix = row.model_prefix !== defaultModelPrefix(row.name);

    return {
        isEnabled: row.is_enabled,
        name: row.name,
        isCustomPrefix,
        customPrefix: isCustomPrefix ? row.model_prefix : "",
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
const EMPTY_BASE_URL_ERROR =
    "This provider's definition evaluates requests against a base URL — restore one, or Revert.";

/**
 * The models whose public ID saving the given prefix would rewrite: the
 * unoverridden rows whose stored public ID differs from the ID the prefix
 * composes over their upstream ID — exactly the rows the gateway recomposes
 * on the write (gateway README §7). The pre-save warning counts the rows
 * with this one predicate and the save notice answers whether it recomposes
 * anything with it, so the operator is warned about precisely what the
 * write's outcome later states, never a near-copy that can drift.
 */
function countRecomposedPublicIds(models: ReadonlyArray<ModelResponse>, prefix: string): number {
    return models.filter(
        (model) =>
            model.id_override === null && model.public_id !== `${prefix}${model.upstream_id}`,
    ).length;
}

/**
 * The provider's editable settings. Saving writes the complete readable state.
 */
function DetailsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly models: ReadonlyArray<ModelResponse>;
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
    const [isCustomPrefix, setIsCustomPrefix] = createSignal(seed.isCustomPrefix);
    const [customPrefix, setCustomPrefix] = createSignal(seed.customPrefix);
    const [baseUrl, setBaseUrl] = createSignal(seed.baseUrl);
    const [apiKey, setApiKey] = createSignal("");

    const isStatic = (): boolean => !isOAuthProviderRow(baseline());

    // The typed prefix is the value on the wire, byte-for-byte: the gateway
    // stores `model_prefix` verbatim and recomposes every unoverridden
    // public ID from exactly what the write states (gateway README §7), and
    // the field promises it is used exactly as typed — normalizing here
    // would silently rewrite the clients' routing and mark the form dirty
    // against its own baseline. Composition is guarded below, where it is
    // typed.
    const effectivePrefix = createMemo(() =>
        isCustomPrefix() ? customPrefix() : defaultModelPrefix(name()),
    );

    // The pre-save warning's gate and count: a prefix off the row the form
    // opens on (a clause of `isDirty` itself, so a changed prefix never
    // stands on a pristine form). The count reads the same rows the save
    // notice will measure — the listing as the gateway holds it now, which a
    // held row can leave behind — and it is always computable here: this
    // form renders only once the read cycle that fills both listings has
    // answered, so the rows are never an unlanded read.
    const prefixRecompositionCount = createMemo(() => {
        const prefix = effectivePrefix();

        return prefix === baseline().model_prefix
            ? null
            : countRecomposedPublicIds(props.models, prefix);
    });

    /** The blast radius of saving the changed prefix, stated before the
     * write in one line: the count of the public IDs it would rewrite.
     * Nothing stands over a count of zero — the line would name a
     * consequence that does not exist, the same rule the deletion
     * confirmation's cascade phrase follows. */
    const prefixChangeWarning = (): string | null => {
        const count = prefixRecompositionCount();

        return count === null || count === 0
            ? null
            : `Saving recomposes ${count} model ${pluralNoun(count, "ID", "IDs")} — clients calling the old IDs will 404 until updated.`;
    };

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
    const prefixError = (): string | null => modelPrefixError(effectivePrefix());

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

        return trimmedBaseUrl() === "" ? EMPTY_BASE_URL_ERROR : baseUrlMessage();
    };

    const apiKeyError = (): string | null => bearerSecretError(apiKey(), "API key");

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
            effectivePrefix() !== baselineRow.model_prefix ||
            (isStatic() && (trimmedBaseUrl() !== (baselineRow.base_url ?? "") || apiKey() !== ""))
        );
    };

    // A name is the administrator's choice, stored verbatim (gateway README
    // §7): the only rule is non-empty — an emptied field must not reach the
    // gateway as a 400 — and nothing is trimmed, because rejecting more than
    // the gateway would block a legal name.
    const isNamePresent = (): boolean => name() !== "";

    const canSave = (): boolean =>
        isDirty() && isNamePresent() && prefixError() === null && isConnectionValid();

    const revert = (): void => {
        const seed = seedFrom(baseline());

        setIsEnabled(seed.isEnabled);
        setName(seed.name);
        setIsCustomPrefix(seed.isCustomPrefix);
        setCustomPrefix(seed.customPrefix);
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
                    model_prefix: effectivePrefix(),
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
            // The warning states a recomposition only when the save actually
            // recomposes one: the submitted prefix is measured against the
            // rows the gateway holds now — never against this form's
            // first-read baseline, which a held row can leave behind what
            // the gateway holds now — through the same predicate the
            // pre-save warning counted.
            const recomposesPublicIds =
                countRecomposedPublicIds(props.models, effectivePrefix()) > 0;

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
                    modelPrefix: effectivePrefix(),
                    isEnabled: isEnabled(),
                    baseUrl: resolvedBaseUrl(),
                }),
            );
            setApiKey("");

            // The save's consequence lines, composed the way the API-key
            // edit's save composes its own: one line per consequence this
            // save actually carries, joined where more than one stands.
            const consequences: string[] = [];
            if (recomposesPublicIds) {
                consequences.push(
                    "The new prefix recomposes the public ID of every model without an override.",
                );
            }
            if (isApiKeyReplaced) {
                consequences.push("The upstream API key was replaced.");
            }

            return {
                tone: "ok",
                title: `${name()} saved.`,
                detail: consequences.length > 0 ? consequences.join(" ") : null,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader title="Settings" />
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
                    onNameInput={setName}
                    nameError={isNamePresent() ? null : "A provider needs a name."}
                    isCustomPrefix={isCustomPrefix()}
                    onCustomPrefixToggle={setIsCustomPrefix}
                    customPrefix={customPrefix()}
                    onCustomPrefixInput={setCustomPrefix}
                    defaultPrefix={defaultModelPrefix(name())}
                    prefixError={prefixError()}
                    prefixChangeWarning={prefixChangeWarning()}
                    isDisabled={form.isSaving()}
                />

                <Show when={isStatic()}>
                    <CompatibleConnectionFields
                        baseUrl={baseUrl() ?? ""}
                        apiKey={apiKey()}
                        onBaseUrlInput={setBaseUrl}
                        onApiKeyInput={setApiKey}
                        apiKeyLabel="Replace the upstream API key"
                        apiKeyHint="Write-only: the gateway never returns it, so leaving this empty keeps what is stored."
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
            return NO_STORED_EXPIRY_SENTENCE;
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
                title="Credentials"
                description="OAuth credentials expire — reconnecting replaces them."
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
                                        ? "Starting…"
                                        : "Reconnect the account"}
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
                                busyLabel="Replacing…"
                                submitLabel="Replace the credentials"
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

    // A provider with no models cascades nothing: the count phrase and the
    // clients' 404s name consequences that do not exist and must not fire.
    // The wording reads the props inline, so the reads land in the card's
    // tracking scope instead of this component's setup.
    return (
        <ConfirmDeleteCard
            title="Delete this provider"
            description={
                props.modelCount === 0
                    ? "Deleting removes the provider. It has no models, so nothing else is removed."
                    : `Deleting removes the provider and, by cascade, its ${props.modelCount} ${pluralNoun(props.modelCount, "model")}.`
            }
            question={
                props.modelCount === 0
                    ? `Delete ${props.provider.name}?`
                    : `Delete ${props.provider.name}? Clients calling those public IDs will start getting 404s.`
            }
            onConfirm={confirmDelete}
        />
    );
}
