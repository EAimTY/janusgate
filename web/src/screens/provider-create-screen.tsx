import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import { performManagementCall } from "/src/management-api/outcome";
import { findUpstreamFormat, UPSTREAM_FORMAT_OPTIONS } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { AppLink } from "/src/ui/app-link";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { Card } from "/src/ui/card";
import { CheckboxField, SelectField, TextField } from "/src/ui/field";
import { PageHeader } from "/src/ui/page-header";

export type ProviderCreateScreenProps = {
    readonly api: ManagementApi;
};

/**
 * `providers:create` (README §4.4): the complete state of one
 * `openai_compatible` provider. The two OAuth account kinds are created by the
 * OAuth flow instead, so this form does not offer them.
 */
export function ProviderCreateScreen(props: ProviderCreateScreenProps): JSX.Element {
    const [name, setName] = createSignal("");
    const [modelPrefix, setModelPrefix] = createSignal("");
    const [baseUrl, setBaseUrl] = createSignal("");
    const [protocol, setProtocol] = createSignal("openai_response");
    const [apiKey, setApiKey] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [nameError, setNameError] = createSignal<string | null>(null);
    const [baseUrlError, setBaseUrlError] = createSignal<string | null>(null);
    const [apiKeyError, setApiKeyError] = createSignal<string | null>(null);
    const [protocolError, setProtocolError] = createSignal<string | null>(null);
    const [failure, setFailure] = createSignal<string | null>(null);
    const [isSaving, setIsSaving] = createSignal(false);

    const submit = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();
        if (isSaving()) {
            return;
        }

        const nameFailure = name().trim() === "" ? "Enter a provider name." : null;
        const baseUrlFailure = baseUrl().trim() === "" ? "Enter the upstream base URL." : null;
        const apiKeyFailure = apiKey().trim() === "" ? "Enter the API key." : null;
        const parsedProtocol = findUpstreamFormat(protocol());
        const protocolFailure = parsedProtocol === null ? "Choose a protocol." : null;

        setNameError(nameFailure);
        setBaseUrlError(baseUrlFailure);
        setApiKeyError(apiKeyFailure);
        setProtocolError(protocolFailure);
        setFailure(null);
        if (
            nameFailure !== null ||
            baseUrlFailure !== null ||
            apiKeyFailure !== null ||
            parsedProtocol === null
        ) {
            return;
        }

        setIsSaving(true);
        const outcome = await performManagementCall(() =>
            props.api.createProvider({
                name: name(),
                model_prefix: modelPrefix(),
                is_enabled: isEnabled(),
                base_url: baseUrl(),
                protocol: parsedProtocol,
                api_key: apiKey(),
            }),
        );
        setIsSaving(false);

        if (outcome.status === "ok") {
            navigate(hrefs.providerDetail(outcome.result.provider.id));
            return;
        }
        if (outcome.status === "failed") {
            setFailure(outcome.message);
        }
    };

    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Create provider"
                description="An OpenAI-compatible upstream account. The API key is write-only: the gateway never returns it."
            />

            <Show when={failure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <Card title="Provider state">
                <form
                    novalidate={true}
                    class="flex max-w-xl flex-col gap-4"
                    onSubmit={submit}
                >
                    <TextField
                        id="provider-name"
                        label="Name"
                        value={name()}
                        onInput={setName}
                        required
                        disabled={isSaving()}
                        error={nameError()}
                        hint="Unique, renameable label."
                    />
                    <TextField
                        id="provider-model-prefix"
                        label="Model prefix"
                        value={modelPrefix()}
                        onInput={setModelPrefix}
                        disabled={isSaving()}
                        isMonospace
                        hint="Prepended to every public model name; may be empty."
                    />
                    <TextField
                        id="provider-base-url"
                        label="Upstream base URL"
                        value={baseUrl()}
                        onInput={setBaseUrl}
                        required
                        disabled={isSaving()}
                        error={baseUrlError()}
                        isMonospace
                        placeholder="https://upstream.example.com/v1"
                    />
                    <SelectField
                        id="provider-protocol"
                        label="Protocol"
                        value={protocol()}
                        options={UPSTREAM_FORMAT_OPTIONS}
                        onChange={setProtocol}
                        disabled={isSaving()}
                        error={protocolError()}
                        hint="The wire format the upstream speaks."
                    />
                    <TextField
                        id="provider-api-key"
                        label="API key"
                        type="password"
                        value={apiKey()}
                        onInput={setApiKey}
                        required
                        disabled={isSaving()}
                        error={apiKeyError()}
                        autocomplete="off"
                        hint="Sent to the upstream with every request; stored by the gateway, never returned."
                    />
                    <CheckboxField
                        id="provider-enabled"
                        label="Serve traffic"
                        checked={isEnabled()}
                        onChange={setIsEnabled}
                        disabled={isSaving()}
                        hint="When disabled, none of this provider's models serve traffic."
                    />

                    <div class="flex items-center gap-2">
                        <Button
                            type="submit"
                            disabled={isSaving()}
                        >
                            {isSaving() ? "Creating…" : "Create provider"}
                        </Button>
                        <AppLink
                            href={hrefs.providers()}
                            class="text-sm text-slate-600 hover:text-slate-900"
                        >
                            Cancel
                        </AppLink>
                    </div>
                </form>
            </Card>

            <p class="text-sm text-slate-600">
                To connect an OpenAI or Antigravity account instead,{" "}
                <AppLink
                    href={hrefs.oauthStart(null)}
                    class="font-medium text-slate-900 hover:underline"
                >
                    start an OAuth flow
                </AppLink>
                : the gateway mints those credentials itself.
            </p>
        </div>
    );
}
