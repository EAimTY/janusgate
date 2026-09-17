import type { JSX } from "@solidjs/web";
import { createSignal, createUniqueId, For, Show } from "solid-js";
import { toUpdateModelRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import {
    ErrorCode,
    type ModelResponse,
    THINKING_LEVEL_ORDER,
    type ThinkingLevel,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { Badge } from "/src/ui/badge";
import { Banner } from "/src/ui/banner";
import { Button } from "/src/ui/button";
import { CheckboxField, TextField } from "/src/ui/field";

export type ModelListProps = {
    readonly api: ManagementApi;
    readonly models: ReadonlyArray<ModelResponse>;
    /** A single provider's listing does not repeat the provider column. */
    readonly shouldShowProvider?: boolean;
    /** Installs the row a `models:update` returned, without refetching. */
    readonly onModelUpdated: (model: ModelResponse) => void;
    /**
     * Called with the gateway's message when it reports the row gone (errno 3):
     * the screen renders the message and reloads the listing.
     */
    readonly onModelVanished: (message: string) => void;
};

function modelKey(model: ModelResponse): string {
    return `${model.provider_id}:${model.upstream_id}`;
}

function thinkingLevelsText(model: ModelResponse): string {
    return model.thinking_levels_override === null
        ? "unfiltered"
        : model.thinking_levels_override.join(", ");
}

export function ModelList(props: ModelListProps): JSX.Element {
    const [editedKey, setEditedKey] = createSignal<string | null>(null);

    return (
        <div class="overflow-x-auto rounded-lg border border-slate-200 bg-white shadow-sm">
            <table class="w-full text-left text-sm">
                <thead class="border-b border-slate-200 bg-slate-50 text-xs tracking-wide text-slate-500 uppercase">
                    <tr>
                        <th class="px-4 py-3 font-medium">Public ID</th>
                        <Show when={props.shouldShowProvider ?? false}>
                            <th class="px-4 py-3 font-medium">Provider</th>
                        </Show>
                        <th class="hidden px-4 py-3 font-medium sm:table-cell">Upstream ID</th>
                        <th class="px-4 py-3 font-medium">Configured</th>
                        <th class="px-4 py-3 font-medium">Serving</th>
                        <th class="hidden px-4 py-3 font-medium sm:table-cell">Thinking levels</th>
                        <th class="px-4 py-3" />
                    </tr>
                </thead>
                <tbody>
                    <For each={props.models}>
                        {(model) => (
                            <>
                                <tr class="border-b border-slate-100">
                                    <td class="px-4 py-3">
                                        <span class="font-mono text-slate-900">{model.id}</span>
                                        <Show when={model.id_override ?? null}>
                                            {(idOverride) => (
                                                <p class="font-mono text-xs text-slate-400">
                                                    override: {idOverride()}
                                                </p>
                                            )}
                                        </Show>
                                    </td>
                                    <Show when={props.shouldShowProvider ?? false}>
                                        <td class="px-4 py-3 text-slate-600">
                                            {model.provider}{" "}
                                            <span class="font-mono text-xs text-slate-400">
                                                #{model.provider_id}
                                            </span>
                                        </td>
                                    </Show>
                                    <td class="hidden px-4 py-3 font-mono text-xs text-slate-600 sm:table-cell">
                                        {model.upstream_id}
                                    </td>
                                    <td class="px-4 py-3">
                                        <Badge tone={model.is_enabled ? "positive" : "neutral"}>
                                            {model.is_enabled ? "Enabled" : "Disabled"}
                                        </Badge>
                                    </td>
                                    <td class="px-4 py-3">
                                        <Badge
                                            tone={
                                                model.is_effectively_enabled
                                                    ? "positive"
                                                    : "neutral"
                                            }
                                        >
                                            {model.is_effectively_enabled
                                                ? "Serving"
                                                : "Not serving"}
                                        </Badge>
                                    </td>
                                    <td class="hidden px-4 py-3 text-slate-600 sm:table-cell">
                                        {thinkingLevelsText(model)}
                                    </td>
                                    <td class="px-4 py-3">
                                        <div class="flex justify-end">
                                            <Button
                                                variant="secondary"
                                                onClick={() =>
                                                    setEditedKey((current) =>
                                                        current === modelKey(model)
                                                            ? null
                                                            : modelKey(model),
                                                    )
                                                }
                                            >
                                                {editedKey() === modelKey(model) ? "Close" : "Edit"}
                                            </Button>
                                        </div>
                                    </td>
                                </tr>
                                <Show when={editedKey() === modelKey(model)}>
                                    <tr class="border-b border-slate-100">
                                        <td
                                            colspan={(props.shouldShowProvider ?? false) ? 7 : 6}
                                            class="bg-slate-50 px-4 py-4"
                                        >
                                            <ModelEditor
                                                api={props.api}
                                                model={model}
                                                onSaved={(updated) => {
                                                    props.onModelUpdated(updated);
                                                    setEditedKey(null);
                                                }}
                                                onVanished={(message) => {
                                                    setEditedKey(null);
                                                    props.onModelVanished(message);
                                                }}
                                                onCancel={() => setEditedKey(null)}
                                            />
                                        </td>
                                    </tr>
                                </Show>
                            </>
                        )}
                    </For>
                </tbody>
            </table>
        </div>
    );
}

type ModelEditorProps = {
    readonly api: ManagementApi;
    readonly model: ModelResponse;
    readonly onSaved: (model: ModelResponse) => void;
    readonly onVanished: (message: string) => void;
    readonly onCancel: () => void;
};

/**
 * One model's override state, edited as a read-modify-write (README §4.5):
 * the form starts from the listed row, and saving sends the row's complete
 * desired override state back.
 */
function ModelEditor(props: ModelEditorProps): JSX.Element {
    const idPrefix = createUniqueId();
    const [idOverride, setIdOverride] = createSignal(props.model.id_override ?? "");
    const [isEnabled, setIsEnabled] = createSignal(props.model.is_enabled);

    const [thinkingLevels, setThinkingLevels] = createSignal<ThinkingLevel[]>(
        props.model.thinking_levels_override ?? [],
    );

    const [failure, setFailure] = createSignal<string | null>(null);
    const [isSaving, setIsSaving] = createSignal(false);

    const toggleThinkingLevel = (level: ThinkingLevel, isChecked: boolean): void => {
        setThinkingLevels((current) =>
            isChecked
                ? THINKING_LEVEL_ORDER.filter(
                      (candidate) => candidate === level || current.includes(candidate),
                  )
                : current.filter((candidate) => candidate !== level),
        );
    };

    const save = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();

        if (isSaving()) {
            return;
        }

        const trimmedOverride = idOverride().trim();
        setFailure(null);
        setIsSaving(true);

        const outcome = await performManagementCall(() =>
            props.api.updateModel(
                toUpdateModelRequest(props.model, {
                    id_override: trimmedOverride === "" ? null : trimmedOverride,
                    is_enabled: isEnabled(),
                    thinking_levels_override:
                        thinkingLevels().length === 0 ? null : thinkingLevels(),
                }),
            ),
        );

        setIsSaving(false);

        if (outcome.status === "ok") {
            props.onSaved(outcome.result.model);
            return;
        }

        if (outcome.status === "failed") {
            if (outcome.errno === ErrorCode.NotFound) {
                props.onVanished(outcome.message);
                return;
            }

            setFailure(outcome.message);
        }
    };

    return (
        <form
            novalidate={true}
            class="flex max-w-2xl flex-col gap-4"
            onSubmit={save}
        >
            <div>
                <h3 class="text-sm font-semibold text-slate-900">Edit model overrides</h3>
                <p class="mt-0.5 font-mono text-xs text-slate-500">
                    {props.model.provider} · {props.model.upstream_id}
                </p>
            </div>

            <Show when={failure()}>
                {(message) => <Banner variant="error">{message()}</Banner>}
            </Show>

            <TextField
                id={`${idPrefix}-id-override`}
                label="Public ID override"
                value={idOverride()}
                onInput={setIdOverride}
                disabled={isSaving()}
                isMonospace
                hint="Empty composes the ID from the provider's prefix and the upstream ID. Changing it stops the previous ID from resolving."
            />

            <CheckboxField
                id={`${idPrefix}-enabled`}
                label="Serve traffic"
                checked={isEnabled()}
                onChange={setIsEnabled}
                disabled={isSaving()}
                hint="The model serves only when the provider is enabled too."
            />

            <fieldset class="flex flex-col gap-2">
                <legend class="text-sm font-medium text-slate-700">Thinking levels</legend>
                <For each={THINKING_LEVEL_ORDER}>
                    {(level) => (
                        <CheckboxField
                            id={`${idPrefix}-thinking-${level}`}
                            label={level}
                            checked={thinkingLevels().includes(level)}
                            onChange={(isChecked) => toggleThinkingLevel(level, isChecked)}
                            disabled={isSaving()}
                        />
                    )}
                </For>
                <p class="text-xs text-slate-500">
                    Only the selected levels are accepted from clients. Selecting none leaves the
                    requested level unfiltered.
                </p>
            </fieldset>

            <div class="flex items-center gap-2">
                <Button
                    type="submit"
                    disabled={isSaving()}
                >
                    {isSaving() ? "Saving…" : "Save model"}
                </Button>
                <Button
                    variant="secondary"
                    disabled={isSaving()}
                    onClick={props.onCancel}
                >
                    Cancel
                </Button>
            </div>
        </form>
    );
}
