import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
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
    dataTableClasses,
    formControlClasses,
    LoadingBlock,
    Notice,
    PageHeader,
    PanelCard,
    pageShellClasses,
    StatusBadge,
    tableActionClasses,
    tableActionDangerClasses,
} from "./ui";

interface ModelsPageProps {
    client: PanelApiClient;
}

function thinkingSummary(model: Model): string {
    if (model.thinkingLevels === null) return "Provider defaults";
    if (model.thinkingLevels.length === 0) return "Never inject";
    return model.thinkingLevels.join(", ");
}

interface ThinkingModeOptionProps {
    isChecked: boolean;
    onSelect: () => void;
    title: string;
    description: string;
}

function ThinkingModeOption(props: ThinkingModeOptionProps): JSX.Element {
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

export function ModelsPage(props: ModelsPageProps): JSX.Element {
    const [models, setModels] = createSignal<Model[]>([]);
    const [providers, setProviders] = createSignal<Provider[]>([]);
    const [isLoading, setIsLoading] = createSignal(true);
    const [error, setError] = createSignal<string | null>(null);
    const [success, setSuccess] = createSignal<string | null>(null);
    const [busyModelId, setBusyModelId] = createSignal<number | null>(null);

    const [editingModel, setEditingModel] = createSignal<Model | null>(null);
    const [providerName, setProviderName] = createSignal("");
    const [upstreamName, setUpstreamName] = createSignal("");
    const [baseName, setBaseName] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [isThinkingOverride, setIsThinkingOverride] = createSignal(false);
    const [selectedThinkingLevels, setSelectedThinkingLevels] = createSignal<ThinkingLevel[]>([]);
    const [isSaving, setIsSaving] = createSignal(false);
    const [formError, setFormError] = createSignal<string | null>(null);

    const selectedProvider = createMemo(
        () => providers().find((provider) => provider.name === providerName()) ?? null,
    );
    const publicNamePreview = createMemo(() =>
        previewModelName({
            modelPrefix: selectedProvider()?.modelPrefix ?? "",
            upstreamName: upstreamName(),
            baseName: baseName(),
            existingName: editingModel()?.name ?? null,
        }),
    );

    async function reloadModels(): Promise<void> {
        setModels(await props.client.listModels());
    }

    function resetCreateForm(availableProviders: readonly Provider[] = providers()): void {
        setEditingModel(null);
        setProviderName(availableProviders[0]?.name ?? "");
        setUpstreamName("");
        setBaseName("");
        setIsEnabled(true);
        setIsThinkingOverride(false);
        setSelectedThinkingLevels([]);
        setFormError(null);
    }

    async function initialLoad(): Promise<void> {
        setIsLoading(true);
        setError(null);
        try {
            const [loadedProviders, loadedModels] = await Promise.all([
                props.client.listProviders(),
                props.client.listModels(),
            ]);
            setProviders(loadedProviders);
            setModels(loadedModels);
            resetCreateForm(loadedProviders);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setIsLoading(false);
        }
    }

    onSettled(() => {
        void initialLoad();
    });

    function editModel(model: Model): void {
        setEditingModel(model);
        setProviderName(model.providerName);
        setUpstreamName(model.upstreamName);
        setBaseName("");
        setIsEnabled(model.enabled);
        setIsThinkingOverride(model.thinkingLevels !== null);
        setSelectedThinkingLevels([
            ...(model.thinkingLevels ?? []),
        ]);
        setFormError(null);
        document.getElementById("model-editor")?.scrollIntoView({
            behavior: "smooth",
        });
    }

    async function saveModel(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        const currentModel = editingModel();
        setIsSaving(true);
        setFormError(null);
        setSuccess(null);
        try {
            const input = {
                ...(currentModel === null
                    ? {}
                    : {
                          id: currentModel.id,
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
            await reloadModels();
            resetCreateForm();
            setSuccess(`${savedModel.name} saved.`);
        } catch (caughtError) {
            setFormError(errorMessage(caughtError));
        } finally {
            setIsSaving(false);
        }
    }

    async function toggleModel(model: Model): Promise<void> {
        setBusyModelId(model.id);
        setError(null);
        setSuccess(null);
        try {
            await props.client.saveModel({
                id: model.id,
                providerName: model.providerName,
                upstreamName: model.upstreamName,
                enabled: !model.enabled,
                thinkingLevels: model.thinkingLevels,
            });
            await reloadModels();
            setSuccess(`${model.name} ${model.enabled ? "disabled" : "enabled"}.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyModelId(null);
        }
    }

    async function deleteModel(model: Model): Promise<void> {
        if (
            !window.confirm(
                `Delete model “${model.name}”? Its public route stops resolving immediately.`,
            )
        ) {
            return;
        }
        setBusyModelId(model.id);
        setError(null);
        setSuccess(null);
        try {
            await props.client.deleteModel(model.id);
            await reloadModels();
            if (editingModel()?.id === model.id) resetCreateForm();
            setSuccess(`${model.name} deleted.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyModelId(null);
        }
    }

    return (
        <section class={pageShellClasses}>
            <PageHeader
                eyebrow="Public routing"
                title="Models"
                description="Map public model names to one concrete provider and upstream model."
                actions={
                    <button
                        class={buttonPrimaryClasses}
                        type="button"
                        onClick={() => resetCreateForm()}
                    >
                        + New mapping
                    </button>
                }
            />

            <Show when={error() !== null}>
                <Notice kind="error">{error()}</Notice>
            </Show>
            <Show when={success() !== null}>
                <Notice kind="success">{success()}</Notice>
            </Show>

            <Show
                when={!isLoading()}
                fallback={<LoadingBlock />}
            >
                <div class="grid grid-cols-[minmax(0,1fr)_minmax(310px,380px)] items-start gap-[22px] max-[1180px]:grid-cols-1">
                    <PanelCard
                        title="Model mappings"
                        description="Exact, case-sensitive public identities."
                        count={models().length}
                    >
                        <Show
                            when={models().length > 0}
                            fallback={
                                <div class="grid gap-[5px] px-5 py-[60px] text-center text-xs text-[#89938d]">
                                    <strong class="text-sm text-[#58645d]">
                                        No model mappings yet.
                                    </strong>
                                    <span>Discover models from a provider or create one here.</span>
                                </div>
                            }
                        >
                            <div class="w-full overflow-x-auto">
                                <table class={dataTableClasses}>
                                    <thead>
                                        <tr>
                                            <th>Public name</th>
                                            <th>Provider</th>
                                            <th>Upstream name</th>
                                            <th>Thinking</th>
                                            <th>Status</th>
                                            <th>Actions</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        <For each={models()}>
                                            {(model) => (
                                                <tr>
                                                    <td>
                                                        <strong class="font-[730] text-[#202b25]">
                                                            {model.name}
                                                        </strong>
                                                        <small class="mt-1 block max-w-[270px] overflow-hidden text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                            ID {model.id}
                                                        </small>
                                                    </td>
                                                    <td>
                                                        <span>{model.providerName}</span>
                                                        <small class="mt-1 block max-w-[270px] overflow-hidden text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                            {model.providerType}
                                                        </small>
                                                    </td>
                                                    <td>
                                                        <code class="rounded-[5px] bg-[#eff5f1] px-[5px] py-0.5 font-mono text-[11px] text-[#315a46] [overflow-wrap:anywhere]">
                                                            {model.upstreamName}
                                                        </code>
                                                    </td>
                                                    <td>{thinkingSummary(model)}</td>
                                                    <td>
                                                        <StatusBadge isEnabled={model.enabled} />
                                                    </td>
                                                    <td>
                                                        <div class="flex flex-wrap justify-end gap-x-[10px] gap-y-1">
                                                            <button
                                                                class={tableActionClasses}
                                                                type="button"
                                                                disabled={
                                                                    busyModelId() === model.id
                                                                }
                                                                onClick={() => editModel(model)}
                                                            >
                                                                Edit
                                                            </button>
                                                            <button
                                                                class={tableActionClasses}
                                                                type="button"
                                                                disabled={
                                                                    busyModelId() === model.id
                                                                }
                                                                onClick={() =>
                                                                    void toggleModel(model)
                                                                }
                                                            >
                                                                {model.enabled
                                                                    ? "Disable"
                                                                    : "Enable"}
                                                            </button>
                                                            <button
                                                                class={tableActionDangerClasses}
                                                                type="button"
                                                                disabled={
                                                                    busyModelId() === model.id
                                                                }
                                                                onClick={() =>
                                                                    void deleteModel(model)
                                                                }
                                                            >
                                                                Delete
                                                            </button>
                                                        </div>
                                                    </td>
                                                </tr>
                                            )}
                                        </For>
                                    </tbody>
                                </table>
                            </div>
                        </Show>
                    </PanelCard>

                    <aside
                        id="model-editor"
                        class="sticky top-6 overflow-hidden rounded-2xl border border-[#dbe2dc] bg-white shadow-[0_18px_45px_rgb(19_37_27_/_9%)] max-[1180px]:static max-[1180px]:w-full max-[1180px]:max-w-[620px]"
                    >
                        <header class="border-b border-[#dbe2dc] bg-[linear-gradient(130deg,rgb(41_166_111_/_8%),transparent_65%),#fafcf9] px-[22px] pt-[21px] pb-[18px]">
                            <p class="m-0 text-[11px] font-extrabold tracking-[0.16em] text-[#19784e] uppercase">
                                {editingModel() === null ? "Create" : "Edit"}
                            </p>
                            <h2 class="mt-1 mb-[3px] text-[19px] font-[760] tracking-[-0.02em] text-[#18211d]">
                                {editingModel() === null
                                    ? "New model mapping"
                                    : editingModel()?.name}
                            </h2>
                            <Show when={editingModel() !== null}>
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
                                    <For each={providers()}>
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
                                        editingModel() === null
                                            ? "Defaults to upstream name"
                                            : "Blank preserves current name"
                                    }
                                />
                                <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                                    A supplied value destructively renames the route. No alias
                                    remains.
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
                                    Current provider prefix: “
                                    {selectedProvider()?.modelPrefix ?? ""}”
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
                                <legend class="mb-2 text-xs font-[720] text-[#344038]">
                                    Thinking levels
                                </legend>
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
                                                    checked={selectedThinkingLevels().includes(
                                                        level,
                                                    )}
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
                                <Show when={editingModel() !== null}>
                                    <button
                                        class={buttonGhostClasses}
                                        type="button"
                                        disabled={isSaving()}
                                        onClick={() => resetCreateForm()}
                                    >
                                        Cancel edit
                                    </button>
                                </Show>
                                <button
                                    class={buttonPrimaryClasses}
                                    type="submit"
                                    disabled={isSaving() || providers().length === 0}
                                >
                                    {isSaving() ? "Saving…" : "Save mapping"}
                                </button>
                            </footer>
                        </form>
                    </aside>
                </div>
            </Show>
        </section>
    );
}
