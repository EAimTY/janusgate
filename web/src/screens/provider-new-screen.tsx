import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Match, Show, Switch } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { Diagnosis } from "/src/management-api/diagnosis";
import type {
    OAuthProviderType,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { extractAuthorizationCode } from "/src/management-api/oauth-code";
import { performManagementCall } from "/src/management-api/outcome";
import {
    defaultModelPrefix,
    defaultProviderBaseName,
    normalizeModelPrefix,
    suggestProviderName,
} from "/src/management-api/provider-defaults";
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
} from "/src/ui/provider-form";

/**
 * One creation page for every provider kind. The three static-key wire
 * formats and the two OAuth logins are the same creation — the kind picker
 * only swaps the credential path that follows the shared identity fields.
 * Every provider starts enabled; switching one off lives on its edit page.
 * A static-key kind is created under its own `type`; OAuth completions mint
 * the provider and land on its page, like a key creation does.
 */
export function ProviderNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [kind, setKind] = createSignal<ProviderKind>("openai_response");
    const [name, setName] = createSignal(defaultProviderBaseName("openai_response"));
    const [isNameTouched, setIsNameTouched] = createSignal(false);
    const [takenNames, setTakenNames] = createSignal<ReadonlySet<string>>(new Set());
    const [isCustomPrefix, setIsCustomPrefix] = createSignal(false);
    const [customPrefix, setCustomPrefix] = createSignal("");
    const [baseUrl, setBaseUrl] = createSignal("");
    const [apiKey, setApiKey] = createSignal("");

    // The taken names arrive after the first read; until then the base name
    // stands in, and a hand-typed name is never overwritten.
    void performManagementCall(() => props.api.listProviders()).then((outcome) => {
        if (outcome.status !== "ok") {
            return;
        }

        const taken = new Set(outcome.result.providers.map((row) => row.name));
        setTakenNames(taken);

        if (!isNameTouched()) {
            setName(suggestProviderName(kind(), taken));
        }
    });

    const suggestion = createMemo(() => suggestProviderName(kind(), takenNames()));

    /** The name the request carries: what was typed, or the suggestion when untouched. */
    const resolvedName = createMemo(() => (name().trim() === "" ? suggestion() : name().trim()));

    /** The prefix the request carries: the default, or the hand-set value. */
    const resolvedPrefix = createMemo(() =>
        isCustomPrefix()
            ? normalizeModelPrefix(customPrefix())
            : defaultModelPrefix(resolvedName()),
    );

    const pickKind = (next: ProviderKind): void => {
        setKind(next);
        setDiagnosis(null);
        setFlow(null);
        setCode("");

        if (!isNameTouched()) {
            setName(suggestProviderName(next, takenNames()));
        }
    };

    const inputName = (value: string): void => {
        setIsNameTouched(true);
        setName(value);
    };

    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [diagnosis, setDiagnosis] = createSignal<Diagnosis | null>(null);
    const [isCreating, setIsCreating] = createSignal(false);
    const [isStarting, setIsStarting] = createSignal(false);
    const [isCompleting, setIsCompleting] = createSignal(false);

    const isBusy = (): boolean => isCreating() || isStarting() || isCompleting();

    /** Reads the pasted callback: a full URL reduces to its code, a bare code passes through. */
    const readPastedCode = (): string | null => {
        const pasted = extractAuthorizationCode(code());

        if (pasted !== null) {
            return pasted;
        }

        setDiagnosis({
            kind: "rejected",
            headline: "Paste the callback URL or the code.",
            detail: "That paste holds neither a bare code nor a URL with a code in it.",
            fix: "Copy the whole address the login landed on, or just its code, and paste it here.",
        });

        return null;
    };

    /** Creates a static-key provider. A taken name comes back as its own message. */
    const create = async (): Promise<void> => {
        if (isBusy()) {
            return;
        }

        setDiagnosis(null);
        setIsCreating(true);

        const outcome = await performManagementCall(() =>
            props.api.createProvider({
                name: resolvedName(),
                model_prefix: resolvedPrefix(),
                is_enabled: true,
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

        const pasted = readPastedCode();

        if (pasted === null) {
            return;
        }

        setDiagnosis(null);
        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.completeProviderOauth({
                state: startedFlow.state,
                code: pasted,
                name: resolvedName(),
                model_prefix: resolvedPrefix(),
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
                title="Create provider"
                description="One upstream account. Pick the kind — the credential path follows."
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
                <CardHeader title="Identity" />
                <div class="mt-4 flex max-w-2xl flex-col gap-4">
                    <ProviderIdentityFields
                        name={name()}
                        namePlaceholder={suggestion()}
                        onNameInput={inputName}
                        isCustomPrefix={isCustomPrefix()}
                        onCustomPrefixToggle={setIsCustomPrefix}
                        customPrefix={customPrefix()}
                        onCustomPrefixInput={setCustomPrefix}
                        defaultPrefix={defaultModelPrefix(resolvedName())}
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
                                description="Approve access, then paste back what the login landed on. This app never stores it."
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
                            description="The key is write-only — the gateway never returns it."
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
