import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { credentialExpiryLine, formatTimestamp, nowMicros } from "/src/format";
import { toUpdateProviderRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    OAuthProviderType,
    ProviderResponse,
    ReauthorizeProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    compositionMessage,
    isBaseUrl,
    isBearerSecret,
    isModelPrefix,
} from "/src/management-api/identifiers";
import { extractAuthorizationCode } from "/src/management-api/oauth-code";
import { performManagementCall } from "/src/management-api/outcome";
import { defaultModelPrefix, normalizeModelPrefix } from "/src/management-api/provider-defaults";
import { findOAuthProviderType, isOAuthProviderRow } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import {
    createHeldRow,
    createScreenRead,
    type FailureCall,
    isSameProviderRow,
    readAll,
    type ScreenNotice,
} from "/src/screens/read";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button, ButtonLink, TextField } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { AuthorizeStep } from "/src/ui/oauth-flow";
import {
    CompatibleConnectionFields,
    ProviderEnabledSwitch,
    ProviderIdentityFields,
} from "/src/ui/provider-form";

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
    // Reported by the settings form: an open form with unsaved edits holds its
    // row against refreshes, so an operator's input can never silently vanish.
    const [isFormDirty, setIsFormDirty] = createSignal(false);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        reReadOn: () => props.providerId,
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                {
                    run: (cycle) => props.api.listProviders(cycle),
                },
                {
                    run: (cycle) =>
                        props.api.listModels(
                            {
                                provider_id: props.providerId,
                            },
                            cycle,
                        ),
                },
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);
        },
    });

    const latestProvider = createMemo(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
    );

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the form holds unsaved edits, and the hold is stated.
    const { row: provider, isRowHeld } = createHeldRow(
        latestProvider,
        isFormDirty,
        isSameProviderRow,
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={read.notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={read.isReading() && !read.hasRead() && read.notice() === null}>
                <PageHeader
                    title="Edit provider"
                    backHref={hrefs.providerDetail(props.providerId)}
                    backLabel="Provider"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead() && provider() === null}>
                <PageHeader
                    title="Edit provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <EmptyState
                    title="Provider not found"
                    description="The gateway holds no provider here. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back."
                >
                    <ButtonLink
                        href={hrefs.providers()}
                        tone="primary"
                    >
                        Back to providers
                    </ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={provider()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={`Edit ${row.name}`}
                            backHref={hrefs.providerDetail(row.id)}
                            backLabel="Provider"
                        />

                        <Show when={isRowHeld()}>
                            <Notice
                                tone="warn"
                                title="This provider changed on the gateway while you have unsaved edits."
                                detail="Save writes your edits over the gateway's version; Revert picks the gateway's version up."
                            />
                        </Show>

                        <DetailsCard
                            api={props.api}
                            provider={row}
                            models={models()}
                            onReload={read.reload}
                            onNotice={read.showNotice}
                            onFailure={read.reportFailure}
                            onDirtyChange={setIsFormDirty}
                        />

                        <Show
                            when={findOAuthProviderType(row.type)}
                            keyed
                        >
                            {(oauthType) => (
                                <CredentialsCard
                                    api={props.api}
                                    provider={row}
                                    oauthType={oauthType}
                                    onReload={read.reload}
                                    onNotice={read.showNotice}
                                    onFailure={read.reportFailure}
                                />
                            )}
                        </Show>

                        <DangerCard
                            api={props.api}
                            provider={row}
                            modelCount={models().length}
                            onFailure={read.reportFailure}
                        />
                    </>
                )}
            </Show>
        </div>
    );
}

/** The provider's editable settings. Saving writes the complete readable state. */
function DetailsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly onReload: () => void;
    readonly onNotice: (notice: ScreenNotice | null) => void;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
    readonly onDirtyChange: (isDirty: boolean) => void;
}): JSX.Element {
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the values it sent, because the write answers `{}` and the
    // resulting row is otherwise known only to this form. The API key is
    // never part of it: the secret is write-only.
    const [baseline, setBaseline] = createSignal(props.provider);
    const [isEnabled, setIsEnabled] = createSignal(props.provider.is_enabled);
    const [name, setName] = createSignal(props.provider.name);
    // A stored prefix equal to the name with a slash is the default, so the
    // switch opens off; anything else is a hand-set value worth keeping.
    const [isCustomPrefix, setIsCustomPrefix] = createSignal(
        props.provider.model_prefix !== `${props.provider.name}/`,
    );
    const [customPrefix, setCustomPrefix] = createSignal(
        props.provider.model_prefix !== `${props.provider.name}/`
            ? props.provider.model_prefix
            : "",
    );
    // A static key's row carries the stored base URL; an OAuth login's has none.
    const [baseUrl, setBaseUrl] = createSignal(
        isOAuthProviderRow(props.provider) ? "" : props.provider.base_url,
    );
    const [apiKey, setApiKey] = createSignal("");
    const [isSaving, setIsSaving] = createSignal(false);

    const isStatic = (): boolean => findOAuthProviderType(baseline().type) === null;

    const effectivePrefix = createMemo(() =>
        isCustomPrefix() ? normalizeModelPrefix(customPrefix()) : defaultModelPrefix(name()),
    );

    const trimmedBaseUrl = (): string => baseUrl().trim();

    /** Inline composition errors, shown where the value was typed (gateway README §7). */
    const prefixError = (): string | null =>
        isModelPrefix(effectivePrefix()) ? null : compositionMessage("Model prefix");

    const baseUrlError = (): string | null => {
        const trimmed = trimmedBaseUrl();
        return trimmed === "" || isBaseUrl(trimmed) ? null : baseUrlMessage();
    };

    const apiKeyError = (): string | null =>
        apiKey() === "" || isBearerSecret(apiKey()) ? null : compositionMessage("API key");

    /** The connection fields are submittable when every typed value is routable. The key stays
     * empty to keep the stored secret; the URL is required. */
    const isConnectionValid = (): boolean =>
        !isStatic() ||
        (isBaseUrl(trimmedBaseUrl()) && (apiKey() === "" || isBearerSecret(apiKey())));

    const isDirty = createMemo(() => {
        const baselineRow = baseline();

        return (
            isEnabled() !== baselineRow.is_enabled ||
            name() !== baselineRow.name ||
            effectivePrefix() !== baselineRow.model_prefix ||
            (isStatic() &&
                (trimmedBaseUrl() !==
                    (isOAuthProviderRow(baselineRow) ? "" : baselineRow.base_url) ||
                    apiKey() !== ""))
        );
    });

    // A name is a human label, but the contract still requires it to hold
    // something: an emptied field must not reach the gateway as a 400.
    const isNamePresent = (): boolean => name().trim() !== "";

    const canSave = (): boolean =>
        isDirty() && isNamePresent() && prefixError() === null && isConnectionValid();

    // The screen holds the form's row while this dirt stands (README §6.3).
    createEffect(
        () => isDirty(),
        (isDirtyValue) => {
            props.onDirtyChange(isDirtyValue);
        },
    );

    const revert = (): void => {
        const baselineRow = baseline();

        setIsEnabled(baselineRow.is_enabled);
        setName(baselineRow.name);
        setIsCustomPrefix(baselineRow.model_prefix !== `${baselineRow.name}/`);
        setCustomPrefix(
            baselineRow.model_prefix !== `${baselineRow.name}/` ? baselineRow.model_prefix : "",
        );
        setBaseUrl(isOAuthProviderRow(baselineRow) ? "" : baselineRow.base_url);
        setApiKey("");
        // Reverting resolves a held row onto the gateway's current version.
        props.onReload();
    };

    const save = async (): Promise<void> => {
        if (isSaving() || !canSave()) {
            return;
        }

        setIsSaving(true);

        const outcome = await performManagementCall(() =>
            props.api.updateProvider(
                toUpdateProviderRequest(props.provider, {
                    name: name(),
                    model_prefix: effectivePrefix(),
                    is_enabled: isEnabled(),
                    ...(isStatic()
                        ? {
                              base_url: trimmedBaseUrl(),
                              ...(apiKey() === ""
                                  ? {}
                                  : {
                                        api_key: apiKey(),
                                    }),
                          }
                        : {}),
                }),
            ),
        );

        setIsSaving(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, {
                action: "update-provider",
            });
            return;
        }

        // The warning states a recomposition only when the save actually
        // recomposes one: the gateway rewrites exactly the unoverridden
        // models whose composed ID differs from the public ID it already
        // holds, so the submitted prefix is measured against those rows —
        // never against this form's first-read baseline, which a held row
        // can leave behind what the gateway holds now.
        const prefix = effectivePrefix();
        const recomposesPublicIds = props.models.some(
            (model) =>
                model.id_override === null && model.public_id !== `${prefix}${model.upstream_id}`,
        );

        // The write's complete desired state: the update rewrote exactly
        // these fields, so the row they describe is this form's new truth.
        setBaseline(
            isOAuthProviderRow(baseline())
                ? {
                      ...baseline(),
                      name: name(),
                      model_prefix: effectivePrefix(),
                      is_enabled: isEnabled(),
                  }
                : {
                      ...baseline(),
                      name: name(),
                      model_prefix: effectivePrefix(),
                      is_enabled: isEnabled(),
                      base_url: trimmedBaseUrl(),
                  },
        );
        setApiKey("");
        props.onNotice({
            tone: "ok",
            title: `${name()} saved.`,
            detail: recomposesPublicIds
                ? "The new prefix recomposes the public ID of every model without an override."
                : null,
        });
        props.onReload();
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void save();
    };

    return (
        <Card>
            <CardHeader title="Settings" />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={submit}
            >
                <ProviderEnabledSwitch
                    isEnabled={isEnabled()}
                    onToggle={setIsEnabled}
                    isDisabled={isSaving()}
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
                    isDisabled={isSaving()}
                />

                <Show when={isStatic()}>
                    <CompatibleConnectionFields
                        baseUrl={baseUrl()}
                        apiKey={apiKey()}
                        onBaseUrlInput={setBaseUrl}
                        onApiKeyInput={setApiKey}
                        apiKeyLabel="Replace the API key"
                        apiKeyHint="Write-only: the gateway never returns it, so leaving this empty keeps what is stored."
                        baseUrlError={baseUrlError()}
                        apiKeyError={apiKeyError()}
                        isDisabled={isSaving()}
                    />
                </Show>

                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        type="submit"
                        tone="primary"
                        isDisabled={!canSave()}
                        isBusy={isSaving()}
                    >
                        {isSaving() ? "Saving…" : "Save changes"}
                    </Button>
                    <Button
                        isDisabled={!isDirty() || isSaving()}
                        onClick={revert}
                    >
                        Revert
                    </Button>
                    <Show when={isDirty() && !isSaving()}>
                        <span
                            class="text-[13px] font-medium text-warn"
                            role="status"
                        >
                            Unsaved changes
                        </span>
                    </Show>
                </div>
            </form>
        </Card>
    );
}

/** An OAuth provider's credential health and its reauthorization flow. */
function CredentialsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly oauthType: OAuthProviderType;
    readonly onReload: () => void;
    readonly onNotice: (notice: ScreenNotice | null) => void;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
}): JSX.Element {
    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const expiryLine = (): string => {
        if (!isOAuthProviderRow(props.provider) || props.provider.expires_at === null) {
            return "No expiry is stored for this provider.";
        }

        return credentialExpiryLine(props.provider.expires_at, nowMicros());
    };

    const start = async (): Promise<void> => {
        if (isStarting()) {
            return;
        }

        setIsStarting(true);

        const outcome = await performManagementCall(() =>
            props.api.startProviderOauth({
                type: props.oauthType,
            }),
        );

        setIsStarting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, {
                action: "start-oauth",
            });
            return;
        }

        setFlow(outcome.result);
    };

    const complete = async (): Promise<void> => {
        const startedFlow = flow();

        if (startedFlow === null || isCompleting()) {
            return;
        }

        const pasted = extractAuthorizationCode(code());

        if (pasted === null) {
            props.onNotice({
                tone: "bad",
                title: "Paste the callback URL or the code.",
                detail: "That paste holds neither a bare code nor a URL with a code in it.",
            });
            return;
        }

        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.reauthorizeProvider({
                state: startedFlow.state,
                code: pasted,
                provider_id: props.provider.id,
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, {
                action: "reauthorize",
            });

            if (diagnosis.kind === "gone") {
                // The state is consumed; the flow resets and the listing
                // re-reads instead of either being retried.
                setFlow(null);
                setCode("");
                props.onReload();
            }

            props.onFailure(outcome.cause, {
                action: "reauthorize",
            });
            return;
        }

        const replacement: ReauthorizeProviderResponse = outcome.result;
        setFlow(null);
        setCode("");
        props.onNotice({
            tone: "ok",
            title: "Credentials replaced.",
            detail: `The new credentials expire ${formatTimestamp(replacement.expires_at)}.`,
        });
        props.onReload();
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void complete();
    };

    return (
        <Card>
            <CardHeader
                title="Credentials"
                description="OAuth credentials expire — reconnecting replaces them."
            />
            <div class="mt-4 flex max-w-2xl flex-col gap-4">
                <p class="text-[13.5px] text-ink-dim">{expiryLine()}</p>

                <Show
                    when={flow()}
                    keyed
                    fallback={
                        <div>
                            <Button
                                tone="primary"
                                glyph="key"
                                isBusy={isStarting()}
                                onClick={() => void start()}
                            >
                                {isStarting() ? "Starting…" : "Reconnect the account"}
                            </Button>
                        </div>
                    }
                >
                    {(startedFlow) => (
                        <form
                            novalidate={true}
                            class="flex flex-col gap-4"
                            onSubmit={submit}
                        >
                            <AuthorizeStep
                                flow={startedFlow}
                                onStartOver={() => {
                                    setFlow(null);
                                    setCode("");
                                }}
                            >
                                <TextField
                                    label="Callback URL or authorization code"
                                    value={code()}
                                    onInput={setCode}
                                    placeholder="Paste the whole callback URL"
                                    isDisabled={isCompleting()}
                                />
                                <div>
                                    <Button
                                        type="submit"
                                        tone="primary"
                                        isBusy={isCompleting()}
                                    >
                                        {isCompleting() ? "Replacing…" : "Replace the credentials"}
                                    </Button>
                                </div>
                            </AuthorizeStep>
                        </form>
                    )}
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
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
}): JSX.Element {
    const confirmDelete = async (): Promise<boolean> => {
        const outcome = await performManagementCall(() =>
            props.api.deleteProvider({
                provider_id: props.provider.id,
            }),
        );

        if (outcome.status === "aborted") {
            return false;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, {
                action: "delete-provider",
            });
            return false;
        }

        navigate(hrefs.providers());
        return true;
    };

    // A provider with no models cascades nothing: the count phrase and the
    // clients' 404s name consequences that do not exist and must not fire.
    const description =
        props.modelCount === 0
            ? "Deleting removes the provider. It has no models, so nothing else is removed."
            : `Deleting removes the provider and, by cascade, its ${props.modelCount} ${props.modelCount === 1 ? "model" : "models"}.`;
    const question =
        props.modelCount === 0
            ? `Delete ${props.provider.name}?`
            : `Delete ${props.provider.name}? Clients calling those public IDs will start getting 404s.`;

    return (
        <ConfirmDeleteCard
            title="Delete this provider"
            description={description}
            question={question}
            onConfirm={confirmDelete}
        />
    );
}
