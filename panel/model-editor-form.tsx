import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, Show } from "solid-js";
import { errorMessage, type Model, type PanelApiClient, type Provider } from "./api-client";
import { providerTypeLabel } from "./labels";
import {
    previewModelName,
    type ThinkingLevel,
    thinkingLevels,
    toggleThinkingLevel,
} from "./model-helpers";
import {
    buttonGhostClasses,
    buttonPrimaryClasses,
    checkboxClasses,
    formControlClasses,
    Notice,
} from "./ui";

interface ModelEditorFormProps {
    client: PanelApiClient;
    providers: readonly Provider[];
    /** `null` opens the create form; a model prefills its fields for editing. */
    editingModel: Model | null;
    /** Reloads the list, records the outcome, and resets to the create form. */
    onSaved: (savedModel: { id: number; name: string }) => Promise<void>;
    onCancelEdit: () => void;
}

function ThinkingModeOption(props: {
    isChecked: boolean;
    onSelect: () => void;
    title: string;
    description: string;
}): JSX.Element {
    return (
        <label class="grid grid-cols-[15px_1fr] items-start gap-[7px] rounded-lg border border-[#e0e6e2] bg-[#fafbfa] p-2.5 text-xs font-[720] text-[#344038]">
            <input
                class={`${checkboxClasses} mt-0.5`}
                type="radio"
                name="thinking-mode"
                checked={props.isChecked}
                onChange={() => props.onSelect()}
            />
            <span class="grid gap-0.5">
                <strong class="text-[11px]">{props.title}</strong>
                <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                    {props.description}
                </small>
            </span>
        </label>
    );
}

/** The create/edit mapping form; a fresh instance is mounted per editing session. */
export function ModelEditorForm(props: ModelEditorFormProps): JSX.Element {
    const editingModel = props.editingModel;
    const [providerName, setProviderName] = createSignal(
        editingModel?.providerName ?? props.providers[0]?.name ?? "",
    );
    const [upstreamName, setUpstreamName] = createSignal(editingModel?.upstreamName ?? "");
    const [baseName, setBaseName] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(editingModel?.enabled ?? true);
    const [isThinkingOverride, setIsThinkingOverride] = createSignal(
        editingModel?.thinkingLevels !== null && editingModel?.thinkingLevels !== undefined,
    );
    const [selectedThinkingLevels, setSelectedThinkingLevels] = createSignal<ThinkingLevel[]>([
        ...(editingModel?.thinkingLevels ?? []),
    ]);
    const [isSaving, setIsSaving] = createSignal(false);
    const [formError, setFormError] = createSignal<string | null>(null);

    const selectedProvider = createMemo(
        () => props.providers.find((provider) => provider.name === providerName()) ?? null,
    );
    const publicNamePreview = createMemo(() =>
        previewModelName({
            modelPrefix: selectedProvider()?.modelPrefix ?? "",
            upstreamName: upstreamName(),
            baseName: baseName(),
            existingName: editingModel?.name ?? null,
        }),
    );

    async function saveModel(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        setIsSaving(true);
        setFormError(null);
        try {
            const input = {
                ...(editingModel === null
                    ? {}
                    : {
                          id: editingModel.id,
                      }),
                providerName: providerName(),
                upstreamName: upstreamName(),
                ...(baseName().length === 0
                    ? {}
                    : {
                          name: baseName(),
                      }),
                enabled: isEnabled(),
                thinkingLevels: isThinkingOverride() ? selectedThinkingLevels() : null,
            };
            const savedModel = await props.client.saveModel(input);
            await props.onSaved(savedModel);
        } catch (caughtError) {
            setFormError(errorMessage(caughtError));
        } finally {
            setIsSaving(false);
        }
    }

    return (
        <aside
            id="model-editor"
            class="sticky top-6 overflow-hidden rounded-2xl border border-[#dbe2dc] bg-white shadow-[0_18px_45px_rgb(19_37_27_/_9%)] max-[1180px]:static max-[1180px]:w-full max-[1180px]:max-w-[620px]"
        >
            <header class="border-b border-[#dbe2dc] bg-[linear-gradient(130deg,rgb(41_166_111_/_8%),transparent_65%),#fafcf9] px-[22px] pt-[21px] pb-[18px]">
                <p class="m-0 text-[11px] font-extrabold tracking-[0.16em] text-[#19784e] uppercase">
                    {editingModel === null ? "Create" : "Edit"}
                </p>
                <h2 class="mt-1 mb-[3px] text-[19px] font-[760] tracking-[-0.02em] text-[#18211d]">
                    {editingModel === null ? "New model mapping" : editingModel.name}
                </h2>
                <Show when={editingModel !== null}>
                    <p class="m-0 text-xs text-[#66726b]">
                        Leave base name blank to preserve the current public name.
                    </p>
                </Show>
            </header>
            <form
                class="grid gap-[17px] px-[22px] pt-5 pb-[22px] [&>[role=alert]]:mb-0"
                onSubmit={(event) => void saveModel(event)}
            >
                <Show when={formError() !== null}>
                    <Notice kind="error">{formError()}</Notice>
                </Show>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Provider
                    <select
                        class={formControlClasses}
                        required
                        value={providerName()}
                        onChange={(event) => setProviderName(event.currentTarget.value)}
                    >
                        <option
                            value=""
                            disabled
                        >
                            Select a provider
                        </option>
                        <For each={props.providers}>
                            {(provider) => (
                                <option value={provider.name}>
                                    {provider.name} · {providerTypeLabel(provider.type)}
                                </option>
                            )}
                        </For>
                    </select>
                    <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                        Every provider type is eligible, including OpenAI OAuth.
                    </small>
                </label>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Upstream model name
                    <input
                        class={formControlClasses}
                        required
                        value={upstreamName()}
                        onInput={(event) => setUpstreamName(event.currentTarget.value)}
                        placeholder="gpt-5.4"
                    />
                    <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                        Passed to the selected provider exactly as entered.
                    </small>
                </label>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Public base name (optional)
                    <input
                        class={formControlClasses}
                        value={baseName()}
                        onInput={(event) => setBaseName(event.currentTarget.value)}
                        placeholder={
                            editingModel === null
                                ? "Defaults to upstream name"
                                : "Blank preserves current name"
                        }
                    />
                    <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                        A supplied value destructively renames the route. No alias remains.
                    </small>
                </label>
                <div class="grid gap-[5px] rounded-[9px] border border-[#dbe8e0] bg-[#f1f6f3] p-[13px]">
                    <span class="text-[9px] font-extrabold tracking-[0.1em] text-[#6b776f] uppercase">
                        Final public name
                    </span>
                    <code class="bg-transparent p-0 font-mono text-[13px] font-[720] text-[#184b34]">
                        {publicNamePreview().length > 0
                            ? publicNamePreview()
                            : "Waiting for an upstream name…"}
                    </code>
                    <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                        Current provider prefix: “{selectedProvider()?.modelPrefix ?? ""}”
                    </small>
                </div>
                <label class="flex items-center gap-[7px] text-xs font-[720] text-[#344038]">
                    <input
                        class={checkboxClasses}
                        type="checkbox"
                        checked={isEnabled()}
                        onChange={(event) => setIsEnabled(event.currentTarget.checked)}
                    />
                    Enabled
                </label>
                <fieldset class="m-0 grid min-w-0 gap-[9px] border-0 p-0">
                    <legend class="mb-2 text-xs font-[720] text-[#344038]">Thinking levels</legend>
                    <ThinkingModeOption
                        isChecked={!isThinkingOverride()}
                        onSelect={() => setIsThinkingOverride(false)}
                        title="Provider defaults"
                        description="Defaults, not off. No reasoning setting is injected."
                    />
                    <ThinkingModeOption
                        isChecked={isThinkingOverride()}
                        onSelect={() => setIsThinkingOverride(true)}
                        title="Custom supported levels"
                        description="An empty selection means never inject reasoning configuration."
                    />
                    <div
                        class={[
                            "flex flex-wrap gap-[7px] pt-[7px] pl-[25px]",
                            {
                                "opacity-[0.48]": !isThinkingOverride(),
                            },
                        ]}
                    >
                        <For each={thinkingLevels}>
                            {(level) => (
                                <label class="flex items-center gap-[5px] rounded-md bg-[#edf3ef] px-[7px] py-[5px] font-mono text-[10px] font-[650] text-[#344038]">
                                    <input
                                        class={checkboxClasses}
                                        type="checkbox"
                                        disabled={!isThinkingOverride()}
                                        checked={selectedThinkingLevels().includes(level)}
                                        onChange={(event) =>
                                            setSelectedThinkingLevels(
                                                toggleThinkingLevel(
                                                    selectedThinkingLevels(),
                                                    level,
                                                    event.currentTarget.checked,
                                                ),
                                            )
                                        }
                                    />
                                    {level}
                                </label>
                            )}
                        </For>
                    </div>
                </fieldset>
                <footer class="flex items-center justify-end gap-[9px] pt-[3px]">
                    <Show when={editingModel !== null}>
                        <button
                            class={buttonGhostClasses}
                            type="button"
                            disabled={isSaving()}
                            onClick={props.onCancelEdit}
                        >
                            Cancel edit
                        </button>
                    </Show>
                    <button
                        class={buttonPrimaryClasses}
                        type="submit"
                        disabled={isSaving() || props.providers.length === 0}
                    >
                        {isSaving() ? "Saving…" : "Save mapping"}
                    </button>
                </footer>
            </form>
        </aside>
    );
}
