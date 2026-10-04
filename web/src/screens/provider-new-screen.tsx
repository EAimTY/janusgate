import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Match, Show, Switch } from "solid-js";
import { translation } from "/src/i18n";
import { switchPresetMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import {
    type DefinitionPayloadResult,
    hasBaseReference,
    validateDefinitionPayload,
} from "/src/management-api/definition-payload";
import { isAbortFailure } from "/src/management-api/errors";
import type { DefinitionResponse } from "/src/management-api/generated/v0";
import {
    baseUrlMessage,
    bearerSecretError,
    isBaseUrl,
    isBearerSecret,
    isRoutingToken,
    routingTokenError,
} from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import {
    matchStoredDefinition,
    PROVIDER_PRESETS,
    type ProviderPreset,
    presetDefinitionJson,
} from "/src/presets";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { Button, TextField } from "/src/ui/controls";
import { Glyph } from "/src/ui/glyph";
import { createInlineConfirmFocus } from "/src/ui/inline-confirm";
import { InlineConfirmQuestion } from "/src/ui/inline-confirm-question";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import {
    CompatibleConnectionFields,
    ProviderIdentityFields,
    ProviderPresetPicker,
} from "/src/ui/provider-form";
import { createCreateAction } from "./create-form";
import { createOauthFlow } from "./oauth-flow";
import { CompletionForm, DeviceStep } from "./oauth-flow-view";
import { suggestProviderName } from "./provider-defaults";
import {
    createStandingNotice,
    ReadCallFailure,
    readAll,
    readProviderDefinitions,
    readProviders,
    type ScreenNotice,
    screenFailureNotice,
} from "./read";
import { StatedNotice } from "./read-notice";

/**
 * The one line each editable card states while a live flow holds its fields:
 * the lock's reason, stated once per card rather than as a hint on every
 * disabled control.
 */
const flowLockNotice = (): string =>
    translation()(
        ($) =>
            $.helpers.providerNewScreen
                .lockedWhileTheAuthorizationIsLiveTheProviderIsCreatedFromWhatTheFlows,
    );

/**
 * The lock notice's one rendering: the quiet-ink paragraph stating
 * `flowLockNotice()`, whose wording's design rule — stated once per card
 * rather than as a hint on every disabled control — lives at the constant.
 * Callers own the gate: live-flow sites wrap this in `Show when={isFlowLive()}`,
 * and a site already inside a live branch renders it bare.
 */
function FlowLockNotice(): JSX.Element {
    return <p class="mt-3 text-[12.5px] text-ink-faint">{flowLockNotice()}</p>;
}

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
    /**
     * Whether a boot cycle answered whole, establishing what is stored: a
     * cycle in which either call fails installs nothing, so the claims stay
     * withheld until a whole cycle — the failure notice being the only
     * state a failed cycle leaves.
     */
    const [isStoredKnown, setIsStoredKnown] = createSignal(false);
    const [name, setName] = createSignal("");
    const [modelPrefix, setModelPrefix] = createSignal<string | null>(null);
    const resolvedPrefix = (): string =>
        modelPrefix() ?? `${isRoutingToken(resolvedName()) ? resolvedName() : suggestion()}-`;
    const [isNameTouched, setIsNameTouched] = createSignal(false);
    const [takenNames, setTakenNames] = createSignal<ReadonlySet<string>>(new Set());
    const [baseUrl, setBaseUrl] = createSignal("");
    // The base URL's error is stated only once the operator has typed into
    // the field — a pristine form is not an error, a required-but-empty field
    // is not yet a mistake (the api-key forms' untouched-name gate).
    const [hasBaseUrlInput, setHasBaseUrlInput] = createSignal(false);
    const [apiKey, setApiKey] = createSignal("");
    // The page's standing notice under the shared two-origin rule
    // (`createStandingNotice` in `./read`): the boot read's failures carry
    // the read origin and the static creation path's outcomes the action
    // one, so a boot cycle that answers clears only the former and the
    // creation's own clear only the latter. The OAuth flow's failures never
    // reach it — the flow fragment states its own inside the Connect card —
    // so the channels cannot render one outcome twice.
    const failure = createStandingNotice();
    /** Whether the boot read's cycle is in flight — the read-again button's busy state. */
    const [isBootReading, setIsBootReading] = createSignal(true);

    /** The catalog preset of the given id, falling back to the first entry. */
    const presetById = (id: string): ProviderPreset =>
        PROVIDER_PRESETS.find((candidate) => candidate.id === id) ?? PROVIDER_PRESETS[0];

    const preset = (): ProviderPreset => presetById(presetId());

    /**
     * The latest stored version of this preset's definition, when the gateway
     * holds one — the listing orders by name then version, so the last of a
     * name is the newest.
     */
    const stored = createMemo(() => matchStoredDefinition(preset(), definitions()));

    /** The definition JSON the operator edits; prefilled from the preset. */
    const [definitionJson, setDefinitionJson] = createSignal(presetDefinitionJson(preset()));
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
    // Focus travels with the arming: the pick's confirm takes focus from the
    // picker, and every disarm that answers a pick hands it back — the
    // shared inline-confirm contract (`/src/ui/inline-confirm`). A dissolve
    // no pick asked for (a reset to the preset, a flow going live) calls
    // nothing: focus stands outside the armed branch there, where the user
    // put it.
    const confirmFocus = createInlineConfirmFocus();

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
     * the failure notice reruns them in place (README §6.3).
     *
     * Both calls run through the shared `readAll` under the cycle's signal,
     * so the cycle settles once: both must answer for it to count as read.
     * A cycle in which either call fails installs nothing — no listing, no
     * stored-known stamp, no name seed — its diagnosis standing on the
     * notice with the read origin, Read again retrying the whole cycle. A
     * whole cycle installs both results, clears only the read-origin failure
     * (a creation diagnosis the cycle never retried survives the re-read —
     * the two-origin rule), and stamps the shell. The cycle writes only the
     * listing signals and the notice, never the live OAuth flow's state:
     * neither rerun may unlock or disturb a flow in flight.
     */
    const readBootState = (): void => {
        bootController?.abort();

        const cycle = new AbortController();
        bootController = cycle;
        setIsBootReading(true);

        void readAll(
            cycle.signal,
            readProviders(props.api),
            readProviderDefinitions(props.api),
        ).then(
            ([providers, definitions]) => {
                if (cycle.signal.aborted) {
                    // Superseded after the reads resolved but before this
                    // continuation ran: the newer cycle owns the page —
                    // nothing installs, not even the in-flight lift.
                    return;
                }

                setIsBootReading(false);

                const taken = new Set(providers.providers.map((row) => row.name));
                setTakenNames(taken);
                setDefinitions(definitions.provider_definitions);
                setIsStoredKnown(true);

                if (!isNameTouched()) {
                    // The seed computes over the local `taken`, not the
                    // `suggestion` memo: a write from this continuation
                    // stages until the next flush, so a read-back of the
                    // memo would still see the committed empty set and
                    // seed a taken base name without its `-N` suffix.
                    setName(suggestProviderName(preset().suggestedName, taken));
                }

                // The cycle read: it clears only the read-origin failure
                // — a creation diagnosis the cycle never retried
                // survives the re-read (the two-origin rule).
                failure.clearOfOrigin("read");
                reportRefresh();
            },
            (error: unknown): void => {
                if (isAbortFailure(error) || cycle.signal.aborted) {
                    // Superseded or disposed: an aborted cycle installs
                    // nothing — not even its diagnosis, which belongs to
                    // a cycle the page has already replaced.
                    return;
                }

                setIsBootReading(false);

                if (!(error instanceof ReadCallFailure)) {
                    // `readAll` rejects with nothing else; anything here
                    // is a bug and stays loud, the shared harness's own
                    // contract for what escapes a load.
                    throw error;
                }

                // The cycle failed: its diagnosis replaces whatever
                // stands — the newest cycle's failure is the current
                // fact, and the read-origin stamp brings the Read-again
                // recovery with it.
                failure.state("read", error.notice);
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

    /** The name the request carries: what was typed, or the suggestion when the field stands empty. */
    const resolvedName = createMemo(() => (name().trim() === "" ? suggestion() : name().trim()));

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

    const apiKeyError = (): string | null => bearerSecretError(apiKey(), "apiKey");

    /**
     * An OAuth start or completion is submittable with a valid definition, a
     * account name, and — when the definition addresses the base — a valid
     * base URL, so the gateway's 400 naming `base_url` never has a chance to
     * leave the browser.
     */
    const canComplete = (): boolean =>
        (resolvedPrefix() === "" || isRoutingToken(resolvedPrefix())) &&
        definitionError() === null &&
        (!needsBaseUrl() || isBaseUrl(trimmedBaseUrl()));

    /** A static-key creation is submittable under the shared gate with a complete connection. */
    const canCreateStatic = (): boolean => canComplete() && isBearerSecret(apiKey());

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
                    get title() {
                        return translation()(($) => $.operationNotices.invalidDefinition);
                    },
                    detail: parsed.error,
                    get fix() {
                        return translation()(($) => $.operationNotices.fixDefinition);
                    },
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

    const inputName = (value: string): void => {
        setIsNameTouched(true);
        setName(value);
    };

    /** Restores the selected preset's payload and re-arms the stored-version reuse. */
    const resetDefinition = (): void => {
        // With the edit gone there is nothing the preset-switch arming
        // guards, so it goes with the edit.
        setArmedPresetId(null);
        setDefinitionJson(presetDefinitionJson(preset()));
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
        setDefinitionJson(presetDefinitionJson(nextPreset));

        if (!isNameTouched()) {
            // The seed computes over the picked preset's own suggestion — the
            // same local-argument defense the pick's payload prefill uses —
            // not the `suggestion` memo: a write from this switch stages
            // until the next flush, so a read-back of the memo would still
            // see the previous preset's suggestion.
            setName(suggestProviderName(nextPreset.suggestedName, takenNames()));
        }
    };

    const pickPreset = (next: string): void => {
        // The preset comes from the picker's argument, not from re-reading
        // `presetId()` — the setter's value is not immediately readable back
        // in every Solid dev runtime, and the editor's prefill must be the
        // selected preset's payload, never the previous selection's.
        const nextPreset = presetById(next);

        // An edited connection recipe is the operator's own work: the pick
        // arms instead of applying, and only the Preset card's confirm
        // performs the switch. An untouched definition switches instantly —
        // the common path gains no friction.
        if (isDefinitionTouched()) {
            confirmFocus.arm();
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

        confirmFocus.disarm();
        applyPreset(presetById(armedId));
    };

    /** The confirm's way back: the arming dissolves, the edit stands. */
    const cancelPresetSwitch = (): void => {
        confirmFocus.disarm();
        setArmedPresetId(null);
    };

    /**
     * The static creation path's failure channel: a stated notice stands
     * with the action's origin, and the action's own clear — the pre-run
     * `null` — lifts only an action-origin failure, never the boot read's
     * (the two-origin rule: an action states its own or clears its own).
     */
    const reportCreationFailure = (notice: ScreenNotice | null): void => {
        if (notice === null) {
            failure.clearOfOrigin("action");
            return;
        }

        failure.state("action", notice);
    };

    // The static-key creation's shared action: the page's prelude ensures the
    // definition first — a refused ensure states its notice and no provider
    // call runs — and a succeeded creation lands on the new provider's page.
    const create = createCreateAction({
        action: "create-provider",
        channel: reportCreationFailure,
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
     * Whether an OAuth flow is live: the picker, the identity fields, the
     * definition editing surface, and the base-URL field lock, because the
     * provider is created from what the flow's start already sent — an edit
     * now would look accepted and do nothing. Each editable card states that
     * reason in a single line (`flowLockNotice()`) for as long as the flow
     * holds — the Connect card's line standing beside its one locked field —
     * so no control grays out silently.
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

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.providerNewScreen.createProvider)}
                description={translation()(
                    ($) =>
                        $.screens.providerNewScreen
                            .oneUpstreamAccountPickThePresetTheCredentialMessage,
                )}
                backHref={hrefs.providers()}
                backLabel="Providers"
            />

            <StatedNotice
                notice={failure.notice}
                class="flex flex-col items-start gap-3"
            >
                {/* The failed boot read's way back, on the page it
                    failed — and only there: an action-origin failure
                    is not the boot read's to recover, and a click
                    here would rerun a read whose success clears only
                    a read failure, leaving the action's diagnosis
                    standing while retrying nothing. The rerun aborts
                    the cycle in flight and writes only the listing
                    signals and the notice, so a live OAuth flow
                    stands through it untouched. */}
                <Show when={failure.origin() === "read"}>
                    <Button
                        glyph="refresh"
                        tone="ghost"
                        isBusy={isBootReading()}
                        onClick={readBootState}
                    >
                        {translation()(($) => $.screens.providerNewScreen.readAgain)}
                    </Button>
                </Show>
            </StatedNotice>

            <Card>
                <CardHeader title={translation()(($) => $.screens.providerNewScreen.preset)} />
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
                                <InlineConfirmQuestion
                                    question={switchPresetMessage(armed.title)}
                                    focusArmed={confirmFocus.focusArmed}
                                />
                                <Button
                                    tone="danger"
                                    isDisabled={isBusy()}
                                    onClick={confirmPresetSwitch}
                                >
                                    {translation()(($) => $.screens.providerNewScreen.switchPreset)}
                                </Button>
                                <Button
                                    isDisabled={isBusy()}
                                    onClick={cancelPresetSwitch}
                                >
                                    {translation()(($) => $.screens.providerNewScreen.keepMyEdit)}
                                </Button>
                            </div>
                        )}
                    </Show>
                    <p class="mt-3 text-[12.5px] leading-snug text-ink-faint">
                        {preset().provenance}
                    </p>
                    <Show when={isFlowLive()}>
                        <FlowLockNotice />
                    </Show>
                </div>
            </Card>

            <Card>
                <CardHeader title={translation()(($) => $.screens.providerNewScreen.identity)} />
                <div class="mt-4 flex max-w-2xl flex-col gap-4">
                    <ProviderIdentityFields
                        name={name()}
                        namePlaceholder={suggestion()}
                        modelPrefix={resolvedPrefix()}
                        onModelPrefixInput={setModelPrefix}
                        prefixError={routingTokenError(resolvedPrefix(), "modelPrefix")}
                        onNameInput={inputName}
                        isDisabled={isBusy() || isFlowLive()}
                    />
                </div>
                <Show when={isFlowLive()}>
                    <FlowLockNotice />
                </Show>
            </Card>

            {/* The credential path follows the preset's grant: an OAuth
                grant renders the Connect card, and the grant-less static
                path — the only other case — is the Switch's fallback. */}
            <Switch
                fallback={
                    <Card>
                        <CardHeader
                            title={translation()(($) => $.screens.providerNewScreen.connection)}
                            description={translation()(
                                ($) =>
                                    $.screens.providerNewScreen
                                        .theKeyIsWriteonlyTheGatewayNeverReturnsMessage,
                            )}
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
                                    {create.isCreating()
                                        ? translation()(
                                              ($) => $.additional.providerNewScreen.creating,
                                          )
                                        : translation()(
                                              ($) => $.additional.providerNewScreen.createProvider,
                                          )}
                                </Button>
                            </div>
                        </form>
                    </Card>
                }
            >
                <Match when={preset().grant !== null}>
                    <Card>
                        <CardHeader
                            title={translation()(
                                ($) => $.screens.providerNewScreen.connectTheAccount,
                            )}
                            description={translation()(
                                ($) =>
                                    $.screens.providerNewScreen
                                        .approveAccessWhereTheFlowPointsThenLetMessage,
                            )}
                        />
                        {/* The flow's failures state themselves where its step
                            and buttons stand — a refused begin or exchange, a
                            failed outcome poll, a flow's expiry — never on
                            the page's read-level notice above the cards. */}
                        <StatedNotice
                            notice={oauth.failure}
                            class="mt-4"
                        />
                        <Show
                            when={oauth.flow() === null}
                            fallback={
                                <div class="mt-4">
                                    {/* The flow is live and the recipe
                                        addresses the base: the field stays
                                        on the page carrying the value the
                                        flow's start sent, disabled under
                                        the same single-line lock reason
                                        every locked card states — what the
                                        start sent stays readable without
                                        being editable. */}
                                    <Show when={needsBaseUrl()}>
                                        <div class="mb-4 max-w-2xl">
                                            <TextField
                                                label={translation()(
                                                    ($) =>
                                                        $.screens.providerNewScreen.upstreamBaseURL,
                                                )}
                                                type="url"
                                                value={baseUrl()}
                                                onInput={inputBaseUrl}
                                                placeholder="https://upstream.example.com/v1"
                                                isDisabled={true}
                                            />
                                        </div>
                                        <FlowLockNotice />
                                    </Show>
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
                                            <CompletionForm
                                                flow={oauth}
                                                startedFlow={authorizationFlow}
                                                busyLabel={translation()(
                                                    ($) =>
                                                        $.additional.providerNewScreen.connecting,
                                                )}
                                                submitLabel={translation()(
                                                    ($) =>
                                                        $.additional.providerNewScreen
                                                            .connectTheAccount,
                                                )}
                                                isSubmittable={canComplete}
                                                class="flex max-w-2xl flex-col gap-4"
                                            />
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
                                            label={translation()(
                                                ($) => $.screens.providerNewScreen.upstreamBaseURL,
                                            )}
                                            type="url"
                                            value={baseUrl()}
                                            onInput={inputBaseUrl}
                                            placeholder="https://upstream.example.com/v1"
                                            isDisabled={isBusy()}
                                            error={baseUrlError()}
                                            hint={translation()(
                                                ($) =>
                                                    $.additional.providerNewScreen
                                                        .theUpstreamAddressThisDefinitionsRequestsEvaluateAgainstMessage,
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
                                    {oauth.isStarting()
                                        ? translation()(
                                              ($) => $.additional.providerNewScreen.starting,
                                          )
                                        : translation()(
                                              ($) =>
                                                  $.additional.providerNewScreen
                                                      .startTheAuthorization,
                                          )}
                                </Button>
                            </div>
                        </Show>
                    </Card>
                </Match>
            </Switch>

            {/* The definition stands last, below the required fields: it is
                the advanced surface — prefilled by the preset and rarely
                opened — while the identity and credential fields above are
                the path every creation walks. The card's placement is
                presentation only: every gate derives from the definition
                payload memo, never from the DOM order. */}
            <Card>
                <CardHeader
                    title={translation()(($) => $.screens.providerNewScreen.definition)}
                    description={translation()(
                        ($) =>
                            $.screens.providerNewScreen
                                .theConnectionRecipeTheGatewayStoresEditingTheMessage,
                    )}
                />
                <div class="mt-4">
                    <Show
                        when={stored()}
                        fallback={
                            // The claims wait for a boot cycle that answers
                            // whole: while the read is in flight — and after
                            // a cycle that failed, which installs nothing —
                            // the page states no fact about the stored
                            // versions; the failure notice is the only state
                            // a failed cycle leaves.
                            <Show when={isStoredKnown()}>
                                <p class="text-[12.5px] text-ink-faint">
                                    {translation()(
                                        ($) =>
                                            $.screens.providerNewScreen
                                                .notStoredYetSubmittingCreatesVersion1Message,
                                    )}
                                </p>
                            </Show>
                        }
                    >
                        {(current) => (
                            <p class="text-[12.5px] text-ink-faint">
                                {translation()(($) => $.domainMessages.version, {
                                    version: current().version,
                                })}
                            </p>
                        )}
                    </Show>
                    <Show when={isFlowLive()}>
                        <FlowLockNotice />
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
                            {translation()(
                                ($) =>
                                    $.screens.providerNewScreen
                                        .advancedTheConnectionRecipePrefilledByThePreset,
                            )}
                        </summary>
                        <div class="mt-3">
                            <textarea
                                dir="ltr"
                                aria-label={translation()(
                                    ($) => $.screens.providerNewScreen.definitionJSON,
                                )}
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
                                    <p class="mt-1 text-[12.5px] text-bad">{current()}</p>
                                )}
                            </Show>
                            <div class="mt-2 flex justify-end">
                                <Button
                                    isDisabled={isBusy() || isFlowLive()}
                                    onClick={resetDefinition}
                                >
                                    {translation()(
                                        ($) => $.screens.providerNewScreen.resetToPreset,
                                    )}
                                </Button>
                            </div>
                        </div>
                    </details>
                    {/* The pointer stands only while the disclosure hides
                        the path error: open, the disclosure states the
                        offense itself, and a pointer telling the operator to
                        open it would be impossible to follow. */}
                    <Show when={definitionError() !== null && !isDefinitionOpen()}>
                        <p class="mt-2 text-[12.5px] text-bad">
                            {translation()(
                                ($) =>
                                    $.screens.providerNewScreen
                                        .theConnectionRecipeIsInvalidOpenTheAdvancedMessage,
                            )}
                        </p>
                    </Show>
                </div>
            </Card>
        </div>
    );
}
