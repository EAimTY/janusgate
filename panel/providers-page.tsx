import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import {
    type OAuthFlowType,
    oauthFlowForProviderType,
    type ProviderSummary,
    type ProviderType,
} from "/common/domain";
import { unixSeconds } from "/common/validation";
import { MILLISECONDS_PER_SECOND } from "/config";
import { errorMessage, type PanelApiClient } from "./api-client";
import { panelConfig } from "./config";
import { protocolLabel, providerTypeLabel } from "./labels";
import { OAuthFlowDialog, type OAuthFlowLaunch } from "./oauth-flow-dialog";
import { createPageState } from "./page-state";
import { ProviderEditorDialog } from "./provider-editor-dialog";
import {
    buttonPrimaryClasses,
    buttonSecondaryClasses,
    dataTableClasses,
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
function expiryPresentation(provider: ProviderSummary): {
    label: string;
    tone: "good" | "neutral" | "warning";
} {
    if (provider.expiresAt === null)
        return {
            label: "Static credential",
            tone: "neutral",
        };
    const secondsRemaining = provider.expiresAt - unixSeconds();
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
        label: new Date(provider.expiresAt * MILLISECONDS_PER_SECOND).toLocaleString(),
        tone: "good",
    };
}

const expiryToneClasses: Record<"good" | "neutral" | "warning", string> = {
    good: "text-[#19784e]",
    neutral: "text-[#66726b]",
    warning: "text-[#9b6818]",
};

export function ProvidersPage(props: ProvidersPageProps): JSX.Element {
    const [providers, setProviders] = createSignal<ProviderSummary[]>([]);
    const [success, setSuccess] = createSignal<string | null>(null);
    const [busyProviderName, setBusyProviderName] = createSignal<string | null>(null);

    const [isEditorOpen, setIsEditorOpen] = createSignal(false);
    const [editingProvider, setEditingProvider] = createSignal<ProviderSummary | null>(null);

    const [oauthLaunch, setOAuthLaunch] = createSignal<OAuthFlowLaunch | null>(null);

    async function loadProviders(): Promise<ProviderSummary[]> {
        const loadedProviders = await props.client.listProviders();
        setProviders(loadedProviders);
        return loadedProviders;
    }

    const { isLoading, error, setError, retry } = createPageState(loadProviders);

    const providerCount = createMemo(() => providers().length);

    function openCompatibleCreator(): void {
        setEditingProvider(null);
        setIsEditorOpen(true);
    }

    function openProviderEditor(provider: ProviderSummary): void {
        setEditingProvider(provider);
        setIsEditorOpen(true);
    }

    function launchOAuth(type: OAuthFlowType, providerName: string | null): void {
        setOAuthLaunch({
            type,
            providerName,
            existingProviderNames: new Set(providers().map((provider) => provider.name)),
        });
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

    async function handleProviderSaved(providerName: string, isCreation: boolean): Promise<void> {
        // Closing the dialog unmounts it, so every failure from here on must be reported at
        // page level; the dialog's error state is unreachable past this point.
        setIsEditorOpen(false);
        try {
            await loadProviders();
            setSuccess(`${providerName} saved.`);
            if (isCreation) await discover(providerName);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        }
    }

    async function handleOAuthCompleted(flow: OAuthFlowLaunch): Promise<void> {
        // The provider row is already written and the single-use redirect URL is consumed,
        // so the dialog closes before any further work and every post-connection failure is
        // reported here at page level; `onCompleted` must never reject back into the
        // consumed flow (see the OAuthFlowDialogProps contract).
        setOAuthLaunch(null);
        setSuccess(
            flow.providerName === null
                ? "OAuth provider connected."
                : `${flow.providerName} reauthenticated.`,
        );
        let refreshedProviders: ProviderSummary[];
        try {
            refreshedProviders = await loadProviders();
        } catch (caughtError) {
            setError(errorMessage(caughtError));
            return;
        }
        const discoveryTarget =
            flow.providerName ??
            refreshedProviders.find(
                (provider) =>
                    oauthFlowForProviderType(provider.type) === flow.type &&
                    !flow.existingProviderNames.has(provider.name),
            )?.name;
        if (discoveryTarget !== undefined) await discover(discoveryTarget);
        else if (flow.providerName === null) {
            setError(
                "Provider connected, but its generated name could not be identified for discovery.",
            );
        }
    }

    async function toggleProvider(provider: ProviderSummary): Promise<void> {
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

    async function deleteProvider(provider: ProviderSummary): Promise<void> {
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
                            onClick={() => launchOAuth("openai", null)}
                        >
                            + OpenAI
                        </button>
                        <button
                            class={buttonSecondaryClasses}
                            type="button"
                            onClick={() => launchOAuth("antigravity", null)}
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
                <Notice kind="error">
                    <span class="flex flex-wrap items-center justify-between gap-x-2.5 gap-y-1">
                        {error()}
                        <button
                            class={tableActionDangerClasses}
                            type="button"
                            onClick={retry}
                        >
                            Retry
                        </button>
                    </span>
                </Notice>
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
                                                                            disabled={
                                                                                busyProviderName() ===
                                                                                provider.name
                                                                            }
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
                                                                            disabled={
                                                                                busyProviderName() ===
                                                                                provider.name
                                                                            }
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
                                                                            disabled={
                                                                                busyProviderName() ===
                                                                                provider.name
                                                                            }
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
                                                                                        busyProviderName() ===
                                                                                        provider.name
                                                                                    }
                                                                                    onClick={() =>
                                                                                        launchOAuth(
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
                                                                            disabled={
                                                                                busyProviderName() ===
                                                                                provider.name
                                                                            }
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

            <Show when={isEditorOpen()}>
                <ProviderEditorDialog
                    client={props.client}
                    editingProvider={editingProvider()}
                    onClose={() => setIsEditorOpen(false)}
                    onSaved={handleProviderSaved}
                />
            </Show>
            <Show
                when={oauthLaunch()}
                keyed
            >
                {(flow) => (
                    <OAuthFlowDialog
                        client={props.client}
                        launch={flow}
                        onClose={() => setOAuthLaunch(null)}
                        onCompleted={handleOAuthCompleted}
                    />
                )}
            </Show>
        </section>
    );
}
