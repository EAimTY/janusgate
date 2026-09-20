import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { describeValidity, formatTimestamp } from "/src/format";
import { toUpdateProviderRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    OAuthProviderType,
    ProviderResponse,
    ReauthorizeProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import {
    findOAuthProviderType,
    isOAuthProviderRow,
    providerTypeLabel,
} from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { credentialBadge } from "/src/screens/providers-screen";
import { Button, ButtonLink, TextField } from "/src/ui/controls";
import { Badge, EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ModelList } from "/src/ui/models-table";
import { AuthorizeStep } from "/src/ui/oauth-flow";
import {
    CompatibleConnectionFields,
    ProviderIdentityFields,
    ProviderServingSwitch,
} from "/src/ui/provider-form";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

/**
 * One provider's page: its details, its credentials, its models, and its
 * deletion — each in its own card, each stating what it did where it did it.
 * The details card is the edit form: it opens prefilled, tracks dirt, and
 * saves the row's complete state back.
 */
export function ProviderDetailScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() =>
                props.api.listModels(
                    {
                        provider_id: props.providerId,
                    },
                    call.signal,
                ),
            ),
        ]);

        if (providersOutcome.status === "aborted" || modelsOutcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setProviders(providersOutcome.status === "ok" ? providersOutcome.result.providers : []);
        setModels(modelsOutcome.status === "ok" ? modelsOutcome.result.models : []);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        props.providerId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const provider = createMemo(
        () => providers().find((candidate) => candidate.id === props.providerId) ?? null,
    );

    const providersById = createMemo(
        () =>
            new Map<number, ProviderResponse>(
                providers().map((row) => [
                    row.id,
                    row,
                ]),
            ),
    );

    const reportFailure = (
        cause: unknown,
        action:
            | "update-provider"
            | "delete-provider"
            | "discover-models"
            | "start-oauth"
            | "reauthorize",
    ): void => {
        const diagnosis = describeCallFailure(cause, action);

        setNotice({
            tone: "bad",
            title: diagnosis.headline,
            detail: diagnosis.detail,
            fix: diagnosis.fix,
        });
    };

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={isReading() && !hasRead() && notice() === null}>
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && provider() === null}>
                <PageHeader
                    title="Provider"
                    backHref={hrefs.providers()}
                    backLabel="Providers"
                />
                <EmptyState
                    title="Provider not found"
                    description={`The gateway holds no provider #${props.providerId}. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back.`}
                >
                    <ButtonLink
                        href={hrefs.providers()}
                        tone="primary"
                    >
                        Back to providers
                    </ButtonLink>
                    <ButtonLink href={hrefs.providerNew()}>New provider</ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={provider()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={row.name}
                            description={`#${row.id} · ${providerTypeLabel(row.type)} · prefix ${row.model_prefix === "" ? "(empty)" : row.model_prefix}`}
                            backHref={hrefs.providers()}
                            backLabel="Providers"
                        >
                            <Badge
                                tone={row.is_enabled ? "ok" : "muted"}
                                label={row.is_enabled ? "Serving" : "Off"}
                            />
                            <Badge
                                tone={credentialBadge(row).tone}
                                label={credentialBadge(row).label}
                            />
                        </PageHeader>

                        <Show when={notice()}>
                            {(current) => (
                                <Notice
                                    tone={current().tone}
                                    title={current().title}
                                    detail={current().detail}
                                    fix={current().fix}
                                />
                            )}
                        </Show>

                        <DetailsCard
                            api={props.api}
                            provider={row}
                            onReload={() => void read()}
                            onNotice={setNotice}
                            onFailure={reportFailure}
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
                                    onReload={() => void read()}
                                    onNotice={setNotice}
                                    onFailure={reportFailure}
                                />
                            )}
                        </Show>

                        <ModelsCard
                            api={props.api}
                            provider={row}
                            models={models()}
                            providersById={providersById()}
                            onReload={() => void read()}
                            onNotice={setNotice}
                            onFailure={reportFailure}
                        />

                        <DangerCard
                            api={props.api}
                            provider={row}
                            modelCount={models().length}
                            onFailure={reportFailure}
                        />
                    </>
                )}
            </Show>
        </div>
    );
}

/** The stored base URL of a static-key provider row, or empty for OAuth logins. */
function staticBaseUrlOf(provider: ProviderResponse): string {
    switch (provider.type) {
        case "openai_response":
        case "openai_chat_completion":
        case "gemini_generate_content":
            return provider.base_url;

        case "codex_oauth":
        case "antigravity_oauth":
            return "";
    }
}

/** The provider's editable state. Saving writes the complete readable state. */
function DetailsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly onReload: () => void;
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "update-provider") => void;
}): JSX.Element {
    const [name, setName] = createSignal(props.provider.name);
    const [modelPrefix, setModelPrefix] = createSignal(props.provider.model_prefix);
    const [isEnabled, setIsEnabled] = createSignal(props.provider.is_enabled);
    const [baseUrl, setBaseUrl] = createSignal(staticBaseUrlOf(props.provider));
    const [apiKey, setApiKey] = createSignal("");
    const [isSaving, setIsSaving] = createSignal(false);

    const isStatic = (): boolean => findOAuthProviderType(props.provider.type) === null;

    const isDirty = createMemo(
        () =>
            name() !== props.provider.name ||
            modelPrefix() !== props.provider.model_prefix ||
            isEnabled() !== props.provider.is_enabled ||
            (isStatic() && (baseUrl() !== staticBaseUrlOf(props.provider) || apiKey() !== "")),
    );

    const revert = (): void => {
        setName(props.provider.name);
        setModelPrefix(props.provider.model_prefix);
        setIsEnabled(props.provider.is_enabled);
        setBaseUrl(staticBaseUrlOf(props.provider));
        setApiKey("");
    };

    const save = async (): Promise<void> => {
        if (isSaving()) {
            return;
        }

        setIsSaving(true);

        const outcome = await performManagementCall(() =>
            props.api.updateProvider(
                toUpdateProviderRequest(props.provider, {
                    name: name(),
                    model_prefix: modelPrefix(),
                    is_enabled: isEnabled(),
                    ...(isStatic()
                        ? {
                              base_url: baseUrl(),
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
            props.onFailure(outcome.cause, "update-provider");
            return;
        }

        const prefixChanged = modelPrefix() !== props.provider.model_prefix;
        setApiKey("");
        props.onNotice({
            tone: "ok",
            title: `${name()} saved.`,
            detail: prefixChanged
                ? "The new prefix recomposes the public ID of every model without an override; the list below was read again."
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
            <CardHeader
                title="Details"
                description="Everything the gateway stores about this provider, except its credentials."
            />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={submit}
            >
                <ProviderIdentityFields
                    name={name()}
                    modelPrefix={modelPrefix()}
                    onNameInput={setName}
                    onModelPrefixInput={setModelPrefix}
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
                        isDisabled={isSaving()}
                    />
                </Show>

                <ProviderServingSwitch
                    isEnabled={isEnabled()}
                    onToggle={setIsEnabled}
                    isDisabled={isSaving()}
                />

                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        type="submit"
                        tone="primary"
                        isDisabled={!isDirty()}
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
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "start-oauth" | "reauthorize") => void;
}): JSX.Element {
    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const expiryLine = (): string => {
        if (!isOAuthProviderRow(props.provider) || props.provider.expires_at === null) {
            return "No expiry is stored for this provider.";
        }

        const validity = describeValidity(props.provider.expires_at, Math.floor(Date.now() / 1000));

        return validity.isExpired
            ? `Expired ${validity.text.replace(/^expired /, "")} ago.`
            : `Valid for ${validity.text.replace(/^in /, "")}.`;
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
            props.onFailure(outcome.cause, "start-oauth");
            return;
        }

        setFlow(outcome.result);
    };

    const complete = async (): Promise<void> => {
        const startedFlow = flow();

        if (startedFlow === null || isCompleting()) {
            return;
        }

        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.reauthorizeProvider({
                state: startedFlow.state,
                code: code().trim(),
                provider_id: props.provider.id,
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, "reauthorize");

            if (diagnosis.kind === "gone") {
                // The state is consumed; the flow resets and the listing
                // re-reads instead of either being retried.
                setFlow(null);
                setCode("");
                props.onReload();
            }

            props.onFailure(outcome.cause, "reauthorize");
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
                description="OAuth credentials expire. Reconnecting replaces what the gateway stores; nothing else about the provider changes."
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
                                    label="Authorization code"
                                    value={code()}
                                    onInput={setCode}
                                    placeholder="Paste the code from the callback URL"
                                    isDisabled={isCompleting()}
                                    hint="Single-use, like the state it completes, and never stored."
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

/** The provider's models and the sync that refreshes them from upstream. */
function ModelsCard(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly onReload: () => void;
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "discover-models") => void;
}): JSX.Element {
    const [isSyncing, setIsSyncing] = createSignal(false);

    const sync = async (): Promise<void> => {
        if (isSyncing()) {
            return;
        }

        setIsSyncing(true);

        const outcome = await performManagementCall(() =>
            props.api.discoverModels({
                provider_id: props.provider.id,
            }),
        );

        setIsSyncing(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, "discover-models");
            return;
        }

        const discovered = outcome.result.models;

        props.onNotice(
            discovered.length === 0
                ? {
                      tone: "info",
                      title: "Upstream reports no models.",
                      detail: "Nothing changed — your curated list was kept.",
                  }
                : {
                      tone: "ok",
                      title: `${discovered.length} ${discovered.length === 1 ? "model" : "models"} synced from ${props.provider.name}.`,
                      detail: "Overrides on models that returned were kept; models the provider no longer reports are gone.",
                  },
        );
        props.onReload();
    };

    return (
        <Card>
            <CardHeader
                title={`Models · ${props.models.length}`}
                description="Synced from upstream, or added by hand. Open one to edit its overrides."
            >
                <ButtonLink
                    href={hrefs.modelNew(props.provider.id)}
                    glyph="plus"
                >
                    Add model
                </ButtonLink>
                <Button
                    glyph="refresh"
                    isBusy={isSyncing()}
                    onClick={() => void sync()}
                >
                    {isSyncing() ? "Syncing…" : "Sync models"}
                </Button>
            </CardHeader>
            <div class="mt-4">
                <Show
                    when={props.models.length > 0}
                    fallback={
                        <p class="rounded-lg bg-canvas px-3.5 py-3 text-[13.5px] text-ink-dim dark:bg-muted">
                            No models on this provider yet — sync to read what the provider reports,
                            or add one by hand.
                        </p>
                    }
                >
                    <ModelList
                        models={props.models}
                        providersById={props.providersById}
                    />
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
    readonly onFailure: (cause: unknown, action: "delete-provider") => void;
}): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);

    const remove = async (): Promise<void> => {
        if (isDeleting()) {
            return;
        }

        setIsDeleting(true);

        const outcome = await performManagementCall(() =>
            props.api.deleteProvider({
                provider_id: props.provider.id,
            }),
        );

        setIsDeleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, "delete-provider");
            setIsArmed(false);
            return;
        }

        navigate(hrefs.providers());
    };

    return (
        <Card class="border-bad/30">
            <CardHeader
                title="Delete this provider"
                description={`Deleting removes the provider and, by cascade, its ${props.modelCount} ${props.modelCount === 1 ? "model" : "models"}.`}
            />
            <Show
                when={isArmed()}
                fallback={
                    <div class="mt-4">
                        <Button
                            tone="danger"
                            glyph="trash"
                            onClick={() => setIsArmed(true)}
                        >
                            Delete…
                        </Button>
                    </div>
                }
            >
                <div class="mt-4 flex flex-wrap items-center gap-2">
                    <p class="w-full text-[13.5px] text-ink-dim">
                        {`Delete ${props.provider.name}? Clients calling those public IDs will start getting 404s.`}
                    </p>
                    <Button
                        tone="danger"
                        isBusy={isDeleting()}
                        onClick={() => void remove()}
                    >
                        {isDeleting() ? "Deleting…" : "Delete for good"}
                    </Button>
                    <Button
                        isDisabled={isDeleting()}
                        onClick={() => setIsArmed(false)}
                    >
                        Keep it
                    </Button>
                </div>
            </Show>
        </Card>
    );
}
