import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { isModelEffectivelyServing } from "/src/management-api/serving";
import { discoveryStateLabel } from "/src/management-api/vocabulary";
import { hrefs } from "/src/router";
import { AppLink } from "/src/ui/app-link";
import { Badge, type BadgeTone } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";

/**
 * How one model reads at a glance, under the gateway's three-input effective
 * serving definition derived client-side from rows the app already fetches
 * and never read off the wire: `Serving`, or the one cause that keeps it from
 * serving — the model's own switch (`Off`), the upstream no longer reporting
 * the row (`Suspended`), the provider's switch (`Held by provider`), or a
 * provider row absent from the fetched rows (`Unknown`).
 */
export function modelServingBadge(
    model: ModelResponse,
    provider: ProviderResponse | undefined,
): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    if (isModelEffectivelyServing(model, provider)) {
        return {
            tone: "ok",
            label: "Serving",
        };
    }

    if (!model.is_enabled) {
        return {
            tone: "muted",
            label: "Off",
        };
    }

    if (model.discovery_state === "vanished") {
        return {
            tone: "warn",
            label: "Suspended",
        };
    }

    if (provider !== undefined && !provider.is_enabled) {
        return {
            tone: "warn",
            label: "Held by provider",
        };
    }

    // No switch is off and the row is listed, yet the provider row the
    // conjunction needs is missing from the fetched rows — reachable only
    // while a concurrent provider deletion races the listing read; the next
    // read settles the verdict.
    return {
        tone: "muted",
        label: "Unknown",
    };
}

/**
 * Where one row stands in its discovery lifecycle: discovered from upstream,
 * added by hand, or no longer listed. A row the upstream no longer reports
 * keeps its curated settings and can be claimed back; it never serves
 * meanwhile.
 */
export function modelDiscoveryBadge(model: ModelResponse): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    switch (model.discovery_state) {
        case "discovered":
            return {
                tone: "muted",
                label: discoveryStateLabel("discovered"),
            };

        case "manual":
            return {
                tone: "info",
                label: discoveryStateLabel("manual"),
            };

        case "vanished":
            return {
                tone: "warn",
                label: discoveryStateLabel("vanished"),
            };
    }
}

/**
 * One model's row: its public ID, where it comes from, and its two facts —
 * serving state and discovery state. The row is one link to the model's own
 * page, where the settings behind those facts are changed.
 */
function ModelRow(props: {
    readonly model: ModelResponse;
    readonly provider: ProviderResponse | undefined;
    readonly showProvider?: boolean | undefined;
}): JSX.Element {
    const serving = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelServingBadge(props.model, props.provider);

    const discovery = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelDiscoveryBadge(props.model);

    return (
        <li>
            <AppLink
                href={hrefs.modelDetail(props.model.id)}
                class="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3 transition-colors duration-150 ease-out hover:border-ink-faint"
            >
                <span class="min-w-0 flex-1 basis-56">
                    <span class="tnum block truncate text-[13.5px] font-medium">
                        {props.model.public_id}
                    </span>
                    <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                        {props.showProvider === true && props.provider !== undefined
                            ? `${props.provider.name} · ${props.model.upstream_id}`
                            : props.model.upstream_id}
                        {props.model.id_override !== null ? " · Custom ID" : ""}
                    </span>
                </span>
                <span class="flex items-center gap-2">
                    <Badge
                        tone={serving().tone}
                        label={serving().label}
                    />
                    <Badge
                        tone={discovery().tone}
                        label={discovery().label}
                    />
                </span>
                <Glyph
                    name="chev-right"
                    class="text-ink-faint transition-transform duration-150 ease-out group-hover:translate-x-0.5 group-hover:text-ink-dim"
                />
            </AppLink>
        </li>
    );
}

/** A list of model rows, newest reading first as the gateway orders them. */
export function ModelList(props: {
    readonly models: ReadonlyArray<ModelResponse>;
    readonly providersById: ReadonlyMap<number, ProviderResponse>;
    readonly showProvider?: boolean | undefined;
}): JSX.Element {
    return (
        <ul class="flex flex-col gap-2">
            <For each={props.models}>
                {(model) => (
                    <ModelRow
                        model={model}
                        provider={props.providersById.get(model.provider_id)}
                        showProvider={props.showProvider}
                    />
                )}
            </For>
        </ul>
    );
}
