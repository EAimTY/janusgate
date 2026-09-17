import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { ModelList } from "/src/components/model-list";
import { formatTimestamp } from "/src/format";
import { type ProviderEdits, toUpdateProviderRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import {
    ErrorCode,
    type ModelResponse,
    type ProviderResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import {
    findUpstreamFormat,
    providerTypeLabel,
    UPSTREAM_FORMAT_OPTIONS,
} from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { createAsyncResource } from "/src/screens/resource";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { Card } from "/src/ui/card";
import { CheckboxField, SelectField, TextField } from "/src/ui/field";
import { focusOnMount } from "/src/ui/focus-on-mount";
import { LinkButton } from "/src/ui/link-button";
import { PageHeader } from "/src/ui/page-header";

export type ProviderDetailScreenProps = {
    readonly api: ManagementApi;
    readonly providerId: number;
};

function providerDescription(provider: ProviderResponse | null, providerId: number): string {
    if (provider === null) {
        return `Provider #${providerId}`;
    }

    return `${providerTypeLabel(provider.type)} · #${provider.id}`;
}

function discoveryNotice(modelCount: number): string {
    return `Discovered ${modelCount} ${modelCount === 1 ? "model" : "models"}.`;
}

/**
 * One provider's screen: its complete state as an editable form
 * (`providers:update` is a full-state write, README §4.5), its model rows with
 * `models:discover`, and deletion with its model cascade.
 */
export function ProviderDetailScreen(props: ProviderDetailScreenProps): JSX.Element {
    const providers = createAsyncResource((signal) => props.api.listProviders(signal));

    const models = createAsyncResource((signal) =>
        props.api.listModels(
            {
                provider_id: props.providerId,
            },
            signal,
        ),
    );

    const [actionFailure, setActionFailure] = createSignal<string | null>(null);
    const [notice, setNotice] = createSignal<string | null>(null);
    const [isDiscovering, setIsDiscovering] = createSignal(false);
    const [isConfirmingDelete, setIsConfirmingDelete] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);

    const provider = createMemo(() => {
        const listing = providers.value();

        return listing === null
            ? null
            : (listing.providers.find((row) => row.id === props.providerId) ?? null);
    });

    const modelRows = (): ModelResponse[] | null => models.value()?.models ?? null;

    const replaceModel = (updated: ModelResponse): void => {
        const current = models.value();

        if (current === null) {
            return;
        }

        models.setValue({
            models: current.models.map((row) =>
                row.provider_id === updated.provider_id && row.upstream_id === updated.upstream_id
                    ? updated
                    : row,
            ),
        });
    };

    const discoverModels = async (): Promise<void> => {
        setActionFailure(null);
        setNotice(null);
        setIsDiscovering(true);

        const outcome = await performManagementCall(() =>
            props.api.discoverModels({
                provider_id: props.providerId,
            }),
        );

        setIsDiscovering(false);

        if (outcome.status === "ok") {
            // The discovery response is the provider's complete fresh rows
            // (README §4.5): install them instead of refetching.
            models.setValue({
                models: outcome.result.models,
            });

            setNotice(discoveryNotice(outcome.result.models.length));
            return;
        }

        if (outcome.status === "failed") {
            setActionFailure(outcome.message);

            if (outcome.errno === ErrorCode.NotFound) {
                // The provider is gone, so the listing no longer shows it.
                await providers.reload();
            }
        }
    };

    const deleteProvider = async (): Promise<void> => {
        setActionFailure(null);
        setNotice(null);
        setIsDeleting(true);

        const outcome = await performManagementCall(() =>
            props.api.deleteProvider({
                provider_id: props.providerId,
            }),
        );

        setIsDeleting(false);

        if (outcome.status === "ok") {
            navigate(hrefs.providers());
            return;
        }

        if (outcome.status === "failed") {
            setIsConfirmingDelete(false);
            setActionFailure(outcome.message);

            if (outcome.errno === ErrorCode.NotFound) {
                await providers.reload();
            }
        }
    };

    const rowsChangedByAnotherWriter = async (): Promise<void> => {
        await Promise.all([
            providers.reload(),
            models.reload(),
        ]);
    };

    const providerSaved = async (): Promise<void> => {
        setNotice("Provider updated.");

        // `providers:update` answers an empty object (README §4.5), and a
        // changed prefix recomposes unoverridden model IDs — so both listings
        // this screen shows are refetched.
        await Promise.all([
            providers.reload(),
            models.reload(),
        ]);
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title={provider()?.name ?? `Provider #${props.providerId}`}
                description={providerDescription(provider(), props.providerId)}
                actions={
                    <Show
                        when={isConfirmingDelete()}
                        fallback={
                            <Button
                                variant="danger"
                                onClick={() => setIsConfirmingDelete(true)}
                            >
                                Delete provider
                            </Button>
                        }
                    >
                        <Button
                            variant="danger"
                            ref={focusOnMount}
                            disabled={isDeleting()}
                            onClick={() => void deleteProvider()}
                        >
                            {isDeleting() ? "Deleting…" : "Confirm delete"}
                        </Button>
                        <Button
                            variant="secondary"
                            disabled={isDeleting()}
                            onClick={() => setIsConfirmingDelete(false)}
                        >
                            Cancel
                        </Button>
                    </Show>
                }
            />

            <Show when={actionFailure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>
            <Show when={notice()}>
                {(message) => <Banner variant="success">{message()}</Banner>}
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
                                    Provider #{props.providerId} is not in the listing any more. It
                                    may have been deleted.
                                </p>
                                <div class="flex gap-2">
                                    <LinkButton
                                        href={hrefs.providers()}
                                        variant="secondary"
                                    >
                                        Back to providers
                                    </LinkButton>
                                    <Button
                                        variant="secondary"
                                        onClick={() => void providers.reload()}
                                    >
                                        Refresh
                                    </Button>
                                </div>
                            </div>
                        </Card>
                    </Show>
                }
            >
                {(row) => (
                    <div class="flex flex-col gap-6">
                        <Card title="Provider state">
                            <ProviderEditForm
                                api={props.api}
                                provider={row}
                                onSaved={() => void providerSaved()}
                                onSaveStarted={() => {
                                    setNotice(null);
                                    setActionFailure(null);
                                }}
                                onRowVanished={(message) => {
                                    setActionFailure(message);
                                    void rowsChangedByAnotherWriter();
                                }}
                            />
                        </Card>

                        <Card
                            title="Models"
                            description={
                                modelRows() === null
                                    ? "Model rows are refreshed from the provider's upstream."
                                    : `${modelRows()?.length ?? 0} model rows`
                            }
                            actions={
                                <Button
                                    variant="secondary"
                                    disabled={isDiscovering()}
                                    onClick={() => void discoverModels()}
                                >
                                    {isDiscovering() ? "Discovering…" : "Discover models"}
                                </Button>
                            }
                        >
                            <div class="flex flex-col gap-4">
                                <Show when={models.failure()}>
                                    {(message) => (
                                        <Banner variant="error">
                                            <p>{message()}</p>
                                            <div class="mt-2">
                                                <Button
                                                    variant="secondary"
                                                    onClick={() => void models.reload()}
                                                >
                                                    Retry
                                                </Button>
                                            </div>
                                        </Banner>
                                    )}
                                </Show>

                                <Show when={models.isLoading()}>
                                    <p
                                        role="status"
                                        class="text-sm text-slate-500"
                                    >
                                        Loading models…
                                    </p>
                                </Show>

                                <Show when={modelRows()}>
                                    {(rows) => (
                                        <Show
                                            when={rows().length > 0}
                                            fallback={
                                                <p class="text-sm text-slate-600">
                                                    This provider has no model rows yet. Discover
                                                    them from its upstream to start routing them.
                                                </p>
                                            }
                                        >
                                            <ModelList
                                                api={props.api}
                                                models={rows()}
                                                onModelUpdated={replaceModel}
                                                onModelVanished={() =>
                                                    void rowsChangedByAnotherWriter()
                                                }
                                            />
                                        </Show>
                                    )}
                                </Show>
                            </div>
                        </Card>
                    </div>
                )}
            </Show>
        </div>
    );
}

type ProviderEditFormProps = {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse;
    readonly onSaved: () => void;
    /** Called as a save begins, so the screen drops a notice it has superseded. */
    readonly onSaveStarted: () => void;
    /**
     * Called with the gateway's message when it reports the row gone (errno 3):
     * the screen renders the message and reloads its listings.
     */
    readonly onRowVanished: (message: string) => void;
};

/**
 * The provider's complete readable state, edited as a read-modify-write
 * (README §4.5). The form is mounted per row identity, so its fields start
 * from the row and pick up a changed row after a save.
 */
function ProviderEditForm(props: ProviderEditFormProps): JSX.Element {
    const [name, setName] = createSignal(props.provider.name);
    const [modelPrefix, setModelPrefix] = createSignal(props.provider.model_prefix);
    const [isEnabled, setIsEnabled] = createSignal(props.provider.is_enabled);
    const [baseUrl, setBaseUrl] = createSignal(
        props.provider.type === "openai_compatible" ? props.provider.base_url : "",
    );

    const [protocol, setProtocol] = createSignal(
        props.provider.type === "openai_compatible" ? props.provider.protocol : "openai_response",
    );

    const [apiKey, setApiKey] = createSignal("");
    const [nameError, setNameError] = createSignal<string | null>(null);
    const [baseUrlError, setBaseUrlError] = createSignal<string | null>(null);
    const [protocolError, setProtocolError] = createSignal<string | null>(null);
    const [failure, setFailure] = createSignal<string | null>(null);
    const [isSaving, setIsSaving] = createSignal(false);

    const save = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();

        if (isSaving()) {
            return;
        }

        const nameFailure = name().trim() === "" ? "Enter a provider name." : null;
        const isCompatible = props.provider.type === "openai_compatible";

        const baseUrlFailure =
            isCompatible && baseUrl().trim() === "" ? "Enter the upstream base URL." : null;

        const parsedProtocol = findUpstreamFormat(protocol());

        const protocolFailure =
            isCompatible && parsedProtocol === null ? "Choose a protocol." : null;

        setNameError(nameFailure);
        setBaseUrlError(baseUrlFailure);
        setProtocolError(protocolFailure);
        setFailure(null);

        if (
            nameFailure !== null ||
            baseUrlFailure !== null ||
            protocolFailure !== null ||
            parsedProtocol === null
        ) {
            return;
        }

        const edits: ProviderEdits = {
            name: name(),
            model_prefix: modelPrefix(),
            is_enabled: isEnabled(),
            ...(isCompatible
                ? {
                      base_url: baseUrl(),
                      protocol: parsedProtocol,
                      // An empty field keeps the stored secret: the API key is
                      // write-only and never rendered back.
                      ...(apiKey().trim() === ""
                          ? {}
                          : {
                                api_key: apiKey(),
                            }),
                  }
                : {}),
        };

        setIsSaving(true);
        props.onSaveStarted();

        const outcome = await performManagementCall(() =>
            props.api.updateProvider(toUpdateProviderRequest(props.provider, edits)),
        );

        setIsSaving(false);

        if (outcome.status === "ok") {
            props.onSaved();
            return;
        }

        if (outcome.status === "failed") {
            setFailure(outcome.message);

            if (outcome.errno === ErrorCode.NotFound) {
                props.onRowVanished(outcome.message);
            }
        }
    };

    const isCompatible = (): boolean => props.provider.type === "openai_compatible";

    return (
        <form
            novalidate={true}
            class="flex max-w-xl flex-col gap-4"
            onSubmit={save}
        >
            <Show when={failure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-slate-500">
                <span>
                    Type:{" "}
                    <span class="text-slate-700">{providerTypeLabel(props.provider.type)}</span>{" "}
                    <span class="font-mono text-slate-500">({props.provider.type})</span>
                </span>
                <Show when={props.provider.type !== "openai_compatible"}>
                    <span>
                        Token expiry:{" "}
                        <span class="text-slate-700">
                            {props.provider.type === "openai_compatible"
                                ? "—"
                                : props.provider.expires_at === null
                                  ? "no expiry"
                                  : formatTimestamp(props.provider.expires_at)}
                        </span>
                    </span>
                </Show>
            </div>

            <TextField
                id="provider-detail-name"
                label="Name"
                value={name()}
                onInput={setName}
                required
                disabled={isSaving()}
                error={nameError()}
                hint="Renaming keeps the provider's identity and every model routing."
            />
            <TextField
                id="provider-detail-model-prefix"
                label="Model prefix"
                value={modelPrefix()}
                onInput={setModelPrefix}
                disabled={isSaving()}
                isMonospace
                hint="Prepended to every model ID that has no override of its own."
            />
            <CheckboxField
                id="provider-detail-enabled"
                label="Serve traffic"
                checked={isEnabled()}
                onChange={setIsEnabled}
                disabled={isSaving()}
                hint="When disabled, none of this provider's models serve traffic."
            />

            <Show when={isCompatible()}>
                <TextField
                    id="provider-detail-base-url"
                    label="Upstream base URL"
                    value={baseUrl()}
                    onInput={setBaseUrl}
                    disabled={isSaving()}
                    error={baseUrlError()}
                    isMonospace
                />
                <SelectField
                    id="provider-detail-protocol"
                    label="Protocol"
                    value={protocol()}
                    options={UPSTREAM_FORMAT_OPTIONS}
                    onChange={setProtocol}
                    disabled={isSaving()}
                    error={protocolError()}
                />
                <TextField
                    id="provider-detail-api-key"
                    label="Replace API key"
                    type="password"
                    value={apiKey()}
                    onInput={setApiKey}
                    disabled={isSaving()}
                    autocomplete="off"
                    hint="Leave empty to keep the stored key; the stored key is never readable."
                />
            </Show>

            <Show when={!isCompatible()}>
                <div class="flex flex-col items-start gap-2 rounded-md border border-slate-200 bg-slate-50 px-4 py-3">
                    <p class="text-sm text-slate-700">
                        Credentials of an OAuth provider are replaced by authorizing the account
                        again.
                    </p>
                    <LinkButton
                        href={hrefs.oauthStart(props.provider.id)}
                        variant="secondary"
                    >
                        Reauthorize
                    </LinkButton>
                </div>
            </Show>

            <div class="flex items-center gap-2">
                <Button
                    type="submit"
                    disabled={isSaving()}
                >
                    {isSaving() ? "Saving…" : "Save provider"}
                </Button>
            </div>
        </form>
    );
}
