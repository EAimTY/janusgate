import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Match, Show, Switch } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import {
    type DefinitionPayloadResult,
    hasBaseReference,
    validateDefinitionPayload,
} from "/src/management-api/definition-payload";
import type { DefinitionResponse } from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    bearerSecretError,
    isBaseUrl,
    isBearerSecret,
    modelPrefixError,
} from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import {
    matchStoredDefinition,
    PROVIDER_PRESETS,
    type ProviderPreset,
} from "/src/management-api/presets";
import { defaultModelPrefix, suggestProviderName } from "/src/management-api/provider-defaults";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { Button, TextField } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { AuthorizeStep, DeviceStep } from "/src/ui/oauth-flow";
import {
    CompatibleConnectionFields,
    ProviderIdentityFields,
    ProviderPresetPicker,
} from "/src/ui/provider-form";
import { requiredHint } from "/src/ui/required-hint";
import { createCreateAction } from "./create-form";
import { createOauthFlow } from "./oauth-flow";
import { type ScreenNotice, screenFailureNotice } from "./read";

/**
 * The one line each editable card states while a live flow holds its fields:
 * the lock's reason, stated once per card rather than as a hint on every
 * disabled control.
 */
const FLOW_LOCK_NOTICE =
    "Locked while the authorization is live — the provider is created from what the flow's start already sent.";

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
    const [presetId, setPresetId] = createSignal(PROVIDER_PRESETS[0].id);
    const [definitions, setDefinitions] = createSignal<ReadonlyArray<DefinitionResponse>>([]);
    /** What the mount read established about the preset's stored versions. */
    const [isStoredKnown, setIsStoredKnown] = createSignal<"reading" | "read" | "unreadable">(
        "reading",
    );
    const [name, setName] = createSignal("");
    const [isNameTouched, setIsNameTouched] = createSignal(false);
    const [takenNames, setTakenNames] = createSignal<ReadonlySet<string>>(new Set());
    const [isCustomPrefix, setIsCustomPrefix] = createSignal(false);
    const [customPrefix, setCustomPrefix] = createSignal("");
    const [baseUrl, setBaseUrl] = createSignal("");
    // The base URL's error is stated only once the operator has typed into
    // the field — a pristine form is not an error, a required-but-empty field
    // is not yet a mistake (the api-key forms' untouched-name gate).
    const [hasBaseUrlInput, setHasBaseUrlInput] = createSignal(false);
    const [apiKey, setApiKey] = createSignal("");
    // The page's standing notice: the boot read's failures and the static
    // creation path's outcomes. The OAuth flow's failures never reach it —
    // the flow fragment states its own inside the Connect card — so the two
    // channels cannot render one outcome twice.
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);
    /** Whether the boot read's cycle is in flight — the read-again button's busy state. */
    const [isBootReading, setIsBootReading] = createSignal(true);

    const preset = (): ProviderPreset =>
        PROVIDER_PRESETS.find((candidate) => candidate.id === presetId()) ?? PROVIDER_PRESETS[0];

    /**
     * The latest stored version of this preset's definition, when the gateway
     * holds one — the listing orders by name then version, so the last of a
     * name is the newest.
     */
    const stored = createMemo(() => matchStoredDefinition(preset(), definitions()));

    /** The definition JSON the operator edits; prefilled from the preset. */
    const [definitionJson, setDefinitionJson] = createSignal(
        JSON.stringify(preset().payload, null, 2),
    );
    const [isDefinitionTouched, setIsDefinitionTouched] = createSignal(false);

    /**
     * Whether the Advanced disclosure stands open, tracked from its `toggle`
     * event: the invalid-recipe pointer outside the disclosure instructs
     * opening it, so the pointer must yield while the disclosure is showing
     * the offending path itself — both statements at once would give an
     * instruction that cannot be followed.
     */
    const [isDefinitionOpen, setIsDefinitionOpen] = createSignal(false);

    /**
     * The preset a pick armed while an edited connection recipe stands: the
     * switch is pending, not applied — the Preset card's confirm is the only
     * way it happens, so a stray click in the grid cannot destroy the edited
     * JSON. One signal keeps the mechanism minimal, and every path that
     * removes what the arming guards dissolves it: the confirm, the cancel,
     * a reset to the preset, and a flow going live.
     */
    const [armedPresetId, setArmedPresetId] = createSignal<string | null>(null);

    /** The armed pick resolved to its catalog entry, `null` when disarmed. */
    const armedPreset = createMemo(() => {
        const armedId = armedPresetId();
        return armedId === null
            ? null
            : (PROVIDER_PRESETS.find((candidate) => candidate.id === armedId) ?? null);
    });

    // The taken names arrive after the boot read; until then the preset's
    // base name stands in, and a hand-typed name is never overwritten. A read
    // that fails is stated where every failure on this page stands — never
    // silently swallowed into a suggestion that names a taken name again.
    let bootController: AbortController | null = null;

    /**
     * The boot read: the taken provider names (the name suggestion's ground
     * truth) and the preset's stored definition versions. It subscribes to
     * the refresh pulse directly instead of through the shared read harness:
     * the page renders fully without these reads — they refine the
     * suggestion and the stored-version line, and own no skeleton — so the
     * pulse re-runs them here, and the page's own Read again control beside
     * the failure notice reruns them in place (README §6.3). It writes only
     * the listing signals and the notice, never the live OAuth flow's state:
     * neither rerun may unlock or disturb a flow in flight.
     */
    const readBootState = (): void => {
        bootController?.abort();

        const cycle = new AbortController();
        bootController = cycle;
        setIsBootReading(true);

        // Both calls must answer for the cycle to count as read: a failed
        // one states its diagnosis on the notice, and only an answered cycle
        // clears the notice and stamps the shell.
        let settledCount = 0;
        let failedCount = 0;

        const settleBootCall = (isSuccessful: boolean): void => {
            settledCount += 1;

            if (!isSuccessful) {
                failedCount += 1;
            }

            if (settledCount === 2) {
                // The cycle is fully answered: the in-flight state lifts
                // whether the cycle read or failed — a failed one keeps the
                // notice, and the read-again button beside it, standing.
                // An aborted call never settles here, so a superseded cycle
                // cannot flip the state the newer cycle owns.
                setIsBootReading(false);

                if (failedCount === 0) {
                    setFailure(null);
                    reportRefresh();
                }
            }
        };

        void performManagementCall(() => props.api.listProviders(cycle.signal)).then((outcome) => {
            if (outcome.status === "aborted" || cycle.signal.aborted) {
                return;
            }

            if (outcome.status === "failed") {
                setFailure(screenFailureNotice(outcome.cause));
                settleBootCall(false);
                return;
            }

            const taken = new Set(outcome.result.providers.map((row) => row.name));
            setTakenNames(taken);

            if (!isNameTouched()) {
                setName(suggestProviderName(preset().suggestedName, taken));
            }

            settleBootCall(true);
        });

        void performManagementCall(() => props.api.listProviderDefinitions(cycle.signal)).then(
            (outcome) => {
                if (outcome.status === "aborted" || cycle.signal.aborted) {
                    return;
                }

                if (outcome.status === "failed") {
                    setIsStoredKnown("unreadable");
                    setFailure(screenFailureNotice(outcome.cause));
                    settleBootCall(false);
                    return;
                }

                setIsStoredKnown("read");
                setDefinitions(outcome.result.provider_definitions);
                settleBootCall(true);
            },
        );
    };

    // One boot read per mount, repeated when *refresh* is pressed, the cycle
    // in flight aborted when the pulse moves again or the screen goes away —
    // the read harness's subscription, minus the skeleton.
    createEffect(
        () => refreshRequest(),
        () => {
            readBootState();

            return () => {
                bootController?.abort();
            };
        },
    );

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
    const prefixError = (): string | null => modelPrefixError(resolvedPrefix());

    const trimmedBaseUrl = (): string => baseUrl().trim();

    /** Inline error for the base URL, shown where it was typed and only once
     * it was typed: a pristine form states the requirement through the
     * field's quiet hint, never as alert-red from first paint. */
    const baseUrlError = (): string | null => {
        if (!hasBaseUrlInput()) {
            return null;
        }

        const trimmed = trimmedBaseUrl();
        return !needsBaseUrl() || isBaseUrl(trimmed) ? null : baseUrlMessage();
    };

    /** Types into the base URL field, opening its inline error. */
    const inputBaseUrl = (value: string): void => {
        setHasBaseUrlInput(true);
        setBaseUrl(value);
    };

    const apiKeyError = (): string | null => bearerSecretError(apiKey(), "API key");

    /** A static-key creation is submittable with a valid definition and a complete, routable connection. */
    const canCreateStatic = (): boolean =>
        definitionError() === null &&
        prefixError() === null &&
        (!needsBaseUrl() || isBaseUrl(trimmedBaseUrl())) &&
        isBearerSecret(apiKey());

    /**
     * The edited definition payload, validated, or the offending JSON path.
     * Memoized so the parse-and-validate walk runs once per edit of the JSON
     * rather than once per consumer read — the submit gates, the ensure
     * step, and the two inline error statements all read the same verdict.
     */
    const parsedDefinition = createMemo((): DefinitionPayloadResult => {
        try {
            const parsed: unknown = JSON.parse(definitionJson());
            return validateDefinitionPayload(parsed);
        } catch (error) {
            return {
                error: error instanceof Error ? error.message : String(error),
            };
        }
    });

    const definitionError = (): string | null => {
        const parsed = parsedDefinition();
        return "error" in parsed ? parsed.error : null;
    };

    /**
     * The base-URL neediness the definition under edit dictates, derived from
     * the memoized payload's own URL expressions — the browser-side mirror of
     * the gateway's base-URL requirement (gateway README §7.1 rejects the
     * write or flow start naming a base-addressed definition without one),
     * never a catalog fact. An edit that references the base opens the field
     * and closes the gate; a parse-broken edit has no payload to ask about,
     * and its submit is held by the definition error alone.
     */
    const needsBaseUrl = (): boolean => {
        const parsed = parsedDefinition();
        return "payload" in parsed && hasBaseReference(parsed.payload);
    };

    /**
     * Ensures the definition the preset names exists under the edited JSON:
     * an untouched preset with a stored version reuses its latest version, a
     * new name creates version 1, an edited stored name registers the next
     * version. The definition calls pass no signal, so they cannot abort —
     * the only outcomes are the ensured version and a failure.
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
                    title: "The definition JSON is not a definition the gateway could store.",
                    detail: parsed.error,
                    fix: "Fix the JSON and submit again; nothing was written.",
                },
            };
        }

        const current = stored();

        if (current !== null && !isDefinitionTouched()) {
            return {
                name: current.name,
                version: current.version,
            };
        }

        try {
            if (current === null) {
                const result = await props.api.createProviderDefinition({
                    name: preset().id,
                    ...parsed.payload,
                });

                return {
                    name: result.provider_definition.name,
                    version: result.provider_definition.version,
                };
            }

            const result = await props.api.updateProviderDefinition({
                name: current.name,
                base_version: current.version,
                ...parsed.payload,
            });

            return {
                name: result.provider_definition.name,
                version: result.provider_definition.version,
            };
        } catch (error) {
            // The definition write is its own subject: its name is the preset
            // id and its base version the stored latest, neither of which this
            // page edits — a conflict's recovery is the moved stored version,
            // never the provider-name fix a provider write's conflict carries.
            return {
                error: screenFailureNotice(error, {
                    action: "ensure-definition",
                }),
            };
        }
    };

    /**
     * An OAuth start or completion is submittable with a valid definition, a
     * routable prefix, and — when the definition addresses the base — a valid
     * base URL: exactly the static path's gate, so the gateway's 400 naming
     * `base_url` never has a chance to leave the browser.
     */
    const canComplete = (): boolean =>
        definitionError() === null &&
        prefixError() === null &&
        (!needsBaseUrl() || isBaseUrl(trimmedBaseUrl()));

    const inputName = (value: string): void => {
        setIsNameTouched(true);
        setName(value);
    };

    /** Restores the selected preset's payload and re-arms the stored-version reuse. */
    const resetDefinition = (): void => {
        // With the edit gone there is nothing the preset-switch arming
        // guards, so it goes with the edit.
        setArmedPresetId(null);
        setDefinitionJson(JSON.stringify(preset().payload, null, 2));
        setIsDefinitionTouched(false);
    };

    // The OAuth flow's shared state machine: its failures state themselves
    // inside the Connect card (`oauth.failure`, rendered there), a flow
    // begins only under an ensured definition, and a succeeded completion
    // lands on the new provider's page.
    const oauth = createOauthFlow({
        api: props.api,
        startAction: "start-oauth",
        completeAction: "complete-oauth",
        isSubmittable: canComplete,
        begin: async () => {
            const ensured = await ensureDefinition();

            if ("error" in ensured) {
                return {
                    status: "blocked" as const,
                    notice: ensured.error,
                };
            }

            return {
                status: "called" as const,
                outcome: await performManagementCall(() =>
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
                ),
            };
        },
        onSucceeded: (provider) => {
            navigate(hrefs.providerDetail(provider.id));
        },
    });

    // The preset switching stands below the OAuth flow it interacts with:
    // `applyPreset` discards a live flow (`oauth.reset`), so the helpers are
    // defined after the fragment they reset and the file reads top to bottom
    // with no forward reference for a reader to reconstruct.
    /** Installs the preset: its payload in the editor, the flow discarded. */
    const applyPreset = (nextPreset: ProviderPreset): void => {
        setPresetId(nextPreset.id);
        oauth.reset();

        setIsDefinitionTouched(false);
        setDefinitionJson(JSON.stringify(nextPreset.payload, null, 2));

        if (!isNameTouched()) {
            setName(suggestProviderName(nextPreset.suggestedName, takenNames()));
        }
    };

    const pickPreset = (next: string): void => {
        // The preset comes from the picker's argument, not from re-reading
        // `presetId()` — the setter's value is not immediately readable back
        // in every Solid dev runtime, and the editor's prefill must be the
        // selected preset's payload, never the previous selection's.
        const nextPreset =
            PROVIDER_PRESETS.find((candidate) => candidate.id === next) ?? PROVIDER_PRESETS[0];

        // An edited connection recipe is the operator's own work: the pick
        // arms instead of applying, and only the Preset card's confirm
        // performs the switch. An untouched definition switches instantly —
        // the common path gains no friction.
        if (isDefinitionTouched()) {
            setArmedPresetId(nextPreset.id);
            return;
        }

        applyPreset(nextPreset);
    };

    /** The confirm's action: the armed switch replaces the edited recipe. */
    const confirmPresetSwitch = (): void => {
        const armedId = armedPresetId();
        setArmedPresetId(null);

        if (armedId === null) {
            return;
        }

        applyPreset(
            PROVIDER_PRESETS.find((candidate) => candidate.id === armedId) ?? PROVIDER_PRESETS[0],
        );
    };

    /** The confirm's way back: the arming dissolves, the edit stands. */
    const cancelPresetSwitch = (): void => {
        setArmedPresetId(null);
    };

    // The static-key creation's shared action: the page's prelude ensures the
    // definition first — a refused ensure states its notice and no provider
    // call runs — and a succeeded creation lands on the new provider's page.
    const create = createCreateAction({
        action: "create-provider",
        channel: setFailure,
        canRun: () => canCreateStatic() && !oauth.isBusy(),
        perform: async () => {
            const ensured = await ensureDefinition();

            if ("error" in ensured) {
                return {
                    status: "blocked" as const,
                    notice: ensured.error,
                };
            }

            return {
                status: "called" as const,
                outcome: await performManagementCall(() =>
                    props.api.createProvider({
                        provider_definition_name: ensured.name,
                        provider_definition_version: ensured.version,
                        name: resolvedName(),
                        model_prefix: resolvedPrefix(),
                        is_enabled: true,
                        base_url: needsBaseUrl() ? trimmedBaseUrl() : null,
                        api_key: apiKey(),
                    }),
                ),
            };
        },
        onCreated: (result) => {
            navigate(hrefs.providerDetail(result.provider.id));
        },
    });

    /** Whether any creation or OAuth call is in flight: the page's shared disable. */
    const isBusy = (): boolean => create.isCreating() || oauth.isBusy();

    /**
     * Whether an OAuth flow is live: the picker, the identity fields, and the
     * definition editing surface lock, because the provider is created from
     * what the flow's start already sent — an edit now would look accepted
     * and do nothing. Each editable card states that reason in a single line
     * (`FLOW_LOCK_NOTICE`) for as long as the flow holds, so no control grays
     * out silently.
     */
    const isFlowLive = (): boolean => oauth.flow() !== null;

    // A flow going live locks the picker, so an arming could await nothing:
    // it dissolves with the lock instead of resurfacing stale when the flow
    // ends.
    createEffect(
        () => isFlowLive(),
        (isLive) => {
            if (isLive) {
                setArmedPresetId(null);
            }
        },
    );

    /** The live flow narrowed to its grant, so each step renders its own answer. */
    const startedAuthorizationCode = createMemo(() => {
        const live = oauth.flow();
        return live !== null && live.grant === "authorization_code" ? live : null;
    });

    const startedDevice = createMemo(() => {
        const live = oauth.flow();
        return live !== null && live.grant === "device" ? live : null;
    });

    const submitOauth = (event: SubmitEvent): void => {
        event.preventDefault();
        void oauth.complete();
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
                    <div class="flex flex-col items-start gap-3">
                        <Notice
                            tone={current().tone}
                            title={current().title}
                            detail={current().detail}
                            fix={current().fix}
                        />
                        {/* The failed boot read's way back, on the page it
                            failed: rerunning `readBootState` — the read
                            aborts the cycle in flight and writes only the
                            listing signals and the notice, so a live OAuth
                            flow stands through it untouched. */}
                        <Button
                            glyph="refresh"
                            tone="ghost"
                            isBusy={isBootReading()}
                            onClick={readBootState}
                        >
                            Read again
                        </Button>
                    </div>
                )}
            </Show>

            <Card>
                <CardHeader title="Preset" />
                <div class="mt-4">
                    <ProviderPresetPicker
                        presets={PROVIDER_PRESETS}
                        value={presetId()}
                        onChange={pickPreset}
                        isDisabled={isBusy() || isFlowLive()}
                    />
                    {/* The armed switch's confirm, standing where the pick
                        landed — the same grammar the delete confirmation
                        uses: a question, the action, and the way back. It is
                        the only path to `applyPreset` over an edited recipe,
                        and both buttons stand behind the page's busy guard:
                        a begin or create in flight is already ensuring the
                        edited recipe, and a switch under it would contradict
                        what the call sends. */}
                    <Show
                        when={armedPreset()}
                        keyed
                    >
                        {(armed) => (
                            <div class="mt-3 flex flex-wrap items-center gap-2">
                                <p class="w-full text-[13.5px] text-ink-dim">
                                    {`Switch to ${armed.title}? Your edited connection recipe will be replaced.`}
                                </p>
                                <Button
                                    tone="danger"
                                    isDisabled={isBusy()}
                                    onClick={confirmPresetSwitch}
                                >
                                    Switch preset
                                </Button>
                                <Button
                                    isDisabled={isBusy()}
                                    onClick={cancelPresetSwitch}
                                >
                                    Keep my edit
                                </Button>
                            </div>
                        )}
                    </Show>
                    <p class="mt-3 text-[12.5px] leading-snug text-ink-faint">
                        {preset().provenance}
                    </p>
                    <Show when={isFlowLive()}>
                        <p class="mt-3 text-[12.5px] text-ink-faint">{FLOW_LOCK_NOTICE}</p>
                    </Show>
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
                            // The claims wait for the boot read's answer:
                            // while it is in flight the page states no fact
                            // about the stored versions — the same wait the
                            // stored case's line already keeps.
                            <Show when={isStoredKnown() !== "reading"}>
                                <p class="text-[12.5px] text-ink-faint">
                                    {isStoredKnown() === "unreadable"
                                        ? "The stored versions could not be read."
                                        : "Not stored yet — submitting creates version 1."}
                                </p>
                            </Show>
                        }
                    >
                        {(current) => (
                            <p class="text-[12.5px] text-ink-faint">
                                {`Stored as version ${current().version}.`}
                            </p>
                        )}
                    </Show>
                    <Show when={isFlowLive()}>
                        <p class="mt-3 text-[12.5px] text-ink-faint">{FLOW_LOCK_NOTICE}</p>
                    </Show>
                    <details
                        class="group mt-2"
                        onToggle={(event) => setIsDefinitionOpen(event.currentTarget.open)}
                    >
                        <summary class="flex cursor-pointer list-none items-center gap-1.5 text-[13px] font-medium text-ink-dim transition-colors duration-150 ease-out hover:text-ink [&::-webkit-details-marker]:hidden">
                            <Glyph
                                name="chev-right"
                                class="mt-0.5 shrink-0 transition-transform duration-150 ease-out group-open:rotate-90"
                            />
                            Advanced: the connection recipe — prefilled by the preset
                        </summary>
                        <div class="mt-3">
                            <textarea
                                aria-label="Definition JSON"
                                class="tnum h-80 w-full rounded-lg border border-rule bg-canvas p-3 font-mono text-[12.5px] leading-relaxed transition-colors duration-150 ease-out disabled:cursor-not-allowed disabled:opacity-60 dark:bg-muted"
                                spellcheck={false}
                                value={definitionJson()}
                                onInput={(event) => {
                                    setDefinitionJson(event.currentTarget.value);
                                    setIsDefinitionTouched(true);
                                }}
                                disabled={isBusy() || isFlowLive()}
                            />
                            <Show when={definitionError()}>
                                {(current) => (
                                    <p class="mt-1 text-[12.5px] text-red-600 dark:text-red-400">
                                        {current()}
                                    </p>
                                )}
                            </Show>
                            <div class="mt-2 flex justify-end">
                                <Button
                                    isDisabled={isBusy() || isFlowLive()}
                                    onClick={resetDefinition}
                                >
                                    Reset to preset
                                </Button>
                            </div>
                        </div>
                    </details>
                    {/* The pointer stands only while the disclosure hides
                        the path error: open, the disclosure states the
                        offense itself, and a pointer telling the operator to
                        open it would be impossible to follow. */}
                    <Show when={definitionError() !== null && !isDefinitionOpen()}>
                        <p class="mt-2 text-[12.5px] text-red-600 dark:text-red-400">
                            The connection recipe is invalid — open the Advanced disclosure above
                            for the offending path.
                        </p>
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
                        isDisabled={isBusy() || isFlowLive()}
                    />
                </div>
                <Show when={isFlowLive()}>
                    <p class="mt-3 text-[12.5px] text-ink-faint">{FLOW_LOCK_NOTICE}</p>
                </Show>
            </Card>

            <Switch>
                <Match when={preset().grant !== null}>
                    <Card>
                        <CardHeader
                            title="Connect the account"
                            description="Approve access where the flow points, then let it complete. This app never stores the credentials."
                        />
                        {/* The flow's failures state themselves where its step
                            and buttons stand — a refused begin or exchange, a
                            failed outcome poll, a flow's expiry — never on
                            the page's read-level notice above the cards. */}
                        <Show when={oauth.failure()}>
                            {(current) => (
                                <div class="mt-4">
                                    <Notice
                                        tone="bad"
                                        title={current().title}
                                        detail={current().detail}
                                        fix={current().fix}
                                    />
                                </div>
                            )}
                        </Show>
                        <Show
                            when={oauth.flow() === null}
                            fallback={
                                <div class="mt-4">
                                    <Show
                                        when={startedAuthorizationCode()}
                                        keyed
                                        fallback={
                                            <Show
                                                when={startedDevice()}
                                                keyed
                                            >
                                                {(deviceFlow) => (
                                                    <DeviceStep
                                                        flow={deviceFlow}
                                                        onComplete={oauth.completeDevice}
                                                        onStartOver={oauth.reset}
                                                    />
                                                )}
                                            </Show>
                                        }
                                    >
                                        {(authorizationFlow) => (
                                            <form
                                                novalidate={true}
                                                class="flex max-w-2xl flex-col gap-4"
                                                onSubmit={submitOauth}
                                            >
                                                <AuthorizeStep
                                                    flow={authorizationFlow}
                                                    onStartOver={oauth.reset}
                                                >
                                                    <TextField
                                                        label="Callback URL or authorization code"
                                                        value={oauth.code()}
                                                        onInput={oauth.setCode}
                                                        placeholder="Paste the whole callback URL"
                                                        isDisabled={oauth.isCompleting()}
                                                        error={oauth.pasteError()}
                                                    />
                                                    <div>
                                                        <Button
                                                            type="submit"
                                                            tone="primary"
                                                            isDisabled={!canComplete()}
                                                            isBusy={oauth.isCompleting()}
                                                        >
                                                            {oauth.isCompleting()
                                                                ? "Connecting…"
                                                                : "Connect the account"}
                                                        </Button>
                                                    </div>
                                                </AuthorizeStep>
                                            </form>
                                        )}
                                    </Show>
                                </div>
                            }
                        >
                            <div class="mt-4">
                                <Show when={needsBaseUrl()}>
                                    {/* The edited recipe addresses the base,
                                        so the flow start needs the URL it
                                        stores — the static path's field, the
                                        requirement stated quietly from first
                                        paint and the error only once typed. */}
                                    <div class="mb-4 max-w-2xl">
                                        <TextField
                                            label="Upstream base URL"
                                            type="url"
                                            value={baseUrl()}
                                            onInput={inputBaseUrl}
                                            placeholder="https://upstream.example.com/v1"
                                            isDisabled={isBusy()}
                                            error={baseUrlError()}
                                            hint={requiredHint(
                                                "The upstream address this definition's requests evaluate against — the gateway refuses a flow start without one.",
                                            )}
                                        />
                                    </div>
                                </Show>
                                <Button
                                    tone="primary"
                                    isDisabled={!canComplete()}
                                    isBusy={oauth.isStarting()}
                                    onClick={() => void oauth.start()}
                                >
                                    {oauth.isStarting() ? "Starting…" : "Start the authorization"}
                                </Button>
                            </div>
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
                            onSubmit={create.submit}
                        >
                            <CompatibleConnectionFields
                                hasBaseUrl={needsBaseUrl()}
                                baseUrl={baseUrl()}
                                apiKey={apiKey()}
                                onBaseUrlInput={inputBaseUrl}
                                onApiKeyInput={setApiKey}
                                baseUrlError={baseUrlError()}
                                apiKeyError={apiKeyError()}
                                isDisabled={create.isCreating()}
                            />
                            <div>
                                <Button
                                    type="submit"
                                    tone="primary"
                                    isDisabled={!canCreateStatic()}
                                    isBusy={create.isCreating()}
                                >
                                    {create.isCreating() ? "Creating…" : "Create provider"}
                                </Button>
                            </div>
                        </form>
                    </Card>
                </Match>
            </Switch>
        </div>
    );
}
