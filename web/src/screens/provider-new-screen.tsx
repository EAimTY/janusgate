import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Match, onCleanup, Show, Switch } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { StartProviderOauthResponse } from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    compositionMessage,
    isBaseUrl,
    isBearerSecret,
    isModelPrefix,
    routingTokenMessage,
} from "/src/management-api/identifiers";
import { extractAuthorizationCode } from "/src/management-api/oauth-code";
import { performManagementCall } from "/src/management-api/outcome";
import { PROVIDER_PRESETS, type ProviderPreset } from "/src/management-api/presets";
import { defaultModelPrefix, suggestProviderName } from "/src/management-api/provider-defaults";
import { hrefs, navigate } from "/src/router";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { AuthorizeStep, DeviceStep } from "/src/ui/oauth-flow";
import {
    CompatibleConnectionFields,
    ProviderIdentityFields,
    ProviderPresetPicker,
} from "/src/ui/provider-form";
import { describeCallFailure } from "./failure";
import { type ScreenNotice, screenFailureNotice } from "./read";

/**
 * The creation page: one preset catalog, one definition, one provider. A
 * preset prefills the definition JSON the gateway stores under
 * `provider-definitions:create` — or reuses the stored version untouched —
 * and the credential path the preset declares follows the shared identity
 * fields: a static key is written outright, an OAuth flow is started and
 * completed. Editing the JSON of a stored definition registers the next
 * version; the stored one is never rewritten.
 */
export function ProviderNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [presetId, setPresetId] = createSignal(PROVIDER_PRESETS[0]?.id ?? "");
    const [definitions, setDefinitions] = createSignal<
        ReadonlyArray<{
            readonly name: string;
            readonly version: number;
        }>
    >([]);
    const [name, setName] = createSignal("");
    const [isNameTouched, setIsNameTouched] = createSignal(false);
    const [takenNames, setTakenNames] = createSignal<ReadonlySet<string>>(new Set());
    const [isCustomPrefix, setIsCustomPrefix] = createSignal(false);
    const [customPrefix, setCustomPrefix] = createSignal("");
    const [baseUrl, setBaseUrl] = createSignal("");
    const [apiKey, setApiKey] = createSignal("");

    const preset = (): ProviderPreset =>
        PROVIDER_PRESETS.find((candidate) => candidate.id === presetId()) ??
        (PROVIDER_PRESETS[0] as ProviderPreset);

    /** The stored definition of this preset, when the gateway holds one. */
    const stored = createMemo(() =>
        definitions().find((definition) => definition.name === preset().id),
    );

    /** The definition JSON the operator edits; prefilled from the preset. */
    const [definitionJson, setDefinitionJson] = createSignal(
        JSON.stringify(preset().payload, null, 2),
    );
    const [isDefinitionTouched, setIsDefinitionTouched] = createSignal(false);

    const pickPreset = (next: string): void => {
        setPresetId(next);
        setFailure(null);
        setFlow(null);
        setCode("");

        const nextPreset = PROVIDER_PRESETS.find((candidate) => candidate.id === next) ?? preset();

        setIsDefinitionTouched(false);
        setDefinitionJson(JSON.stringify(nextPreset.payload, null, 2));

        if (!isNameTouched()) {
            setName(suggestProviderName(nextPreset.suggestedName, takenNames()));
        }
    };

    // The taken names arrive after the first read; until then the preset's
    // base name stands in, and a hand-typed name is never overwritten.
    const suggestionController = new AbortController();
    onCleanup(() => suggestionController.abort());

    void performManagementCall(() => props.api.listProviders(suggestionController.signal)).then(
        (outcome) => {
            if (outcome.status !== "ok") {
                return;
            }

            const taken = new Set(outcome.result.providers.map((row) => row.name));
            setTakenNames(taken);

            if (!isNameTouched()) {
                setName(suggestProviderName(preset().suggestedName, taken));
            }
        },
    );

    void performManagementCall(() =>
        props.api.listProviderDefinitions(suggestionController.signal),
    ).then((outcome) => {
        if (outcome.status !== "ok") {
            return;
        }

        setDefinitions(
            outcome.result.provider_definitions.map((definition) => ({
                name: definition.name,
                version: definition.version,
            })),
        );
    });

    const suggestion = createMemo(() => suggestProviderName(preset().suggestedName, takenNames()));

    /** The name the request carries: what was typed, or the suggestion when untouched. */
    const resolvedName = createMemo(() => (name().trim() === "" ? suggestion() : name().trim()));

    /**
     * The prefix the request carries: the default, or the hand-set value.
     * The typed prefix is the value on the wire, byte-for-byte.
     */
    const resolvedPrefix = createMemo(() =>
        isCustomPrefix() ? customPrefix() : defaultModelPrefix(resolvedName()),
    );

    /** Inline composition errors, shown where the value was typed. */
    const prefixError = (): string | null =>
        isModelPrefix(resolvedPrefix()) ? null : routingTokenMessage("Model prefix");

    const trimmedBaseUrl = (): string => baseUrl().trim();

    const needsBaseUrl = (): boolean => preset().needsBaseUrl;

    const baseUrlError = (): string | null => {
        const trimmed = trimmedBaseUrl();
        return (!needsBaseUrl() || isBaseUrl(trimmed)) && (needsBaseUrl() || trimmed === "")
            ? null
            : baseUrlMessage();
    };

    const apiKeyError = (): string | null =>
        apiKey() === "" || isBearerSecret(apiKey()) ? null : compositionMessage("API key");

    /** A static-key creation is submittable with a complete, routable connection. */
    const canCreateStatic = (): boolean =>
        prefixError() === null &&
        (!needsBaseUrl() || isBaseUrl(trimmedBaseUrl())) &&
        isBearerSecret(apiKey());

    /** The parsed definition payload, or the parse error naming it. */
    const parsedDefinition = ():
        | {
              payload: unknown;
          }
        | {
              error: string;
          } => {
        try {
            return {
                payload: JSON.parse(definitionJson()),
            };
        } catch (error) {
            return {
                error: error instanceof Error ? error.message : String(error),
            };
        }
    };

    const definitionError = (): string | null => {
        const parsed = parsedDefinition();
        return "error" in parsed ? parsed.error : null;
    };

    /**
     * Ensures the definition the preset names exists under the edited JSON:
     * an untouched preset with a stored version reuses it, a new name creates
     * version 1, an edited stored name registers the next version.
     */
    const ensureDefinition = async (): Promise<
        | {
              name: string;
              version: number;
          }
        | {
              error: ScreenNotice;
          }
    > => {
        const parsed = parsedDefinition();

        if ("error" in parsed) {
            return {
                error: {
                    tone: "bad",
                    title: "The definition JSON does not parse.",
                    detail: parsed.error,
                    fix: "Fix the JSON and submit again; nothing was written.",
                },
            };
        }

        const payload = parsed.payload as Record<string, unknown>;
        const current = stored();

        if (current !== undefined && !isDefinitionTouched() && Object.keys(payload).length > 0) {
            return {
                name: current.name,
                version: current.version,
            };
        }

        if (current === undefined) {
            const outcome = await performManagementCall(() =>
                props.api.createProviderDefinition({
                    name: preset().id,
                    ...(payload as {
                        format: Parameters<typeof props.api.createProviderDefinition>[0]["format"];
                        inference: never;
                        credential: never;
                    }),
                }),
            );

            if (outcome.status === "aborted") {
                return {
                    error: {
                        tone: "info",
                        title: "Cancelled.",
                        detail: "",
                        fix: null,
                    },
                };
            }

            if (outcome.status === "failed") {
                return {
                    error: screenFailureNotice(outcome.cause, {
                        action: "create-provider",
                    }),
                };
            }

            return {
                name: outcome.result.provider_definition.name,
                version: outcome.result.provider_definition.version,
            };
        }

        const outcome = await performManagementCall(() =>
            props.api.updateProviderDefinition({
                name: current.name,
                base_version: current.version,
                ...(payload as {
                    format: Parameters<typeof props.api.updateProviderDefinition>[0]["format"];
                    inference: never;
                    credential: never;
                }),
            }),
        );

        if (outcome.status === "aborted") {
            return {
                error: {
                    tone: "info",
                    title: "Cancelled.",
                    detail: "",
                    fix: null,
                },
            };
        }

        if (outcome.status === "failed") {
            return {
                error: screenFailureNotice(outcome.cause, {
                    action: "update-provider",
                }),
            };
        }

        return {
            name: outcome.result.provider_definition.name,
            version: outcome.result.provider_definition.version,
        };
    };

    /** An OAuth completion is submittable with a valid, routable prefix. */
    const canComplete = (): boolean => prefixError() === null;

    const inputName = (value: string): void => {
        setIsNameTouched(true);
        setName(value);
    };

    const [flow, setFlow] = createSignal<StartProviderOauthResponse | null>(null);
    const [code, setCode] = createSignal("");
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);
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

        setFailure({
            tone: "bad",
            title: "Paste the callback URL or the code.",
            detail: "That paste holds neither a bare code nor a URL with a code in it.",
            fix: "Copy the whole address the login landed on, or just its code, and paste it here.",
        });

        return null;
    };

    /** Creates a static-key provider under the ensured definition. */
    const create = async (): Promise<void> => {
        if (isBusy() || !canCreateStatic()) {
            return;
        }

        setFailure(null);
        setIsCreating(true);

        const ensured = await ensureDefinition();

        if ("error" in ensured) {
            setIsCreating(false);
            setFailure(ensured.error);
            return;
        }

        const outcome = await performManagementCall(() =>
            props.api.createProvider({
                provider_definition_name: ensured.name,
                provider_definition_version: ensured.version,
                name: resolvedName(),
                model_prefix: resolvedPrefix(),
                is_enabled: true,
                base_url: needsBaseUrl() ? trimmedBaseUrl() : null,
                api_key: apiKey(),
            }),
        );

        setIsCreating(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setFailure(
                screenFailureNotice(outcome.cause, {
                    action: "create-provider",
                }),
            );
            return;
        }

        navigate(hrefs.providerDetail(outcome.result.provider.id));
    };

    const start = async (): Promise<void> => {
        if (isBusy() || flow() !== null) {
            return;
        }

        setFailure(null);
        setIsStarting(true);

        const ensured = await ensureDefinition();

        if ("error" in ensured) {
            setIsStarting(false);
            setFailure(ensured.error);
            return;
        }

        const outcome = await performManagementCall(() =>
            props.api.startProviderOauth({
                provider_definition_name: ensured.name,
                provider_definition_version: ensured.version,
                name: resolvedName(),
                model_prefix: resolvedPrefix(),
                ...(needsBaseUrl() && isBaseUrl(trimmedBaseUrl())
                    ? {
                          base_url: trimmedBaseUrl(),
                      }
                    : {}),
            }),
        );

        setIsStarting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setFailure(
                screenFailureNotice(outcome.cause, {
                    action: "start-oauth",
                }),
            );
            return;
        }

        setFlow(outcome.result);
    };

    /** Completes an authorization-code flow with the pasted callback. */
    const complete = async (): Promise<void> => {
        const startedFlow = flow();

        if (
            startedFlow === null ||
            startedFlow.grant !== "authorization_code" ||
            isBusy() ||
            prefixError() !== null
        ) {
            return;
        }

        const pasted = readPastedCode();

        if (pasted === null) {
            return;
        }

        setFailure(null);
        setIsCompleting(true);

        const outcome = await performManagementCall(() =>
            props.api.completeProviderOauth({
                grant: "authorization_code",
                state: startedFlow.state,
                code: pasted,
            }),
        );

        setIsCompleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setFailure(
                screenFailureNotice(outcome.cause, {
                    action: "complete-oauth",
                }),
            );

            if (
                describeCallFailure(outcome.cause, {
                    action: "complete-oauth",
                }).kind === "gone"
            ) {
                // The state is unknown, expired, or already consumed: this
                // flow is dead, so it resets instead of being retried.
                setFlow(null);
                setCode("");
            }

            return;
        }

        if (outcome.result.status === "succeeded") {
            const providerId = outcome.result.provider.id;
            navigate(hrefs.providerDetail(providerId));
        }
    };

    /** One device-poll ask; the step panel schedules the next one. */
    const completeDevice = async (): Promise<"pending" | "done"> => {
        const startedFlow = flow();

        if (startedFlow === null || startedFlow.grant !== "device") {
            return "done";
        }

        const outcome = await performManagementCall(() =>
            props.api.completeProviderOauth({
                grant: "device",
                state: startedFlow.state,
            }),
        );

        if (outcome.status === "aborted") {
            return "pending";
        }

        if (outcome.status === "failed") {
            return "pending";
        }

        if (outcome.result.status === "pending") {
            return "pending";
        }

        if (outcome.result.status === "failed") {
            setFailure({
                tone: "bad",
                title: "The flow failed.",
                detail: outcome.result.failure_message,
                fix: "Start a fresh flow and authorize it promptly.",
            });

            return "done";
        }

        if (outcome.result.status !== "succeeded") {
            return "done";
        }

        navigate(hrefs.providerDetail(outcome.result.provider.id));

        return "done";
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
        setFailure(null);
    };

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Create provider"
                description="One upstream account. Pick the preset — the credential path follows."
                backHref={hrefs.providers()}
                backLabel="Providers"
            />

            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Card>
                <CardHeader title="Preset" />
                <div class="mt-4">
                    <ProviderPresetPicker
                        presets={PROVIDER_PRESETS}
                        value={presetId()}
                        onChange={pickPreset}
                        isDisabled={isBusy() || flow() !== null}
                    />
                </div>
            </Card>

            <Card>
                <CardHeader
                    title="Definition"
                    description="The connection recipe the gateway stores. Editing the JSON of a stored definition registers the next version — the stored one is never rewritten."
                />
                <div class="mt-4">
                    <Show
                        when={stored()}
                        fallback={
                            <p class="text-[12.5px] text-ink-faint">
                                Not stored yet — submitting creates version 1.
                            </p>
                        }
                    >
                        {(current) => (
                            <p class="text-[12.5px] text-ink-faint">
                                {`Stored as version ${current().version}.`}
                            </p>
                        )}
                    </Show>
                    <textarea
                        class="tnum mt-2 h-80 w-full rounded-lg border border-rule bg-canvas p-3 font-mono text-[12.5px] leading-relaxed dark:bg-muted"
                        spellcheck={false}
                        value={definitionJson()}
                        onInput={(event) => {
                            setDefinitionJson(event.currentTarget.value);
                            setIsDefinitionTouched(true);
                        }}
                        disabled={isBusy()}
                    />
                    <Show when={definitionError()}>
                        {(current) => (
                            <p class="mt-1 text-[12.5px] text-red-600 dark:text-red-400">
                                {current()}
                            </p>
                        )}
                    </Show>
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
                        prefixError={prefixError()}
                        isDisabled={isBusy()}
                    />
                </div>
            </Card>

            <Switch>
                <Match when={preset().grant !== null}>
                    <Card>
                        <CardHeader
                            title="Connect the account"
                            description="Approve access where the flow points, then let it complete. This app never stores the credentials."
                        />
                        <Show
                            when={flow()}
                            keyed
                            fallback={
                                <div class="mt-4">
                                    <Button
                                        tone="primary"
                                        isBusy={isStarting()}
                                        onClick={() => void start()}
                                    >
                                        {isStarting() ? "Starting…" : "Start the authorization"}
                                    </Button>
                                </div>
                            }
                        >
                            {(startedFlow) => (
                                <div class="mt-4">
                                    <Show
                                        when={startedFlow.grant === "authorization_code"}
                                        fallback={
                                            <DeviceStep
                                                flow={startedFlow as never}
                                                onComplete={completeDevice}
                                                onSucceeded={() => {}}
                                                onFailed={() => {}}
                                                onStartOver={startOver}
                                            />
                                        }
                                    >
                                        <form
                                            novalidate={true}
                                            class="flex max-w-2xl flex-col gap-4"
                                            onSubmit={submitOauth}
                                        >
                                            <AuthorizeStep
                                                flow={
                                                    startedFlow as Extract<
                                                        StartProviderOauthResponse,
                                                        {
                                                            grant: "authorization_code";
                                                        }
                                                    >
                                                }
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
                                                        isDisabled={!canComplete()}
                                                        isBusy={isCompleting()}
                                                    >
                                                        {isCompleting()
                                                            ? "Connecting…"
                                                            : "Connect the account"}
                                                    </Button>
                                                </div>
                                            </AuthorizeStep>
                                        </form>
                                    </Show>
                                </div>
                            )}
                        </Show>
                    </Card>
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
                            <Show when={needsBaseUrl()}>
                                <CompatibleConnectionFields
                                    baseUrl={baseUrl()}
                                    apiKey={apiKey()}
                                    onBaseUrlInput={setBaseUrl}
                                    onApiKeyInput={setApiKey}
                                    baseUrlError={baseUrlError()}
                                    apiKeyError={apiKeyError()}
                                    isDisabled={isCreating()}
                                />
                            </Show>
                            <Show when={!needsBaseUrl()}>
                                <CompatibleConnectionFields
                                    baseUrl=""
                                    apiKey={apiKey()}
                                    onBaseUrlInput={setBaseUrl}
                                    onApiKeyInput={setApiKey}
                                    apiKeyError={apiKeyError()}
                                    isDisabled={isCreating()}
                                />
                            </Show>
                            <div>
                                <Button
                                    type="submit"
                                    tone="primary"
                                    isDisabled={!canCreateStatic()}
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
