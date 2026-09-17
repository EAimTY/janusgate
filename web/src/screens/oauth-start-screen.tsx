import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import {
    ErrorCode,
    type OAuthProviderType,
    type ProviderResponse,
    type StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import {
    findOAuthProviderType,
    OAUTH_PROVIDER_TYPE_OPTIONS,
    providerTypeLabel,
} from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { createAsyncResource } from "/src/screens/resource";
import { Banner } from "/src/ui/banner";
import { Button, buttonClass } from "/src/ui/button";
import { Card } from "/src/ui/card";
import { SelectField, TextAreaField, TextField } from "/src/ui/field";
import { LinkButton } from "/src/ui/link-button";
import { PageHeader } from "/src/ui/page-header";

export type OauthStartScreenProps = {
    readonly api: ManagementApi;
    readonly reauthorizeProviderId: number | null;
};

/**
 * The OAuth paste-back flow (README §4.4, §7.2): `oauth:start` yields the
 * authorize URL and the single-use state, the administrator authorizes in a
 * new tab, and pasting the returned code completes the flow. Without a
 * provider the flow creates a provider; with one it replaces that provider's
 * credentials.
 */
export function OauthStartScreen(props: OauthStartScreenProps): JSX.Element {
    return (
        <Show
            when={props.reauthorizeProviderId}
            fallback={<CreateProviderOauthScreen api={props.api} />}
        >
            {(providerId) => (
                <ReauthorizeProviderOauthScreen
                    api={props.api}
                    providerId={providerId()}
                />
            )}
        </Show>
    );
}

function AuthorizeStep(props: {
    readonly flow: StartProviderOauthResponse;
    readonly onStartOver: () => void;
    readonly children: JSX.Element;
}): JSX.Element {
    return (
        <div class="flex flex-col gap-5">
            <div class="flex flex-col items-start gap-3 rounded-md border border-slate-200 bg-slate-50 px-4 py-3">
                <p class="text-sm text-slate-700">
                    Open the authorization page, approve access, then paste the code it returns.
                </p>
                <a
                    href={props.flow.authorize_url}
                    target="_blank"
                    rel="noreferrer noopener"
                    class={buttonClass("secondary")}
                >
                    Open authorization page
                </a>
                <p class="text-xs text-slate-500">
                    This flow expires at {formatTimestamp(props.flow.expires_at)}; its state is
                    single-use, so a failed exchange needs a fresh flow. Authorization codes are
                    single-use too and die early: paste the code and complete promptly after
                    approving access.
                </p>
            </div>

            {props.children}

            <div>
                <Button
                    variant="secondary"
                    onClick={props.onStartOver}
                >
                    Start over
                </Button>
            </div>
        </div>
    );
}

function CreateProviderOauthScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [providerType, setProviderType] = createSignal(
        OAUTH_PROVIDER_TYPE_OPTIONS[0]?.value ?? "openai_oauth",
    );

    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [name, setName] = createSignal("");
    const [modelPrefix, setModelPrefix] = createSignal("");
    const [codeError, setCodeError] = createSignal<string | null>(null);
    const [nameError, setNameError] = createSignal<string | null>(null);
    const [failure, setFailure] = createSignal<string | null>(null);
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const start = async (): Promise<void> => {
        const selectedType = findOAuthProviderType(providerType());

        if (selectedType === null) {
            setFailure("Choose an account type to authorize.");
            return;
        }

        setFailure(null);
        setIsStarting(true);

        const outcome = await performManagementCall(() =>
            props.api.startProviderOauth({
                type: selectedType,
            }),
        );

        setIsStarting(false);

        if (outcome.status === "ok") {
            setFlow(outcome.result);
            return;
        }

        if (outcome.status === "failed") {
            setFailure(outcome.message);
        }
    };

    const complete = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();
        const currentFlow = flow();

        if (currentFlow === null || isCompleting()) {
            return;
        }

        const codeFailure = code().trim() === "" ? "Paste the authorization code." : null;
        const nameFailure = name().trim() === "" ? "Enter a provider name." : null;
        setCodeError(codeFailure);
        setNameError(nameFailure);
        setFailure(null);

        if (codeFailure !== null || nameFailure !== null) {
            return;
        }

        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.completeProviderOauth({
                state: currentFlow.state,
                code: code().trim(),
                name: name(),
                model_prefix: modelPrefix(),
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "ok") {
            navigate(hrefs.providerDetail(outcome.result.provider.id));
            return;
        }

        if (outcome.status === "failed") {
            setFailure(outcome.message);

            if (outcome.errno === ErrorCode.NotFound) {
                // Unknown, expired, or already-consumed: this flow is dead.
                setFlow(null);
                setCode("");
            }
        }
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Connect an OAuth account"
                description="The gateway runs the authorization and mints the provider's credentials; the app only relays the code."
            />

            <Show when={failure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <Card title="Authorization">
                <Show
                    when={flow()}
                    fallback={
                        <div class="flex max-w-xl flex-col gap-4">
                            <SelectField
                                id="oauth-provider-type"
                                label="Account type"
                                value={providerType()}
                                options={OAUTH_PROVIDER_TYPE_OPTIONS}
                                onChange={setProviderType}
                                disabled={isStarting()}
                                hint="The kind of account this flow authorizes; it becomes the provider's type."
                            />
                            <div>
                                <Button
                                    disabled={isStarting()}
                                    onClick={() => void start()}
                                >
                                    {isStarting() ? "Starting…" : "Start authorization"}
                                </Button>
                            </div>
                        </div>
                    }
                >
                    {(currentFlow) => (
                        <form
                            novalidate={true}
                            class="flex max-w-xl flex-col gap-4"
                            onSubmit={complete}
                        >
                            <AuthorizeStep
                                flow={currentFlow()}
                                onStartOver={() => {
                                    setFlow(null);
                                    setCode("");
                                    setFailure(null);
                                }}
                            >
                                <TextAreaField
                                    id="oauth-code"
                                    label="Authorization code"
                                    value={code()}
                                    onInput={setCode}
                                    required
                                    disabled={isCompleting()}
                                    error={codeError()}
                                    isMonospace
                                    placeholder="Paste the code from the callback URL"
                                    hint="Codes are single-use and are never stored."
                                />
                                <TextField
                                    id="oauth-provider-name"
                                    label="Provider name"
                                    value={name()}
                                    onInput={setName}
                                    required
                                    disabled={isCompleting()}
                                    error={nameError()}
                                    hint="Unique, renameable label for the provider being created."
                                />
                                <TextField
                                    id="oauth-provider-model-prefix"
                                    label="Model prefix"
                                    value={modelPrefix()}
                                    onInput={setModelPrefix}
                                    disabled={isCompleting()}
                                    isMonospace
                                    hint="Prepended to every public model name; may be empty."
                                />
                                <div>
                                    <Button
                                        type="submit"
                                        disabled={isCompleting()}
                                    >
                                        {isCompleting() ? "Completing…" : "Complete authorization"}
                                    </Button>
                                </div>
                            </AuthorizeStep>
                        </form>
                    )}
                </Show>
            </Card>
        </div>
    );
}

function ReauthorizeProviderOauthScreen(props: {
    readonly api: ManagementApi;
    readonly providerId: number;
}): JSX.Element {
    const providers = createAsyncResource((signal) => props.api.listProviders(signal));
    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [codeError, setCodeError] = createSignal<string | null>(null);
    const [failure, setFailure] = createSignal<string | null>(null);
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const provider = createMemo(() => {
        const listing = providers.value();

        return listing === null
            ? null
            : (listing.providers.find((row) => row.id === props.providerId) ?? null);
    });

    const isOauthProvider = (row: ProviderResponse): boolean => row.type !== "openai_compatible";

    const start = async (accountType: OAuthProviderType): Promise<void> => {
        setFailure(null);
        setIsStarting(true);

        const outcome = await performManagementCall(() =>
            props.api.startProviderOauth({
                type: accountType,
            }),
        );

        setIsStarting(false);

        if (outcome.status === "ok") {
            setFlow(outcome.result);
            return;
        }

        if (outcome.status === "failed") {
            setFailure(outcome.message);
        }
    };

    const complete = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();
        const currentFlow = flow();

        if (currentFlow === null || isCompleting()) {
            return;
        }

        const codeFailure = code().trim() === "" ? "Paste the authorization code." : null;
        setCodeError(codeFailure);
        setFailure(null);

        if (codeFailure !== null) {
            return;
        }

        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.reauthorizeProvider({
                state: currentFlow.state,
                code: code().trim(),
                provider_id: props.providerId,
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "ok") {
            navigate(hrefs.providerDetail(props.providerId));
            return;
        }

        if (outcome.status === "failed") {
            setFailure(outcome.message);

            if (outcome.errno === ErrorCode.NotFound) {
                // The state is consumed, and the provider may be gone too.
                setFlow(null);
                setCode("");
                await providers.reload();
            }
        }
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Reauthorize provider"
                description="Runs the account's authorization again and replaces the stored credentials."
            />

            <Show when={failure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <Show when={providers.failure()}>
                {(message) => (
                    <Banner variant="error">
                        <p>{message()}</p>
                        <div class="mt-2">
                            <Button
                                variant="secondary"
                                onClick={() => void providers.reload()}
                            >
                                Retry
                            </Button>
                        </div>
                    </Banner>
                )}
            </Show>

            <Show when={providers.isLoading()}>
                <p
                    role="status"
                    class="text-sm text-slate-500"
                >
                    Loading provider…
                </p>
            </Show>

            <Show
                when={provider()}
                keyed
                fallback={
                    <Show when={providers.value() !== null}>
                        <Card title="Provider not found">
                            <div class="flex flex-col items-start gap-3">
                                <p class="text-sm text-slate-600">
                                    Provider #{props.providerId} is not in the listing any more, so
                                    there are no credentials to replace.
                                </p>
                                <LinkButton
                                    href={hrefs.providers()}
                                    variant="secondary"
                                >
                                    Back to providers
                                </LinkButton>
                            </div>
                        </Card>
                    </Show>
                }
            >
                {(row) => (
                    <Show
                        when={isOauthProvider(row)}
                        fallback={
                            <Card title="Not an OAuth provider">
                                <div class="flex flex-col items-start gap-3">
                                    <p class="text-sm text-slate-600">
                                        {row.name} is an {providerTypeLabel(row.type)} provider: its
                                        credentials are the static API key you entered, which
                                        `providers:update` replaces.
                                    </p>
                                    <LinkButton
                                        href={hrefs.providerDetail(row.id)}
                                        variant="secondary"
                                    >
                                        Open provider
                                    </LinkButton>
                                </div>
                            </Card>
                        }
                    >
                        <Card
                            title={`Authorize ${row.name}`}
                            description={`Account type: ${providerTypeLabel(row.type)}`}
                        >
                            <Show
                                when={flow()}
                                fallback={
                                    <div>
                                        <Button
                                            disabled={isStarting()}
                                            onClick={() => {
                                                const accountType = findOAuthProviderType(row.type);
                                                if (accountType !== null) {
                                                    void start(accountType);
                                                }
                                            }}
                                        >
                                            {isStarting() ? "Starting…" : "Start authorization"}
                                        </Button>
                                    </div>
                                }
                            >
                                {(currentFlow) => (
                                    <form
                                        novalidate={true}
                                        class="flex max-w-xl flex-col gap-4"
                                        onSubmit={complete}
                                    >
                                        <AuthorizeStep
                                            flow={currentFlow()}
                                            onStartOver={() => {
                                                setFlow(null);
                                                setCode("");
                                                setFailure(null);
                                            }}
                                        >
                                            <TextAreaField
                                                id="oauth-reauthorize-code"
                                                label="Authorization code"
                                                value={code()}
                                                onInput={setCode}
                                                required
                                                disabled={isCompleting()}
                                                error={codeError()}
                                                isMonospace
                                                placeholder="Paste the code from the callback URL"
                                                hint="Codes are single-use and are never stored."
                                            />
                                            <div>
                                                <Button
                                                    type="submit"
                                                    disabled={isCompleting()}
                                                >
                                                    {isCompleting()
                                                        ? "Replacing credentials…"
                                                        : "Replace credentials"}
                                                </Button>
                                            </div>
                                        </AuthorizeStep>
                                    </form>
                                )}
                            </Show>
                        </Card>
                    </Show>
                )}
            </Show>
        </div>
    );
}
