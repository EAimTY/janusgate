import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
import { oauthFlowForProviderType, UPSTREAM_PROTOCOLS } from "/common/domain";
import {
    MODEL_PREFIX_MAX_LENGTH,
    MODEL_PREFIX_PATTERN,
    PROVIDER_NAME_MAX_LENGTH,
    PROVIDER_NAME_PATTERN,
} from "/config";
import {
    type CompatibleProtocol,
    errorMessage,
    type OAuthCompletionInput,
    type OAuthType,
    type PanelApiClient,
    type Provider,
    type ProviderType,
} from "./api-client";
import { panelConfig } from "./config";
import { protocolLabel, providerTypeLabel } from "./labels";
import {
    buttonGhostClasses,
    buttonPrimaryClasses,
    buttonSecondaryClasses,
    checkboxClasses,
    Dialog,
    dataTableClasses,
    formControlClasses,
    LoadingBlock,
    Notice,
    PageHeader,
    PanelCard,
    pageShellClasses,
    StatusBadge,
    tableActionClasses,
    tableActionDangerClasses,
} from "./ui";

interface ProvidersPageProps {
    client: PanelApiClient;
}

interface OAuthFlow {
    type: OAuthType;
    providerName: string | null;
    existingProviderNames: ReadonlySet<string>;
    authorizeUrl: string | null;
}

const providerGroups: ReadonlyArray<{
    type: ProviderType;
    label: string;
    description: string;
}> = [
    {
        type: "openai_oauth",
        label: "OpenAI accounts",
        description: "ChatGPT Codex backend through an OpenAI OAuth account.",
    },
    {
        type: "antigravity_oauth",
        label: "Antigravity accounts",
        description: "Google Code Assist through an Antigravity OAuth account.",
    },
    {
        type: "openai_compatible",
        label: "API-key providers",
        description:
            "Static credentials for an OpenAI-compatible or Gemini GenerateContent endpoint.",
    },
];

/**
 * OAuth access tokens are refreshed lazily with a safety margin on each gateway request, so an
 * expired `expires_at` only means the token has been idle — it does not make the provider
 * unusable. Only a rejected refresh (revoked grant) requires reauthentication.
 */
function expiryPresentation(provider: Provider): {
    label: string;
    tone: "good" | "neutral" | "warning";
} {
    if (provider.expiresAt === null)
        return {
            label: "Static credential",
            tone: "neutral",
        };
    const secondsRemaining = provider.expiresAt - Math.floor(Date.now() / 1000);
    if (secondsRemaining <= 0)
        return {
            label: "Idle — refreshes automatically on use",
            tone: "neutral",
        };
    if (secondsRemaining <= panelConfig.tokenExpiryWarningSeconds) {
        const hours = Math.max(1, Math.ceil(secondsRemaining / 3600));
        return {
            label: `Expires in ${hours}h, then auto-refreshes`,
            tone: "warning",
        };
    }
    return {
        label: new Date(provider.expiresAt * 1000).toLocaleString(),
        tone: "good",
    };
}

const expiryToneClasses: Record<"good" | "neutral" | "warning", string> = {
    good: "text-[#19784e]",
    neutral: "text-[#66726b]",
    warning: "text-[#9b6818]",
};

export function ProvidersPage(props: ProvidersPageProps): JSX.Element {
    const [providers, setProviders] = createSignal<Provider[]>([]);
    const [isLoading, setIsLoading] = createSignal(true);
    const [error, setError] = createSignal<string | null>(null);
    const [success, setSuccess] = createSignal<string | null>(null);
    const [busyProviderName, setBusyProviderName] = createSignal<string | null>(null);

    const [isEditorOpen, setIsEditorOpen] = createSignal(false);
    const [editingProvider, setEditingProvider] = createSignal<Provider | null>(null);
    const [editorName, setEditorName] = createSignal("");
    const [editorPrefix, setEditorPrefix] = createSignal("");
    const [editorBaseUrl, setEditorBaseUrl] = createSignal("");
    const [editorProtocol, setEditorProtocol] = createSignal<CompatibleProtocol>("openai_response");
    const [editorApiKey, setEditorApiKey] = createSignal("");
    const [isEditorEnabled, setIsEditorEnabled] = createSignal(true);
    const [isSavingEditor, setIsSavingEditor] = createSignal(false);
    const [editorError, setEditorError] = createSignal<string | null>(null);

    const [oauthFlow, setOAuthFlow] = createSignal<OAuthFlow | null>(null);
    const [oauthRedirectUrl, setOAuthRedirectUrl] = createSignal("");
    const [oauthName, setOAuthName] = createSignal("");
    const [oauthPrefix, setOAuthPrefix] = createSignal("");
    const [oauthProjectId, setOAuthProjectId] = createSignal("");
    const [isOAuthPrefixSpecified, setIsOAuthPrefixSpecified] = createSignal(false);
    const [oauthError, setOAuthError] = createSignal<string | null>(null);
    const [isStartingOAuth, setIsStartingOAuth] = createSignal(false);
    const [isCompletingOAuth, setIsCompletingOAuth] = createSignal(false);

    const providerCount = createMemo(() => providers().length);

    async function loadProviders(): Promise<Provider[]> {
        const loadedProviders = await props.client.listProviders();
        setProviders(loadedProviders);
        return loadedProviders;
    }

    async function initialLoad(): Promise<void> {
        setIsLoading(true);
        setError(null);
        try {
            await loadProviders();
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setIsLoading(false);
        }
    }

    onSettled(() => {
        void initialLoad();
    });

    function openCompatibleCreator(): void {
        setEditingProvider(null);
        setEditorName("");
        setEditorPrefix("");
        setEditorBaseUrl("");
        setEditorProtocol("openai_response");
        setEditorApiKey("");
        setIsEditorEnabled(true);
        setEditorError(null);
        setIsEditorOpen(true);
    }

    function openProviderEditor(provider: Provider): void {
        setEditingProvider(provider);
        setEditorName(provider.name);
        setEditorPrefix(provider.modelPrefix);
        setEditorBaseUrl(provider.baseUrl ?? "");
        setEditorProtocol(provider.protocol ?? "openai_response");
        setEditorApiKey("");
        setIsEditorEnabled(provider.enabled);
        setEditorError(null);
        setIsEditorOpen(true);
    }

    async function discover(providerName: string): Promise<void> {
        setBusyProviderName(providerName);
        setError(null);
        setSuccess(null);
        try {
            const result = await props.client.discoverModels(providerName);
            setSuccess(
                `Discovery for ${providerName}: ${result.created} created, ${result.removed} removed, ${result.skipped} skipped (${result.total} upstream total).`,
            );
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyProviderName(null);
        }
    }

    async function saveEditor(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        const currentProvider = editingProvider();
        const isCreation = currentProvider === null;
        setIsSavingEditor(true);
        setEditorError(null);
        try {
            let savedName: string;
            if (currentProvider === null || currentProvider.type === "openai_compatible") {
                const input = {
                    name: editorName().trim(),
                    protocol: editorProtocol(),
                    baseUrl: editorBaseUrl().trim(),
                    enabled: isEditorEnabled(),
                    modelPrefix: editorPrefix(),
                    ...(isCreation
                        ? {
                              type: "openai_compatible" as const,
                          }
                        : {}),
                    ...(editorApiKey().length > 0
                        ? {
                              apiKey: editorApiKey(),
                          }
                        : {}),
                };
                savedName = await props.client.saveCompatibleProvider(input);
            } else {
                savedName = await props.client.saveOAuthProvider({
                    name: currentProvider.name,
                    enabled: isEditorEnabled(),
                    modelPrefix: editorPrefix(),
                });
            }
            setIsEditorOpen(false);
            await loadProviders();
            setSuccess(`${savedName} saved.`);
            if (isCreation) await discover(savedName);
        } catch (caughtError) {
            setEditorError(errorMessage(caughtError));
        } finally {
            setIsSavingEditor(false);
        }
    }

    async function toggleProvider(provider: Provider): Promise<void> {
        setBusyProviderName(provider.name);
        setError(null);
        setSuccess(null);
        try {
            if (provider.type === "openai_compatible") {
                await props.client.saveCompatibleProvider({
                    name: provider.name,
                    protocol: provider.protocol,
                    baseUrl: provider.baseUrl,
                    enabled: !provider.enabled,
                    modelPrefix: provider.modelPrefix,
                });
            } else {
                await props.client.saveOAuthProvider({
                    name: provider.name,
                    enabled: !provider.enabled,
                    modelPrefix: provider.modelPrefix,
                });
            }
            await loadProviders();
            setSuccess(`${provider.name} ${provider.enabled ? "disabled" : "enabled"}.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyProviderName(null);
        }
    }

    async function deleteProvider(provider: Provider): Promise<void> {
        if (
            !window.confirm(
                `Delete provider “${provider.name}” and all of its models? This cannot be undone.`,
            )
        )
            return;
        setBusyProviderName(provider.name);
        setError(null);
        setSuccess(null);
        try {
            await props.client.deleteProvider(provider.name);
            await loadProviders();
            setSuccess(`${provider.name} deleted.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyProviderName(null);
        }
    }

    async function launchOAuth(type: OAuthType, providerName: string | null): Promise<void> {
        const popup = window.open("about:blank", "_blank");
        if (popup !== null) popup.opener = null;
        const existingProviderNames = new Set(providers().map((provider) => provider.name));
        setOAuthRedirectUrl("");
        setOAuthName("");
        setOAuthPrefix("");
        setOAuthProjectId("");
        setIsOAuthPrefixSpecified(false);
        setOAuthError(null);
        setOAuthFlow({
            type,
            providerName,
            existingProviderNames,
            authorizeUrl: null,
        });
        setIsStartingOAuth(true);
        try {
            const authorizeUrl = await props.client.startProviderOAuth(
                type,
                providerName ?? undefined,
            );
            setOAuthFlow({
                type,
                providerName,
                existingProviderNames,
                authorizeUrl,
            });
            if (popup !== null) popup.location.assign(authorizeUrl);
        } catch (caughtError) {
            popup?.close();
            setOAuthError(errorMessage(caughtError));
        } finally {
            setIsStartingOAuth(false);
        }
    }

    async function completeOAuth(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        const flow = oauthFlow();
        if (flow === null) return;
        setIsCompletingOAuth(true);
        setOAuthError(null);
        try {
            const input: OAuthCompletionInput = {
                redirectUrl: oauthRedirectUrl().trim(),
            };
            if (flow.providerName === null) {
                if (oauthName().trim().length > 0) input.name = oauthName().trim();
                if (isOAuthPrefixSpecified()) input.modelPrefix = oauthPrefix();
            }
            if (flow.type === "antigravity" && oauthProjectId().trim().length > 0) {
                input.projectId = oauthProjectId().trim();
            }
            await props.client.completeProviderOAuth(input);
            const refreshedProviders = await loadProviders();
            const discoveryTarget =
                flow.providerName ??
                refreshedProviders.find(
                    (provider) =>
                        oauthFlowForProviderType(provider.type) === flow.type &&
                        !flow.existingProviderNames.has(provider.name),
                )?.name;
            setOAuthFlow(null);
            setSuccess(
                flow.providerName === null
                    ? "OAuth provider connected."
                    : `${flow.providerName} reauthenticated.`,
            );
            if (discoveryTarget !== undefined) await discover(discoveryTarget);
            else if (flow.providerName === null) {
                setError(
                    "Provider connected, but its generated name could not be identified for discovery.",
                );
            }
        } catch (caughtError) {
            setOAuthError(errorMessage(caughtError));
        } finally {
            setIsCompletingOAuth(false);
        }
    }

    return (
        <section class={pageShellClasses}>
            <PageHeader
                eyebrow="Upstream connections"
                title="Providers"
                description="Manage credentials, protocols, model prefixes, and account availability."
                actions={
                    <div class="flex flex-wrap justify-end gap-[9px] max-[820px]:justify-start max-[540px]:w-full max-[540px]:[&>button]:w-full">
                        <button
                            class={buttonSecondaryClasses}
                            type="button"
                            onClick={() => void launchOAuth("openai", null)}
                        >
                            + OpenAI
                        </button>
                        <button
                            class={buttonSecondaryClasses}
                            type="button"
                            onClick={() => void launchOAuth("antigravity", null)}
                        >
                            + Antigravity
                        </button>
                        <button
                            class={buttonPrimaryClasses}
                            type="button"
                            onClick={openCompatibleCreator}
                        >
                            + API-key provider
                        </button>
                    </div>
                }
            />

            <div class="mb-[25px] flex items-center gap-2.5 rounded-xl border border-[#dbe2dc] bg-[rgb(255_255_255_/_66%)] px-[17px] py-[13px] text-[12px] text-[#66726b] max-[540px]:flex-wrap max-[540px]:items-start">
                <span class="text-[18px] font-extrabold text-[#18211d]">{providerCount()}</span>
                <span>configured providers</span>
                <span class="mx-[5px] h-[17px] w-px bg-[#dbe2dc] max-[540px]:hidden" />
                <span>Names are immutable routing identifiers.</span>
            </div>

            <Show when={error() !== null}>
                <Notice kind="error">{error()}</Notice>
            </Show>
            <Show when={success() !== null}>
                <Notice kind="success">{success()}</Notice>
            </Show>

            <Show
                when={!isLoading()}
                fallback={<LoadingBlock />}
            >
                <For each={providerGroups}>
                    {(group) => {
                        const groupProviders = createMemo(() =>
                            providers().filter((provider) => provider.type === group.type),
                        );
                        return (
                            <PanelCard
                                title={group.label}
                                description={group.description}
                                count={groupProviders().length}
                            >
                                <Show
                                    when={groupProviders().length > 0}
                                    fallback={
                                        <div class="p-[23px] text-center text-[12px] text-[#89938d]">
                                            No providers in this group.
                                        </div>
                                    }
                                >
                                    <div class="w-full overflow-x-auto">
                                        <table class={dataTableClasses}>
                                            <thead>
                                                <tr>
                                                    <th>Name</th>
                                                    <th>Protocol / endpoint</th>
                                                    <th>Model prefix</th>
                                                    <th>Credential</th>
                                                    <th>Status</th>
                                                    <th>Actions</th>
                                                </tr>
                                            </thead>
                                            <tbody>
                                                <For each={groupProviders()}>
                                                    {(provider) => {
                                                        const expiry = expiryPresentation(provider);
                                                        const oauthType = oauthFlowForProviderType(
                                                            provider.type,
                                                        );
                                                        const isBusy =
                                                            busyProviderName() === provider.name;
                                                        return (
                                                            <tr>
                                                                <td>
                                                                    <strong class="font-[730] text-[#202b25]">
                                                                        {provider.name}
                                                                    </strong>
                                                                    <small class="mt-1 block max-w-[270px] overflow-hidden text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                                        {providerTypeLabel(
                                                                            provider.type,
                                                                        )}
                                                                    </small>
                                                                </td>
                                                                <td>
                                                                    <span>
                                                                        {protocolLabel(
                                                                            provider.protocol,
                                                                        )}
                                                                    </span>
                                                                    <small class="mt-1 block max-w-[270px] overflow-hidden font-mono text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                                        {provider.baseUrl ??
                                                                            "Managed endpoint"}
                                                                    </small>
                                                                </td>
                                                                <td>
                                                                    <code class="rounded-[5px] bg-[#eff5f1] px-[5px] py-0.5 font-mono text-[11px] text-[#315a46] [overflow-wrap:anywhere]">
                                                                        {provider.modelPrefix
                                                                            .length > 0
                                                                            ? provider.modelPrefix
                                                                            : "(none)"}
                                                                    </code>
                                                                </td>
                                                                <td>
                                                                    <span
                                                                        class={`text-[11px] font-[650] ${expiryToneClasses[expiry.tone]}`}
                                                                    >
                                                                        {expiry.label}
                                                                    </span>
                                                                </td>
                                                                <td>
                                                                    <StatusBadge
                                                                        isEnabled={provider.enabled}
                                                                    />
                                                                </td>
                                                                <td>
                                                                    <div class="flex flex-wrap justify-end gap-x-2.5 gap-y-1">
                                                                        <button
                                                                            type="button"
                                                                            class={
                                                                                tableActionClasses
                                                                            }
                                                                            disabled={isBusy}
                                                                            onClick={() =>
                                                                                openProviderEditor(
                                                                                    provider,
                                                                                )
                                                                            }
                                                                        >
                                                                            Edit
                                                                        </button>
                                                                        <button
                                                                            type="button"
                                                                            class={
                                                                                tableActionClasses
                                                                            }
                                                                            disabled={isBusy}
                                                                            onClick={() =>
                                                                                void toggleProvider(
                                                                                    provider,
                                                                                )
                                                                            }
                                                                        >
                                                                            {provider.enabled
                                                                                ? "Disable"
                                                                                : "Enable"}
                                                                        </button>
                                                                        <button
                                                                            type="button"
                                                                            class={
                                                                                tableActionClasses
                                                                            }
                                                                            disabled={isBusy}
                                                                            onClick={() =>
                                                                                void discover(
                                                                                    provider.name,
                                                                                )
                                                                            }
                                                                        >
                                                                            Discover
                                                                        </button>
                                                                        <Show when={oauthType}>
                                                                            {(type) => (
                                                                                <button
                                                                                    type="button"
                                                                                    class={
                                                                                        tableActionClasses
                                                                                    }
                                                                                    disabled={
                                                                                        isBusy
                                                                                    }
                                                                                    onClick={() =>
                                                                                        void launchOAuth(
                                                                                            type(),
                                                                                            provider.name,
                                                                                        )
                                                                                    }
                                                                                >
                                                                                    Reauthenticate
                                                                                </button>
                                                                            )}
                                                                        </Show>
                                                                        <button
                                                                            type="button"
                                                                            class={
                                                                                tableActionDangerClasses
                                                                            }
                                                                            disabled={isBusy}
                                                                            onClick={() =>
                                                                                void deleteProvider(
                                                                                    provider,
                                                                                )
                                                                            }
                                                                        >
                                                                            Delete
                                                                        </button>
                                                                    </div>
                                                                </td>
                                                            </tr>
                                                        );
                                                    }}
                                                </For>
                                            </tbody>
                                        </table>
                                    </div>
                                </Show>
                            </PanelCard>
                        );
                    }}
                </For>
            </Show>

            <Dialog
                isOpen={isEditorOpen()}
                isBusy={isSavingEditor()}
                title={editingProvider() === null ? "Add API-key provider" : "Edit provider"}
                description="Changing a prefix affects only models created or discovered later."
                onClose={() => setIsEditorOpen(false)}
            >
                <form
                    class="grid gap-[17px] px-6 pt-[22px] pb-6 [&>[role]]:mb-0 [&_label>small]:text-[10px] [&_label>small]:leading-[1.4] [&_label>small]:font-[450] [&_label>small]:text-[#7c8880]"
                    onSubmit={(event) => void saveEditor(event)}
                >
                    <Show when={editorError() !== null}>
                        <Notice kind="error">{editorError()}</Notice>
                    </Show>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Provider name
                        <input
                            class={formControlClasses}
                            required
                            pattern={PROVIDER_NAME_PATTERN.source}
                            maxlength={PROVIDER_NAME_MAX_LENGTH}
                            disabled={editingProvider() !== null}
                            value={editorName()}
                            onInput={(event) => setEditorName(event.currentTarget.value)}
                        />
                        <small>
                            1–{PROVIDER_NAME_MAX_LENGTH} lowercase letters, numbers, underscores, or
                            hyphens.
                        </small>
                    </label>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Model prefix
                        <input
                            class={formControlClasses}
                            maxlength={MODEL_PREFIX_MAX_LENGTH}
                            pattern={MODEL_PREFIX_PATTERN.source}
                            value={editorPrefix()}
                            onInput={(event) => setEditorPrefix(event.currentTarget.value)}
                        />
                        <small>
                            Verbatim prefix; include your own separator. Blank means none.
                        </small>
                    </label>
                    <Show
                        when={
                            editingProvider() === null ||
                            editingProvider()?.type === "openai_compatible"
                        }
                    >
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            Base URL
                            <input
                                class={formControlClasses}
                                required
                                type="url"
                                placeholder="https://api.example.com/v1"
                                value={editorBaseUrl()}
                                onInput={(event) => setEditorBaseUrl(event.currentTarget.value)}
                            />
                        </label>
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            Protocol
                            <select
                                class={formControlClasses}
                                value={editorProtocol()}
                                onChange={(event) => {
                                    const value = event.currentTarget.value;
                                    for (const protocol of UPSTREAM_PROTOCOLS) {
                                        if (value === protocol) setEditorProtocol(protocol);
                                    }
                                }}
                            >
                                <option value="openai_response">Responses</option>
                                <option value="openai_chat_completion">Chat Completions</option>
                                <option value="gemini_generate_content">
                                    Gemini GenerateContent
                                </option>
                            </select>
                            <small>
                                Gemini endpoints use the x-goog-api-key header; the others use a
                                bearer token.
                            </small>
                        </label>
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            API key {editingProvider() === null ? "" : "(leave blank to preserve)"}
                            <input
                                class={formControlClasses}
                                required={editingProvider() === null}
                                type="password"
                                autocomplete="off"
                                value={editorApiKey()}
                                onInput={(event) => setEditorApiKey(event.currentTarget.value)}
                            />
                        </label>
                    </Show>
                    <label class="flex items-center gap-[7px] text-xs font-[720] text-[#344038]">
                        <input
                            class={checkboxClasses}
                            type="checkbox"
                            checked={isEditorEnabled()}
                            onChange={(event) => setIsEditorEnabled(event.currentTarget.checked)}
                        />
                        Enabled
                    </label>
                    <footer class="flex items-center justify-end gap-[9px] pt-[3px] max-[540px]:flex-col-reverse max-[540px]:items-stretch max-[540px]:[&>button]:w-full">
                        <button
                            class={buttonGhostClasses}
                            type="button"
                            disabled={isSavingEditor()}
                            onClick={() => setIsEditorOpen(false)}
                        >
                            Cancel
                        </button>
                        <button
                            class={buttonPrimaryClasses}
                            type="submit"
                            disabled={isSavingEditor()}
                        >
                            {isSavingEditor() ? "Saving…" : "Save provider"}
                        </button>
                    </footer>
                </form>
            </Dialog>

            <Dialog
                isOpen={oauthFlow() !== null}
                isBusy={isCompletingOAuth()}
                title={
                    oauthFlow()?.providerName === null ? "Connect OAuth provider" : "Reauthenticate"
                }
                description="Authorize in the new tab, then paste the full loopback redirect URL below."
                onClose={() => setOAuthFlow(null)}
            >
                <form
                    class="grid gap-[17px] px-6 pt-[22px] pb-6 [&>[role]]:mb-0 [&_label>small]:text-[10px] [&_label>small]:leading-[1.4] [&_label>small]:font-[450] [&_label>small]:text-[#7c8880]"
                    onSubmit={(event) => void completeOAuth(event)}
                >
                    <Show when={oauthError() !== null}>
                        <Notice kind="error">{oauthError()}</Notice>
                    </Show>
                    <Show when={oauthFlow()?.authorizeUrl !== null}>
                        <Notice kind="info">
                            Authorization opened in a new tab. If it was blocked, use the link
                            below.
                        </Notice>
                        <a
                            class="w-fit text-[12px] font-bold text-[#19784e] underline-offset-[3px]"
                            href={oauthFlow()?.authorizeUrl ?? undefined}
                            target="_blank"
                            rel="noreferrer"
                        >
                            Open authorization page ↗
                        </a>
                    </Show>
                    <Show when={oauthFlow()?.providerName === null}>
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            Provider name (optional)
                            <input
                                class={formControlClasses}
                                pattern={PROVIDER_NAME_PATTERN.source}
                                maxlength={PROVIDER_NAME_MAX_LENGTH}
                                placeholder="Auto-generated when blank"
                                value={oauthName()}
                                onInput={(event) => setOAuthName(event.currentTarget.value)}
                            />
                        </label>
                        <label class="flex items-center gap-[7px] text-xs font-[720] text-[#344038]">
                            <input
                                class={checkboxClasses}
                                type="checkbox"
                                checked={isOAuthPrefixSpecified()}
                                onChange={(event) =>
                                    setIsOAuthPrefixSpecified(event.currentTarget.checked)
                                }
                            />
                            Set a custom model prefix
                        </label>
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            Model prefix
                            <input
                                class={formControlClasses}
                                maxlength={MODEL_PREFIX_MAX_LENGTH}
                                pattern={MODEL_PREFIX_PATTERN.source}
                                disabled={!isOAuthPrefixSpecified()}
                                placeholder="Blank means no prefix"
                                value={oauthPrefix()}
                                onInput={(event) => setOAuthPrefix(event.currentTarget.value)}
                            />
                            <small>
                                Leave customization off for the provider-name default. Turn it on
                                and leave this blank to explicitly use no prefix.
                            </small>
                        </label>
                    </Show>
                    <Show when={oauthFlow()?.type === "antigravity"}>
                        <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                            Google Cloud project ID (optional)
                            <input
                                class={formControlClasses}
                                placeholder="Required only for Workspace/project-backed accounts"
                                value={oauthProjectId()}
                                onInput={(event) => setOAuthProjectId(event.currentTarget.value)}
                            />
                            <small>
                                Leave blank for Antigravity-managed free-tier projects. Workspace
                                accounts may require their own project ID.
                            </small>
                        </label>
                    </Show>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Full redirect URL
                        <textarea
                            class={`${formControlClasses} resize-y`}
                            required
                            rows={4}
                            spellcheck={false}
                            placeholder="http://localhost:…/auth/callback?code=…&state=…"
                            value={oauthRedirectUrl()}
                            onInput={(event) => setOAuthRedirectUrl(event.currentTarget.value)}
                        />
                    </label>
                    <footer class="flex items-center justify-end gap-[9px] pt-[3px] max-[540px]:flex-col-reverse max-[540px]:items-stretch max-[540px]:[&>button]:w-full">
                        <span class="mr-auto text-[10px] text-[#818c85] max-[540px]:m-0 max-[540px]:text-center">
                            {isStartingOAuth()
                                ? "Preparing authorization…"
                                : "OAuth state expires in 15 minutes."}
                        </span>
                        <button
                            class={buttonPrimaryClasses}
                            type="submit"
                            disabled={isStartingOAuth() || isCompletingOAuth()}
                        >
                            {isCompletingOAuth() ? "Completing…" : "Complete connection"}
                        </button>
                    </footer>
                </form>
            </Dialog>
        </section>
    );
}
