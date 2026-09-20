import type { JSX } from "@solidjs/web";
import { createSignal, Match, Show, Switch } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { Diagnosis } from "/src/management-api/diagnosis";
import type {
    OAuthProviderType,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { findOAuthProviderType } from "/src/management-api/vocabulary";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { AuthorizeStep } from "/src/ui/oauth-flow";
import {
    CompatibleConnectionFields,
    ProviderIdentityFields,
    type ProviderKind,
    ProviderKindPicker,
    ProviderServingSwitch,
} from "/src/ui/provider-form";

/**
 * One creation page for every provider kind. The three static-key wire
 * formats and the two OAuth logins are the same creation — the kind picker
 * only swaps the credential path that follows the shared identity fields.
 * A static-key kind is created under its own `type`; OAuth completions mint
 * the provider and land on its page, like a key creation does.
 */
export function ProviderNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [kind, setKind] = createSignal<ProviderKind>("openai_response");
    const [name, setName] = createSignal("");
    const [modelPrefix, setModelPrefix] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [baseUrl, setBaseUrl] = createSignal("");
    const [apiKey, setApiKey] = createSignal("");

    const pickKind = (next: ProviderKind): void => {
        setKind(next);
        setDiagnosis(null);
        setFlow(null);
        setCode("");
    };

    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [diagnosis, setDiagnosis] = createSignal<Diagnosis | null>(null);
    const [isCreating, setIsCreating] = createSignal(false);
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const isBusy = (): boolean => isCreating() || isStarting() || isCompleting();

    /** Creates a static-key provider. The gateway refuses nothing itself: a
     * taken name or an unparsable value comes back as its own message. */
    const create = async (): Promise<void> => {
        if (isBusy()) {
            return;
        }

        setDiagnosis(null);
        setIsCreating(true);

        const outcome = await performManagementCall(() =>
            props.api.createProvider({
                name: name(),
                model_prefix: modelPrefix(),
                is_enabled: isEnabled(),
                base_url: baseUrl(),
                type: kind(),
                api_key: apiKey(),
            }),
        );

        setIsCreating(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setDiagnosis(describeCallFailure(outcome.cause, "create-provider"));
            return;
        }

        navigate(hrefs.providerDetail(outcome.result.provider.id));
    };

    const start = async (oauthType: OAuthProviderType): Promise<void> => {
        if (isBusy()) {
            return;
        }

        setDiagnosis(null);
        setIsStarting(true);

        const outcome = await performManagementCall(() =>
            props.api.startProviderOauth({
                type: oauthType,
            }),
        );

        setIsStarting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setDiagnosis(describeCallFailure(outcome.cause, "start-oauth"));
            return;
        }

        setFlow(outcome.result);
    };

    const complete = async (): Promise<void> => {
        const startedFlow = flow();

        if (startedFlow === null || isBusy()) {
            return;
        }

        setDiagnosis(null);
        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.completeProviderOauth({
                state: startedFlow.state,
                code: code().trim(),
                name: name(),
                model_prefix: modelPrefix(),
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const failure = describeCallFailure(outcome.cause, "complete-oauth");
            setDiagnosis(failure);

            if (failure.kind === "gone") {
                // The state is unknown, expired, or already consumed: this
                // flow is dead, so it resets instead of being retried.
                setFlow(null);
                setCode("");
            }

            return;
        }

        navigate(hrefs.providerDetail(outcome.result.provider.id));
    };

    const submitCompatible = (event: SubmitEvent): void => {
        event.preventDefault();
        void create();
    };

    const submitOauth = (event: SubmitEvent): void => {
        event.preventDefault();
        void complete();
    };

    const startOver = (): void => {
        setFlow(null);
        setCode("");
        setDiagnosis(null);
    };

    const oauthKind = (): OAuthProviderType | null => findOAuthProviderType(kind());

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="New provider"
                description="One upstream provider. Pick the kind — the credential path follows."
                backHref={hrefs.providers()}
                backLabel="Providers"
            />

            <Show when={diagnosis()}>
                {(failure) => (
                    <Notice
                        tone="bad"
                        title={failure().headline}
                        detail={failure().detail}
                        fix={failure().fix}
                    />
                )}
            </Show>

            <Card>
                <CardHeader title="Type" />
                <div class="mt-4">
                    <ProviderKindPicker
                        value={kind()}
                        onChange={pickKind}
                        isDisabled={isBusy() || flow() !== null}
                    />
                </div>
            </Card>

            <Card>
                <CardHeader
                    title="Identity"
                    description="The name clients see, and the prefix every public model ID starts with."
                />
                <div class="mt-4 flex max-w-2xl flex-col gap-4">
                    <ProviderIdentityFields
                        name={name()}
                        modelPrefix={modelPrefix()}
                        onNameInput={setName}
                        onModelPrefixInput={setModelPrefix}
                        isDisabled={isBusy()}
                    />
                    <ProviderServingSwitch
                        isEnabled={isEnabled()}
                        onToggle={setIsEnabled}
                        isDisabled={isBusy()}
                    />
                </div>
            </Card>

            <Switch>
                <Match when={oauthKind()}>
                    {(oauthType) => (
                        <Card>
                            <CardHeader
                                title="Connect the account"
                                description="The gateway runs the authorization itself and mints the credentials. This app only relays the returned code and never stores it."
                            />
                            <Show
                                when={flow()}
                                keyed
                                fallback={
                                    <div class="mt-4">
                                        <Button
                                            tone="primary"
                                            isBusy={isStarting()}
                                            onClick={() => void start(oauthType())}
                                        >
                                            {isStarting() ? "Starting…" : "Start the authorization"}
                                        </Button>
                                    </div>
                                }
                            >
                                {(startedFlow) => (
                                    <form
                                        novalidate={true}
                                        class="mt-4 flex max-w-2xl flex-col gap-4"
                                        onSubmit={submitOauth}
                                    >
                                        <AuthorizeStep
                                            flow={startedFlow}
                                            onStartOver={startOver}
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
                                                    {isCompleting()
                                                        ? "Connecting…"
                                                        : "Connect the account"}
                                                </Button>
                                            </div>
                                        </AuthorizeStep>
                                    </form>
                                )}
                            </Show>
                        </Card>
                    )}
                </Match>
                <Match when={true}>
                    <Card>
                        <CardHeader
                            title="Connection"
                            description="Where the gateway sends this provider's traffic, and the key it authenticates with. The key is write-only — the gateway never returns it."
                        />
                        <form
                            novalidate={true}
                            class="mt-4 flex max-w-2xl flex-col gap-4"
                            onSubmit={submitCompatible}
                        >
                            <CompatibleConnectionFields
                                baseUrl={baseUrl()}
                                apiKey={apiKey()}
                                onBaseUrlInput={setBaseUrl}
                                onApiKeyInput={setApiKey}
                                isDisabled={isCreating()}
                            />
                            <div>
                                <Button
                                    type="submit"
                                    tone="primary"
                                    isBusy={isCreating()}
                                >
                                    {isCreating() ? "Creating…" : "Create provider"}
                                </Button>
                            </div>
                        </form>
                    </Card>
                </Match>
            </Switch>
        </div>
    );
}
