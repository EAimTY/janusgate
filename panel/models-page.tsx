import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, For, onSettled, Show } from "solid-js";
import { errorMessage, type Model, type PanelApiClient, type Provider } from "./api-client";
import {
    previewModelName,
    type ThinkingLevel,
    thinkingLevels,
    toggleThinkingLevel,
} from "./model-helpers";
import { LoadingBlock, Notice } from "./ui";

interface ModelsPageProps {
    client: PanelApiClient;
}

function providerTypeLabel(provider: Provider): string {
    if (provider.type === "openai_oauth") return "OpenAI OAuth";
    if (provider.type === "antigravity_oauth") return "Antigravity OAuth";
    return "OpenAI-compatible";
}

function thinkingSummary(model: Model): string {
    if (model.thinkingLevels === null) return "Provider defaults";
    if (model.thinkingLevels.length === 0) return "Never inject";
    return model.thinkingLevels.join(", ");
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
        <section class="mx-auto min-h-screen w-full max-w-[1500px] px-[clamp(24px,4vw,64px)] pt-[52px] pb-20 [&_button]:focus-visible:outline-3 [&_button]:focus-visible:outline-offset-2 [&_button]:focus-visible:outline-[#29a66f]/25 max-[820px]:min-h-[calc(100vh-66px)] max-[820px]:px-[18px] max-[820px]:pt-8 max-[820px]:pb-[60px]">
            <header class="mb-[30px] flex items-end justify-between gap-7 max-[820px]:flex-col max-[820px]:items-start">
                <div>
                    <p class="m-0 text-[11px] font-extrabold text-[#19784e] uppercase tracking-[0.16em]">
                        Public routing
                    </p>
                    <h1 class="mt-[3px] mb-2 text-[clamp(32px,4vw,46px)] leading-none font-[760] text-[#18211d] tracking-[-0.045em] max-[540px]:text-[34px]">
                        Models
                    </h1>
                    <p class="m-0 max-w-[670px] text-sm text-[#66726b]">
                        Map public model names to one concrete provider and upstream model.
                    </p>
                </div>
                <button
                    class="inline-flex min-h-10 items-center justify-center rounded-[10px] border border-[#19784e] bg-[#19784e] px-[15px] py-[9px] text-[13px] leading-[1.2] font-[720] text-white shadow-[0_8px_20px_rgb(25_120_78_/_17%)] transition-[background,border-color,transform] duration-[140ms] ease-in-out enabled:hover:bg-[#126a42] enabled:active:translate-y-px"
                    type="button"
                    onClick={() => resetCreateForm()}
                >
                    + New mapping
                </button>
            </header>

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
                    <section class="mb-[22px] min-w-0 overflow-hidden rounded-2xl border border-[#dbe2dc] bg-white shadow-[0_9px_28px_rgb(19_37_27_/_4%)]">
                        <header class="flex items-center justify-between gap-5 border-b border-[#e6ebe7] bg-[linear-gradient(90deg,#fafcf9,#fff)] px-[22px] py-[19px]">
                            <div>
                                <h2 class="mb-[3px] text-base font-[760] text-[#18211d] tracking-[-0.02em]">
                                    Model mappings
                                </h2>
                                <p class="m-0 text-xs text-[#66726b]">
                                    Exact, case-sensitive public identities.
                                </p>
                            </div>
                            <span class="grid h-[29px] min-w-[29px] place-items-center rounded-lg bg-[#eaf0ec] px-2 text-xs font-[750] text-[#456052]">
                                {models().length}
                            </span>
                        </header>
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
                                <table class="w-full border-collapse text-[13px]">
                                    <thead>
                                        <tr>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-left text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Public name
                                            </th>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-left text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Provider
                                            </th>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-left text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Upstream name
                                            </th>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-left text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Thinking
                                            </th>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-left text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Status
                                            </th>
                                            <th class="border-b border-[#e9edea] bg-[#fbfcfb] px-4 py-[11px] text-right text-[10px] font-extrabold whitespace-nowrap text-[#78847c] uppercase tracking-[0.1em]">
                                                Actions
                                            </th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        <For each={models()}>
                                            {(model) => (
                                                <tr class="group last:[&>td]:border-b-0">
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <strong class="font-[730] text-[#202b25]">
                                                            {model.name}
                                                        </strong>
                                                        <small class="mt-1 block max-w-[270px] overflow-hidden text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                            ID {model.id}
                                                        </small>
                                                    </td>
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <span>{model.providerName}</span>
                                                        <small class="mt-1 block max-w-[270px] overflow-hidden text-[10px] text-ellipsis whitespace-nowrap text-[#89938d]">
                                                            {model.providerType}
                                                        </small>
                                                    </td>
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <code class="rounded-[5px] bg-[#eff5f1] px-[5px] py-0.5 font-mono text-[11px] text-[#315a46] [overflow-wrap:anywhere]">
                                                            {model.upstreamName}
                                                        </code>
                                                    </td>
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <span>{thinkingSummary(model)}</span>
                                                    </td>
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <span
                                                            class={[
                                                                "inline-flex items-center gap-[5px] rounded-full bg-[#edf0ee] px-2 py-1 text-[10px] font-[750] text-[#68736c] before:h-[5px] before:w-[5px] before:rounded-full before:bg-[#909a94] before:content-['']",
                                                                {
                                                                    "bg-[#dff5e9]": model.enabled,
                                                                    "text-[#176943]": model.enabled,
                                                                    "before:bg-[#29a66f]":
                                                                        model.enabled,
                                                                },
                                                            ]}
                                                        >
                                                            {model.enabled ? "Enabled" : "Disabled"}
                                                        </span>
                                                    </td>
                                                    <td class="max-w-[310px] border-b border-[#edf0ee] px-4 py-[15px] text-right align-middle text-[#3c4841] group-hover:bg-[#fcfdfc]">
                                                        <div class="flex flex-wrap justify-end gap-x-[10px] gap-y-1">
                                                            <button
                                                                class="border-0 bg-transparent px-0 py-0.5 text-[11px] font-bold text-[#277351] enabled:hover:text-[#124e32] enabled:hover:underline enabled:hover:underline-offset-[3px] disabled:cursor-not-allowed disabled:opacity-[0.58]"
                                                                type="button"
                                                                disabled={
                                                                    busyModelId() === model.id
                                                                }
                                                                onClick={() => editModel(model)}
                                                            >
                                                                Edit
                                                            </button>
                                                            <button
                                                                class="border-0 bg-transparent px-0 py-0.5 text-[11px] font-bold text-[#277351] enabled:hover:text-[#124e32] enabled:hover:underline enabled:hover:underline-offset-[3px] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                                                class="border-0 bg-transparent px-0 py-0.5 text-[11px] font-bold text-[#b53b3b] enabled:hover:text-[#124e32] enabled:hover:underline enabled:hover:underline-offset-[3px] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                    </section>

                    <aside
                        id="model-editor"
                        class="sticky top-6 overflow-hidden rounded-2xl border border-[#dbe2dc] bg-white shadow-[0_18px_45px_rgb(19_37_27_/_9%)] max-[1180px]:static max-[1180px]:w-full max-[1180px]:max-w-[620px]"
                    >
                        <header class="border-b border-[#dbe2dc] bg-[linear-gradient(130deg,rgb(41_166_111_/_8%),transparent_65%),#fafcf9] px-[22px] pt-[21px] pb-[18px]">
                            <p class="m-0 text-[11px] font-extrabold text-[#19784e] uppercase tracking-[0.16em]">
                                {editingModel() === null ? "Create" : "Edit"}
                            </p>
                            <h2 class="mt-1 mb-[3px] text-[19px] font-[760] text-[#18211d] tracking-[-0.02em]">
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
                                    class="w-full rounded-lg border border-[#ccd6cf] bg-[#fbfcfb] px-[11px] py-2.5 text-[13px] leading-[1.35] text-[#233029] transition-[border-color,box-shadow] duration-[120ms] enabled:hover:border-[#aebdb4] focus:border-[#29a66f] focus:bg-white focus:outline-none focus:shadow-[0_0_0_3px_rgb(41_166_111_/_10%)] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                                {provider.name} · {providerTypeLabel(provider)}
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
                                    class="w-full rounded-lg border border-[#ccd6cf] bg-[#fbfcfb] px-[11px] py-2.5 text-[13px] leading-[1.35] text-[#233029] transition-[border-color,box-shadow] duration-[120ms] enabled:hover:border-[#aebdb4] focus:border-[#29a66f] focus:bg-white focus:outline-none focus:shadow-[0_0_0_3px_rgb(41_166_111_/_10%)] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                    class="w-full rounded-lg border border-[#ccd6cf] bg-[#fbfcfb] px-[11px] py-2.5 text-[13px] leading-[1.35] text-[#233029] transition-[border-color,box-shadow] duration-[120ms] enabled:hover:border-[#aebdb4] focus:border-[#29a66f] focus:bg-white focus:outline-none focus:shadow-[0_0_0_3px_rgb(41_166_111_/_10%)] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                <span class="text-[9px] font-extrabold text-[#6b776f] uppercase tracking-[0.1em]">
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
                                    class="h-[15px] w-[15px] p-0 accent-[#19784e] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                <label class="grid grid-cols-[15px_1fr] items-start gap-[7px] rounded-lg border border-[#e0e6e2] bg-[#fafbfa] p-2.5 text-xs font-[720] text-[#344038]">
                                    <input
                                        class="mt-0.5 h-[15px] w-[15px] p-0 accent-[#19784e] disabled:cursor-not-allowed disabled:opacity-[0.58]"
                                        type="radio"
                                        name="thinking-mode"
                                        checked={!isThinkingOverride()}
                                        onChange={() => setIsThinkingOverride(false)}
                                    />
                                    <span class="grid gap-0.5">
                                        <strong class="text-[11px]">Provider defaults</strong>
                                        <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                                            Defaults, not off. No reasoning setting is injected.
                                        </small>
                                    </span>
                                </label>
                                <label class="grid grid-cols-[15px_1fr] items-start gap-[7px] rounded-lg border border-[#e0e6e2] bg-[#fafbfa] p-2.5 text-xs font-[720] text-[#344038]">
                                    <input
                                        class="mt-0.5 h-[15px] w-[15px] p-0 accent-[#19784e] disabled:cursor-not-allowed disabled:opacity-[0.58]"
                                        type="radio"
                                        name="thinking-mode"
                                        checked={isThinkingOverride()}
                                        onChange={() => setIsThinkingOverride(true)}
                                    />
                                    <span class="grid gap-0.5">
                                        <strong class="text-[11px]">Custom supported levels</strong>
                                        <small class="text-[10px] leading-[1.4] font-[450] text-[#7c8880]">
                                            An empty selection means never inject reasoning
                                            configuration.
                                        </small>
                                    </span>
                                </label>
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
                                                    class="h-[15px] w-[15px] p-0 accent-[#19784e] disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
                                        class="inline-flex min-h-10 items-center justify-center rounded-[10px] border border-[#dbe2dc] bg-transparent px-[15px] py-[9px] text-[13px] leading-[1.2] font-[720] text-[#4f5d55] transition-[background,border-color,transform] duration-[140ms] ease-in-out enabled:hover:border-[#b8cbbf] enabled:hover:bg-[#edf4ef] enabled:active:translate-y-px disabled:cursor-not-allowed disabled:opacity-[0.58]"
                                        type="button"
                                        disabled={isSaving()}
                                        onClick={() => resetCreateForm()}
                                    >
                                        Cancel edit
                                    </button>
                                </Show>
                                <button
                                    class="inline-flex min-h-10 items-center justify-center rounded-[10px] border border-[#19784e] bg-[#19784e] px-[15px] py-[9px] text-[13px] leading-[1.2] font-[720] text-white shadow-[0_8px_20px_rgb(25_120_78_/_17%)] transition-[background,border-color,transform] duration-[140ms] ease-in-out enabled:hover:bg-[#126a42] enabled:active:translate-y-px disabled:cursor-not-allowed disabled:opacity-[0.58]"
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
