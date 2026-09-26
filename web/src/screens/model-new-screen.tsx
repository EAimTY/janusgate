import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, Show } from "solid-js";
import { toCreateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ProviderResponse } from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { ReadNotice } from "/src/screens/read-notice";
import { Button, ButtonLink, SelectField, Switch, TextField } from "/src/ui/controls";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import {
    composedPublicId,
    ModelIdOverrideFields,
    normalizedPublicIdOverride,
} from "/src/ui/model-form";
import { requiredHint } from "/src/ui/required-hint";
import { createCreateAction } from "./create-form";
import { createScreenRead, readAll, type ScreenNotice } from "./read";

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
            const [providersResult] = await readAll(signal, {
                run: (cycle) => props.api.listProviders(cycle),
            });

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
    const backLabel = (): string => (fixedProvider() === null ? "Models" : "Provider");

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Create model"
                description="Something discovery would not produce — or a model the upstream no longer reports, to claim back."
                backHref={backHref()}
                backLabel={backLabel()}
            />

            <ReadNotice read={read} />

            <Show when={read.isFirstRead()}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead()}>
                <Show
                    when={providers().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title="No providers yet"
                            description="A model belongs to a provider, and there is none to hold it. Add a provider first — discovery may already produce this model."
                        >
                            <ButtonLink
                                href={hrefs.providerNew()}
                                tone="primary"
                                glyph="plus"
                            >
                                Create provider
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
    const [hasIdOverride, setHasIdOverride] = createSignal(false);
    const [idOverride, setIdOverride] = createSignal("");
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    const selectedProvider = createMemo(
        () => props.providers.find((row) => String(row.id) === providerId()) ?? null,
    );

    const trimmedUpstream = (): string => upstreamId().trim();

    const normalizedOverride = (): string | null =>
        normalizedPublicIdOverride(hasIdOverride(), idOverride());

    /** Inline composition errors, shown where the value was typed (gateway README §7). */
    const upstreamError = (): string | null => routingTokenError(trimmedUpstream(), "Upstream ID");

    const overrideError = (): string | null =>
        routingTokenError(normalizedOverride(), "Public ID override");

    const composedId = (): string =>
        composedPublicId(
            selectedProvider()?.model_prefix ?? "",
            trimmedUpstream(),
            normalizedOverride(),
        );

    /** The creation's composition rules: an owned provider with a routable identity. */
    const canCompose = (): boolean =>
        selectedProvider() !== null &&
        trimmedUpstream() !== "" &&
        upstreamError() === null &&
        overrideError() === null;

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
                            id_override: normalizedOverride(),
                            is_enabled: isEnabled(),
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

    const providerOptions = createMemo(() => [
        {
            value: "",
            label: "Choose a provider",
        },
        ...props.providers.map((row) => ({
            value: String(row.id),
            label: row.name,
        })),
    ]);

    return (
        <>
            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Notice
                tone="warn"
                title="A model added by hand is never removed automatically — and it claims back what discovery lost."
                detail="Naming a provider and upstream ID the upstream no longer reports claims that model back under the settings below."
            />

            {/* One form spans both cards, so pressing Enter in the identity
            fields submits the creation like in any text field of a form. */}
            <form
                novalidate={true}
                class="flex flex-col gap-5"
                onSubmit={create.submit}
            >
                <Card>
                    <CardHeader title="Identity" />
                    <div class="mt-4 flex max-w-2xl flex-col gap-4">
                        <Show
                            when={props.fixedProvider}
                            fallback={
                                <SelectField
                                    label="Provider"
                                    value={providerId()}
                                    options={providerOptions()}
                                    onChange={setProviderId}
                                    isDisabled={create.isCreating()}
                                    hint={requiredHint("A model belongs to one provider.")}
                                />
                            }
                        >
                            {(fixed) => (
                                <div>
                                    <p class="block text-[13px] font-medium text-ink">Provider</p>
                                    <p class="tnum mt-1.5">{fixed().name}</p>
                                </div>
                            )}
                        </Show>
                        <TextField
                            label="Upstream ID"
                            value={upstreamId()}
                            onInput={setUpstreamId}
                            placeholder="gpt-4o"
                            isDisabled={create.isCreating()}
                            error={upstreamError()}
                            hint={
                                props.initialUpstreamId === null
                                    ? requiredHint(
                                          "Sent upstream verbatim — letters, numbers, and symbols only.",
                                      )
                                    : requiredHint(
                                          "Carried here from a model the upstream no longer reports — submitting claims it back under the settings below.",
                                      )
                            }
                        />
                    </div>
                </Card>

                <Card>
                    <CardHeader title="Model settings" />
                    <div class="mt-4 flex max-w-2xl flex-col gap-5">
                        <Switch
                            label="Enabled"
                            isOn={isEnabled()}
                            isDisabled={create.isCreating()}
                            onToggle={setIsEnabled}
                        />

                        <ModelIdOverrideFields
                            hasIdOverride={hasIdOverride()}
                            onHasIdOverrideToggle={setHasIdOverride}
                            idOverride={idOverride()}
                            onIdOverrideInput={setIdOverride}
                            composedId={composedId()}
                            idOverrideError={overrideError()}
                            isDisabled={create.isCreating()}
                        />

                        <div>
                            <Button
                                type="submit"
                                tone="primary"
                                isDisabled={!canCreate()}
                                isBusy={create.isCreating()}
                            >
                                {create.isCreating() ? "Creating…" : "Create model"}
                            </Button>
                        </div>
                    </div>
                </Card>
            </form>
        </>
    );
}
