import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { formatTimestamp } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import { ErrorCode, type ProviderResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { providerTypeLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { createAsyncResource } from "/src/screens/resource";
import { AppLink } from "/src/ui/app-link";
import { Badge } from "/src/ui/badge";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { Card } from "/src/ui/card";
import { focusOnMount } from "/src/ui/focus-on-mount";
import { LinkButton } from "/src/ui/link-button";
import { PageHeader } from "/src/ui/page-header";

export type ProvidersScreenProps = {
    readonly api: ManagementApi;
};

/** How a provider's token expiry reads in a listing row. */
function providerExpiryText(provider: ProviderResponse): string {
    if (provider.type === "openai_compatible") {
        return "—";
    }
    return provider.expires_at === null ? "no expiry" : formatTimestamp(provider.expires_at);
}

/**
 * The provider listing (README §4.4 `providers:list`) with the two creation
 * paths — the `openai_compatible` form and the OAuth flow — and per-row
 * deletion.
 */
export function ProvidersScreen(props: ProvidersScreenProps): JSX.Element {
    const providers = createAsyncResource((signal) => props.api.listProviders(signal));
    const [actionFailure, setActionFailure] = createSignal<string | null>(null);
    const [pendingDeleteId, setPendingDeleteId] = createSignal<number | null>(null);
    const [deletingId, setDeletingId] = createSignal<number | null>(null);

    const rows = (): ProviderResponse[] | null => providers.value()?.providers ?? null;

    const deleteProvider = async (provider: ProviderResponse): Promise<void> => {
        setActionFailure(null);
        setDeletingId(provider.id);
        const outcome = await performManagementCall(() =>
            props.api.deleteProvider({
                provider_id: provider.id,
            }),
        );
        setDeletingId(null);
        setPendingDeleteId(null);
        if (outcome.status === "failed") {
            setActionFailure(outcome.message);
            if (outcome.errno === ErrorCode.NotFound) {
                // The row was already gone: the listing is the authority.
                await providers.reload();
            }
            return;
        }
        if (outcome.status === "ok") {
            await providers.reload();
        }
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Providers"
                description="Upstream accounts the gateway serves models from. Deleting a provider also deletes its models."
                actions={
                    <>
                        <LinkButton
                            href={hrefs.oauthStart(null)}
                            variant="secondary"
                        >
                            Connect OAuth account
                        </LinkButton>
                        <LinkButton href={hrefs.providerCreate()}>Create provider</LinkButton>
                    </>
                }
            />

            <Show when={actionFailure()}>
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
                    Loading providers…
                </p>
            </Show>

            <Show when={rows()}>
                {(providerRows) => (
                    <Show
                        when={providerRows().length > 0}
                        fallback={
                            <Card title="No providers yet">
                                <p class="text-sm text-slate-600">
                                    Create an OpenAI-compatible provider, or connect an OAuth
                                    account to have the gateway mint its credentials.
                                </p>
                            </Card>
                        }
                    >
                        <div class="overflow-x-auto rounded-lg border border-slate-200 bg-white shadow-sm">
                            <table class="w-full text-left text-sm">
                                <thead class="border-b border-slate-200 bg-slate-50 text-xs tracking-wide text-slate-500 uppercase">
                                    <tr>
                                        <th class="px-4 py-3 font-medium">Name</th>
                                        <th class="px-4 py-3 font-medium">Type</th>
                                        <th class="hidden px-4 py-3 font-medium sm:table-cell">
                                            Model prefix
                                        </th>
                                        <th class="px-4 py-3 font-medium">Status</th>
                                        <th class="hidden px-4 py-3 font-medium sm:table-cell">
                                            Token expiry
                                        </th>
                                        <th class="px-4 py-3" />
                                    </tr>
                                </thead>
                                <tbody>
                                    <For each={providerRows()}>
                                        {(provider) => (
                                            <tr class="border-b border-slate-100 last:border-b-0">
                                                <td class="px-4 py-3">
                                                    <AppLink
                                                        href={hrefs.providerDetail(provider.id)}
                                                        class="font-medium text-slate-900 hover:underline"
                                                    >
                                                        {provider.name}
                                                    </AppLink>
                                                    <span class="ml-2 font-mono text-xs text-slate-400">
                                                        #{provider.id}
                                                    </span>
                                                </td>
                                                <td class="px-4 py-3 text-slate-600">
                                                    {providerTypeLabel(provider.type)}
                                                </td>
                                                <td class="hidden px-4 py-3 font-mono text-xs text-slate-600 sm:table-cell">
                                                    {provider.model_prefix === ""
                                                        ? "—"
                                                        : provider.model_prefix}
                                                </td>
                                                <td class="px-4 py-3">
                                                    <Badge
                                                        tone={
                                                            provider.is_enabled
                                                                ? "positive"
                                                                : "neutral"
                                                        }
                                                    >
                                                        {provider.is_enabled
                                                            ? "Enabled"
                                                            : "Disabled"}
                                                    </Badge>
                                                </td>
                                                <td class="hidden px-4 py-3 text-slate-600 sm:table-cell">
                                                    {providerExpiryText(provider)}
                                                </td>
                                                <td class="px-4 py-3">
                                                    <div class="flex justify-end gap-2">
                                                        <Show
                                                            when={pendingDeleteId() === provider.id}
                                                            fallback={
                                                                <Button
                                                                    variant="secondary"
                                                                    onClick={() =>
                                                                        setPendingDeleteId(
                                                                            provider.id,
                                                                        )
                                                                    }
                                                                >
                                                                    Delete
                                                                </Button>
                                                            }
                                                        >
                                                            <Button
                                                                variant="danger"
                                                                ref={focusOnMount}
                                                                disabled={
                                                                    deletingId() === provider.id
                                                                }
                                                                onClick={() =>
                                                                    void deleteProvider(provider)
                                                                }
                                                            >
                                                                {deletingId() === provider.id
                                                                    ? "Deleting…"
                                                                    : "Confirm delete"}
                                                            </Button>
                                                            <Button
                                                                variant="secondary"
                                                                disabled={
                                                                    deletingId() === provider.id
                                                                }
                                                                onClick={() =>
                                                                    setPendingDeleteId(null)
                                                                }
                                                            >
                                                                Cancel
                                                            </Button>
                                                        </Show>
                                                    </div>
                                                </td>
                                            </tr>
                                        )}
                                    </For>
                                </tbody>
                            </table>
                        </div>
                    </Show>
                )}
            </Show>
        </div>
    );
}
