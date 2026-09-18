import type { JSX } from "@solidjs/web";
import { For } from "solid-js";
import { describeLevels } from "/src/format";
import type { ModelResponse, ProviderResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { AppLink } from "/src/ui/app-link";
import { Badge, type BadgeTone } from "/src/ui/feedback";
import { Glyph } from "/src/ui/glyph";

/** How one model reads at a glance: serving, off, or held by its provider. */
export function modelServingBadge(
    model: ModelResponse,
    provider: ProviderResponse | undefined,
): {
    readonly tone: BadgeTone;
    readonly label: string;
} {
    if (model.is_effectively_enabled) {
        return {
            tone: "ok",
            label: "Serving",
        };
    }

    if (model.is_enabled && provider !== undefined && !provider.is_enabled) {
        return {
            tone: "warn",
            label: "Held by provider",
        };
    }

    return {
        tone: "muted",
        label: "Off",
    };
}

/**
 * One model's row: its public ID, where it comes from, and its two facts —
 * serving state and thinking levels. The row is one link to the model's own
 * page, where the overrides behind those facts are edited.
 */
export function ModelRow(props: {
    readonly model: ModelResponse;
    readonly provider: ProviderResponse | undefined;
    readonly showProvider?: boolean | undefined;
}): JSX.Element {
    const serving = (): {
        readonly tone: BadgeTone;
        readonly label: string;
    } => modelServingBadge(props.model, props.provider);

    return (
        <li>
            <AppLink
                href={hrefs.modelDetail(props.model.provider_id, props.model.upstream_id)}
                class="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-xl border border-rule bg-surface px-4 py-3 transition-colors duration-150 ease-out hover:border-ink-faint"
            >
                <span class="min-w-0 flex-1 basis-56">
                    <span class="tnum block truncate text-[13.5px] font-medium">
                        {props.model.id}
                    </span>
                    <span class="tnum mt-0.5 block truncate text-[12.5px] text-ink-faint">
                        {props.showProvider === true && props.provider !== undefined
                            ? `${props.provider.name} · ${props.model.upstream_id}`
                            : props.model.upstream_id}
                        {props.model.id_override !== null ? " · ID override" : ""}
                    </span>
                </span>
                <span class="flex items-center gap-2">
                    <Badge
                        tone={serving().tone}
                        label={serving().label}
                    />
                    <span class="hidden rounded-full bg-muted px-2.5 py-1 text-[12.5px] font-medium whitespace-nowrap text-ink-dim sm:inline">
                        {describeLevels(props.model.thinking_levels_override)}
                    </span>
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
