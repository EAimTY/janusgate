import type { JSX } from "@solidjs/web";
import { createSignal, For, onSettled, Show } from "solid-js";
import { errorMessage, type Model, type PanelApiClient, type Provider } from "./api-client";
import { ModelEditorForm } from "./model-editor-form";
import {
    buttonPrimaryClasses,
    dataTableClasses,
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

/** Provider names cannot contain "/", so the pair is a safe identity key. */
function modelKey(model: Model): string {
    return `${model.providerName}/${model.upstreamName}`;
}

export function ModelsPage(props: ModelsPageProps): JSX.Element {
    const [models, setModels] = createSignal<Model[]>([]);
    const [providers, setProviders] = createSignal<Provider[]>([]);
    const [isLoading, setIsLoading] = createSignal(true);
    const [error, setError] = createSignal<string | null>(null);
    const [success, setSuccess] = createSignal<string | null>(null);
    const [busyModelKey, setBusyModelKey] = createSignal<string | null>(null);

    const [editingModel, setEditingModel] = createSignal<Model | null>(null);
    // A fresh ModelEditorForm mounts per editing session; the key forces that remount.
    const [editorSession, setEditorSession] = createSignal(0);

    async function reloadModels(): Promise<void> {
        setModels(await props.client.listModels());
    }

    function resetEditor(): void {
        setEditingModel(null);
        setEditorSession((session) => session + 1);
    }

    function editModel(model: Model): void {
        setEditingModel(model);
        setEditorSession((session) => session + 1);
        document.getElementById("model-editor")?.scrollIntoView({
            behavior: "smooth",
        });
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
            resetEditor();
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setIsLoading(false);
        }
    }

    onSettled(() => {
        void initialLoad();
    });

    async function handleModelSaved(savedModel: { name: string }): Promise<void> {
        await reloadModels();
        resetEditor();
        setSuccess(`${savedModel.name} saved.`);
    }

    async function toggleModel(model: Model): Promise<void> {
        setBusyModelKey(modelKey(model));
        setError(null);
        setSuccess(null);
        try {
            await props.client.saveModel({
                providerName: model.providerName,
                upstreamName: model.upstreamName,
                nameOverride: model.nameOverride,
                enabledOverride: !model.enabled,
                thinkingLevelsOverride: model.thinkingLevels,
            });
            await reloadModels();
            setSuccess(`${model.name} ${model.enabled ? "disabled" : "enabled"}.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyModelKey(null);
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
        setBusyModelKey(modelKey(model));
        setError(null);
        setSuccess(null);
        try {
            await props.client.deleteModel(model.providerName, model.upstreamName);
            await reloadModels();
            const current = editingModel();
            if (
                current !== null &&
                current.providerName === model.providerName &&
                current.upstreamName === model.upstreamName
            ) {
                resetEditor();
            }
            setSuccess(`${model.name} deleted.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyModelKey(null);
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
                        onClick={resetEditor}
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
                                                                    busyModelKey() ===
                                                                    modelKey(model)
                                                                }
                                                                onClick={() => editModel(model)}
                                                            >
                                                                Edit
                                                            </button>
                                                            <button
                                                                class={tableActionClasses}
                                                                type="button"
                                                                disabled={
                                                                    busyModelKey() ===
                                                                    modelKey(model)
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
                                                                    busyModelKey() ===
                                                                    modelKey(model)
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

                    {/* The session key remounts a fresh editor per create/edit switch. */}
                    <Show
                        when={editorSession()}
                        keyed
                    >
                        <ModelEditorForm
                            client={props.client}
                            providers={providers()}
                            editingModel={editingModel()}
                            onSaved={handleModelSaved}
                            onCancelEdit={resetEditor}
                        />
                    </Show>
                </div>
            </Show>
        </section>
    );
}
