import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { readProviders } from "/src/features/inventory/read";
import {
    CompactionSupportOverrideFields,
    createCapabilityOverrideDraft,
    ThinkingLevelsOverrideFields,
    TokenCapOverrideFields,
} from "/src/features/models/model-form";
import { translation } from "/src/i18n";
import { toCreateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { createCreateAction } from "/src/screen-state/create-form";
import type { ScreenNotice } from "/src/screen-state/notices";
import { createScreenRead, readOne } from "/src/screen-state/read";
import {
    Button,
    ButtonLink,
    entitySelectOptions,
    SelectField,
    Switch,
    TextField,
} from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ReadNotice, StatedNotice } from "/src/ui/read-notice";

/**
 * One creation page for a manually added model. When opened from a
 * provider's page the provider is fixed — there is nothing to pick — and
 * otherwise it is picked once, because the model belongs to it. The
 * settings match the edit page field for field. Creating lands on the new
 * model's page, like a provider creation does. The same shape claims back a
 * model the upstream no longer reports: naming its provider and upstream ID
 * resumes service under the requested state instead of inserting a new row —
 * a link from that model's page carries its upstream ID here, prefilled.
 */
export function ModelNewScreen(props: {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
    readonly upstreamId: string | null;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);

    const read = createScreenRead({
        load: async (signal) => {
            const providersResult = await readOne(signal, readProviders(props.api));

            setProviders(providersResult.providers);
        },
    });

    const fixedProvider = createMemo(() =>
        props.providerFilter === null
            ? null
            : (providers().find((row) => row.id === props.providerFilter) ?? null),
    );

    const backHref = (): string => {
        const fixed = fixedProvider();

        return fixed === null ? hrefs.models(props.providerFilter) : hrefs.providerDetail(fixed.id);
    };
    const backLabel = (): string =>
        fixedProvider() === null
            ? translation()(($) => $.screens.modelNewScreen.models)
            : translation()(($) => $.screens.modelNewScreen.provider);

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.modelNewScreen.createModel)}
                description={translation()(
                    ($) =>
                        $.screens.modelNewScreen.somethingDiscoveryWouldNotProduceOrAModelMessage,
                )}
                backHref={backHref()}
                backLabel={backLabel()}
            />

            <ReadNotice
                read={read}
                retry={read.reload}
            />

            <Show when={read.isFirstRead()}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead()}>
                <Show
                    when={providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title={translation()(($) => $.screens.modelNewScreen.noProvidersYet)}
                            description={translation()(
                                ($) =>
                                    $.screens.modelNewScreen
                                        .aModelBelongsToAProviderAndThereMessage,
                            )}
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                {translation()(($) => $.screens.modelNewScreen.createProvider)}
                            </ButtonLink>
                        </EmptyState>
                    }
                >
                    <ModelCreateForm
                        api={props.api}
                        providers={providers()}
                        initialProviderId={props.providerFilter}
                        fixedProvider={fixedProvider()}
                        initialUpstreamId={props.upstreamId}
                    />
                </Show>
            </Show>
        </div>
    );
}

function ModelCreateForm(props: {
    readonly api: ManagementApi;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly initialProviderId: number | null;
    readonly fixedProvider: ProviderResponse | null;
    /** The upstream ID the route carried (a vanished model's claim link): prefilled, never forced. */
    readonly initialUpstreamId: string | null;
}): JSX.Element {
    const initialSelection = (): string => {
        if (props.fixedProvider !== null) {
            return String(props.fixedProvider.id);
        }

        if (
            props.initialProviderId !== null &&
            props.providers.some((row) => row.id === props.initialProviderId)
        ) {
            return String(props.initialProviderId);
        }

        return props.providers.length === 1 && props.providers[0] !== undefined
            ? String(props.providers[0].id)
            : "";
    };

    const [providerId, setProviderId] = createSignal(initialSelection());
    const [upstreamId, setUpstreamId] = createSignal(props.initialUpstreamId ?? "");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const draft = createCapabilityOverrideDraft(null);
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    const selectedProvider = createMemo(
        () => props.providers.find((row) => String(row.id) === providerId()) ?? null,
    );

    const trimmedUpstream = (): string => upstreamId().trim();

    /** Inline composition errors, shown where the value was typed (gateway README §7). */
    const upstreamError = (): string | null => routingTokenError(trimmedUpstream(), "upstreamId");

    /** The creation's composition rules: an owned provider with a routable identity. */
    const canCompose = (): boolean =>
        selectedProvider() !== null &&
        trimmedUpstream() !== "" &&
        upstreamError() === null &&
        draft.maxInputError() === null &&
        draft.maxOutputError() === null;

    const create = createCreateAction({
        action: "create-model",
        channel: setFailure,
        canRun: canCompose,
        perform: async () => {
            // The submit gate holds a selected provider before this runs;
            // the narrow restates the gate for the compiler, and a null here
            // is a broken gate — fatal, never a state to recover from.
            const provider = selectedProvider();

            if (provider === null) {
                throw new Error("The model creation gate passed without a selected provider.");
            }

            return {
                status: "called" as const,
                outcome: await performManagementCall(() =>
                    props.api.createModel(
                        toCreateModelRequest(provider.id, trimmedUpstream(), {
                            is_enabled: isEnabled(),
                            thinking_levels_override: draft.normalizedLevels(),
                            max_input_tokens_override: draft.normalizedMaxInput(),
                            max_output_tokens_override: draft.normalizedMaxOutput(),
                            compaction_support_override: draft.normalizedCompaction(),
                        }),
                    ),
                ),
            };
        },
        onCreated: (result) => {
            navigate(hrefs.modelDetail(result.model.id));
        },
    });

    const canCreate = (): boolean => canCompose() && !create.isCreating();

    const providerOptions = createMemo(() =>
        entitySelectOptions(
            props.providers,
            (row) => row.name,
            translation()(($) => $.remainingText.chooseAProvider),
        ),
    );

    return (
        <>
            <StatedNotice notice={failure} />

            <Notice
                tone="warn"
                title={translation()(
                    ($) => $.screens.modelNewScreen.aModelAddedByHandIsNeverRemovedMessage,
                )}
                detail={translation()(
                    ($) => $.remainingText.namingAProviderAndUpstreamIDTheUpstreamNoLonger,
                )}
            />

            {/* One form spans both cards, so pressing Enter in the identity
            fields submits the creation like in any text field of a form. */}
            <form
                novalidate={true}
                class="flex flex-col gap-5"
                onSubmit={create.submit}
            >
                <Card>
                    <CardHeader title={translation()(($) => $.screens.modelNewScreen.identity)} />
                    <div class="mt-4 flex max-w-2xl flex-col gap-4">
                        <Show
                            when={props.fixedProvider}
                            fallback={
                                <SelectField
                                    label={translation()(($) => $.screens.modelNewScreen.provider)}
                                    value={providerId()}
                                    options={providerOptions()}
                                    onChange={setProviderId}
                                    isDisabled={create.isCreating()}
                                    hint={translation()(
                                        ($) =>
                                            $.additional.modelNewScreen
                                                .aModelBelongsToOneProviderMessage,
                                    )}
                                />
                            }
                        >
                            {(fixed) => (
                                <div>
                                    <p class="block text-[13px] font-medium text-ink">
                                        {translation()(($) => $.screens.modelNewScreen.provider)}
                                    </p>
                                    <p class="tnum mt-1.5">{fixed().name}</p>
                                </div>
                            )}
                        </Show>
                        <TextField
                            label={translation()(($) => $.screens.modelNewScreen.upstreamID)}
                            value={upstreamId()}
                            onInput={setUpstreamId}
                            placeholder="gpt-4o"
                            isDisabled={create.isCreating()}
                            error={upstreamError()}
                            hint={
                                props.initialUpstreamId === null
                                    ? translation()(
                                          ($) =>
                                              $.additional.modelNewScreen
                                                  .sentUpstreamVerbatimLettersNumbersAndSymbolsOnlyMessage,
                                      )
                                    : translation()(
                                          ($) =>
                                              $.additional.modelNewScreen
                                                  .carriedHereFromAModelTheUpstreamNoMessage,
                                      )
                            }
                        />
                    </div>
                </Card>

                <Card>
                    <CardHeader
                        title={translation()(($) => $.screens.modelNewScreen.modelSettings)}
                    />
                    <div class="mt-4 flex max-w-2xl flex-col gap-5">
                        <Switch
                            label={translation()(($) => $.screens.modelNewScreen.enabled)}
                            isOn={isEnabled()}
                            isDisabled={create.isCreating()}
                            onToggle={setIsEnabled}
                        />

                        <ThinkingLevelsOverrideFields
                            hasOverride={draft.hasLevelsOverride()}
                            onHasOverrideToggle={draft.setHasLevelsOverride}
                            selected={draft.selectedLevels()}
                            onSelectedChange={draft.setSelectedLevels}
                            discovered={null}
                            isDisabled={create.isCreating()}
                        />

                        <TokenCapOverrideFields
                            switchLabel={translation()(
                                ($) => $.remainingText.overrideMaxInputTokens,
                            )}
                            fieldLabel={translation()(
                                ($) => $.remainingText.maxInputTokensOverride,
                            )}
                            hasOverride={draft.hasMaxInputOverride()}
                            onHasOverrideToggle={draft.setHasMaxInputOverride}
                            value={draft.maxInputText()}
                            onValueInput={draft.setMaxInputText}
                            discovered={null}
                            valueError={draft.maxInputError()}
                            isDisabled={create.isCreating()}
                        />

                        <TokenCapOverrideFields
                            switchLabel={translation()(
                                ($) => $.remainingText.overrideMaxOutputTokens,
                            )}
                            fieldLabel={translation()(
                                ($) => $.remainingText.maxOutputTokensOverride,
                            )}
                            hasOverride={draft.hasMaxOutputOverride()}
                            onHasOverrideToggle={draft.setHasMaxOutputOverride}
                            value={draft.maxOutputText()}
                            onValueInput={draft.setMaxOutputText}
                            discovered={null}
                            valueError={draft.maxOutputError()}
                            isDisabled={create.isCreating()}
                        />

                        <CompactionSupportOverrideFields
                            hasOverride={draft.hasCompactionOverride()}
                            onHasOverrideToggle={draft.setHasCompactionOverride}
                            selected={draft.selectedCompaction()}
                            onSelectedChange={draft.setSelectedCompaction}
                            discovered={null}
                            isDisabled={create.isCreating()}
                        />

                        <div>
                            <Button
                                type="submit"
                                tone="primary"
                                isDisabled={!canCreate()}
                                isBusy={create.isCreating()}
                            >
                                {create.isCreating()
                                    ? translation()(($) => $.additional.modelNewScreen.creating)
                                    : translation()(($) => $.additional.modelNewScreen.createModel)}
                            </Button>
                        </div>
                    </div>
                </Card>
            </form>
        </>
    );
}
