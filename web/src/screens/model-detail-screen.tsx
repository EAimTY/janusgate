import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { discoveryStateLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import {
    createPinnedRow,
    createScreenRead,
    isSameModelRow,
    isSameProviderRow,
    readAll,
    readModels,
    readProviders,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { EntityUsageCard } from "/src/screens/usage-stats";
import { AppLink } from "/src/ui/app-link";
import { ButtonLink } from "/src/ui/controls";
import { Badge, type BadgeTone, Notice } from "/src/ui/feedback";
import { Card, CardHeader, Fact, FactList, PageHeader } from "/src/ui/layout";
import { modelDiscoveryBadge, modelServingBadge } from "/src/ui/models-table";
import { RowNotFound } from "./row-not-found";

/**
 * One model's page: what it is, where it stands, and what serving clients
 * did with it. Nothing is changed here — the edit page owns every change.
 */
export function ModelDetailScreen(props: {
    readonly api: ManagementApi;
    readonly modelId: number;
}): JSX.Element {
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);

    const read = createScreenRead({
        load: async (signal) => {
            const [providersResult, modelsResult] = await readAll(
                signal,
                readProviders(props.api),
                readModels(props.api, {}),
            );

            setProviders(providersResult.providers);
            setModels(modelsResult.models);
        },
    });

    // Each row keeps its previous answer when a re-read returns the same
    // content (`createPinnedRow`), so a refresh never flips object identity
    // and never remounts the cards (or restarts the usage read) over
    // unchanged data. The owning provider is a lookup beside the model, not
    // the screen's subject, so it pins without holding.
    const model = createPinnedRow(
        () => models().find((row) => row.id === props.modelId) ?? null,
        isSameModelRow,
    );

    const provider = createPinnedRow(() => {
        const current = model();
        return current === null
            ? null
            : (providers().find((row) => row.id === current.provider_id) ?? null);
    }, isSameProviderRow);

    return (
        <ScreenFrame
            read={read}
            row={() => {
                const current = model();
                return current === null
                    ? null
                    : {
                          model: current,
                          provider: provider(),
                      };
            }}
            firstReadHeader={
                <PageHeader
                    title="Model"
                    backHref={hrefs.models(null)}
                    backLabel="Models"
                />
            }
            notFound={
                <RowNotFound
                    kind="model"
                    headerTitle="Model"
                />
            }
        >
            {(both) => (
                <ModelFacts
                    api={props.api}
                    provider={both.provider}
                    model={both.model}
                />
            )}
        </ScreenFrame>
    );
}

function ModelFacts(props: {
    readonly api: ManagementApi;
    readonly provider: ProviderResponse | null;
    readonly model: ModelResponse;
}): JSX.Element {
    const serving = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelServingBadge(props.model, props.provider ?? undefined);

    const discovery = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelDiscoveryBadge(props.model);

    return (
        <>
            <PageHeader
                title={props.model.public_id}
                description={`${props.model.upstream_id} on ${props.model.provider}.`}
                backHref={hrefs.models(props.model.provider_id)}
                backLabel="Models"
            >
                <Badge
                    tone={serving().tone}
                    label={serving().label}
                />
                <Badge
                    tone={discovery().tone}
                    label={discovery().label}
                />
                <ButtonLink href={hrefs.modelEdit(props.model.id)}>Edit model</ButtonLink>
            </PageHeader>

            <Show when={props.model.discovery_state === "vanished"}>
                {/* The same warn notice every screen states, with the two
                recovery paths linked where they are named. */}
                <Notice
                    tone="warn"
                    title="The upstream no longer reports this model."
                    detail={
                        <>
                            It serves nothing meanwhile.{" "}
                            <AppLink
                                href={hrefs.providerDetail(props.model.provider_id)}
                                class="font-medium text-accent hover:text-accent-strong"
                            >
                                Discover again on the provider's page
                            </AppLink>
                            {" when the upstream lists it, or "}
                            <AppLink
                                href={hrefs.modelNew(
                                    props.model.provider_id,
                                    props.model.upstream_id,
                                )}
                                class="font-medium text-accent hover:text-accent-strong"
                            >
                                claim it back from the Create model page
                            </AppLink>
                            {"."}
                        </>
                    }
                />
            </Show>

            <Card>
                <CardHeader title="Details" />
                <FactList>
                    <Fact
                        label="Public ID"
                        value={props.model.public_id}
                        isTabular
                        class="break-all"
                    />
                    <Fact
                        label="Upstream ID"
                        value={props.model.upstream_id}
                        isTabular
                        class="break-all"
                    />
                    <Fact
                        label="Provider"
                        value={
                            props.provider === null ? (
                                <span class="text-ink-dim">{props.model.provider}</span>
                            ) : (
                                <AppLink
                                    href={hrefs.providerDetail(props.provider.id)}
                                    class="font-medium text-accent hover:text-accent-strong"
                                >
                                    {props.provider.name}
                                </AppLink>
                            )
                        }
                    />
                    <Fact
                        label="Discovery"
                        value={discoveryStateLabel(props.model.discovery_state)}
                        class="text-ink-dim"
                    />
                    {/* The one enabled pair this page speaks: the Details
                        card's Status echoes the enabled vocabulary every
                        detail page shares — the tight list-row "On/Off" never
                        appears here, while the header badge stays the
                        effective-serving ladder above. */}
                    <Fact
                        label="Status"
                        value={props.model.is_enabled ? "Enabled" : "Disabled"}
                    />
                </FactList>
            </Card>

            <EntityUsageCard
                api={props.api}
                filter={{
                    model_id: props.model.id,
                }}
                subjectKind="model"
            />
        </>
    );
}
