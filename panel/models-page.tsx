import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import type { ModelSummary, ProviderSummary } from "/common/domain";
import { errorMessage, type PanelApiClient } from "./api-client";
import { providerTypeLabel } from "./labels";
import { ModelEditorForm } from "./model-editor-form";
import { createPageState } from "./page-state";
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

function thinkingSummary(model: ModelSummary): string {
    if (model.thinkingLevels === null) return "Provider defaults";
    if (model.thinkingLevels.length === 0) return "Never inject";
    return model.thinkingLevels.join(", ");
}

/** Provider names cannot contain "/", so the pair is a safe identity key. */
function modelKey(model: ModelSummary): string {
    return `${model.providerName}/${model.upstreamName}`;
}

export function ModelsPage(props: ModelsPageProps): JSX.Element {
    const [models, setModels] = createSignal<ModelSummary[]>([]);
    const [providers, setProviders] = createSignal<ProviderSummary[]>([]);
    const [success, setSuccess] = createSignal<string | null>(null);
    const [busyModelKey, setBusyModelKey] = createSignal<string | null>(null);

    const [editingModel, setEditingModel] = createSignal<ModelSummary | null>(null);
    // A fresh ModelEditorForm mounts per editing session; the key forces that remount.
    const [editorSession, setEditorSession] = createSignal(0);

    async function reloadModels(): Promise<void> {
        setModels(await props.client.listModels());
    }

    function resetEditor(): void {
        setEditingModel(null);
        setEditorSession((session) => session + 1);
    }

    function editModel(model: ModelSummary): void {
        setEditingModel(model);
        setEditorSession((session) => session + 1);
        document.getElementById("model-editor")?.scrollIntoView({
            behavior: "smooth",
        });
    }

    const { isLoading, error, setError, retry } = createPageState(async () => {
        const [loadedProviders, loadedModels] = await Promise.all([
            props.client.listProviders(),
            props.client.listModels(),
        ]);
        setProviders(loadedProviders);
        setModels(loadedModels);
        resetEditor();
    });

    async function handleModelSaved(savedModel: { name: string }): Promise<void> {
        await reloadModels();
        resetEditor();
        setSuccess(`${savedModel.name} saved.`);
    }

    async function toggleModel(model: ModelSummary): Promise<void> {
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
            // The mounted form captured mount-time field values, so when the toggled model is
            // the one being edited, remount the form from the reloaded snapshot; otherwise its
            // next Save would silently revert this toggle through `enabledOverride`.
            const current = editingModel();
            if (current !== null && modelKey(current) === modelKey(model)) {
                setEditingModel(
                    models().find((freshModel) => modelKey(freshModel) === modelKey(model)) ?? null,
                );
                setEditorSession((session) => session + 1);
            }
            setSuccess(`${model.name} ${model.enabled ? "disabled" : "enabled"}.`);
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setBusyModelKey(null);
        }
    }

    async function deleteModel(model: ModelSummary): Promise<void> {
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
            if (current !== null && modelKey(current) === modelKey(model)) {
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
                <Notice kind="error">
                    <span class="flex flex-wrap items-center justify-between gap-x-2.5 gap-y-1">
                        {error()}
                        <button
                            class={tableActionDangerClasses}
                            type="button"
                            onClick={retry}
                        >
                            Retry
                        </button>
                    </span>
                </Notice>
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
                                                            {providerTypeLabel(model.providerType)}
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

                    {/* Function children scope the form subtree to one editorSession() value:
                        only a session value change re-invokes the callback and remounts the
                        form; providers() and editingModel() keep updating it reactively
                        through props without remounting. */}
                    <Show
                        when={editorSession()}
                        keyed
                    >
                        {(_session) => (
                            <ModelEditorForm
                                client={props.client}
                                providers={providers()}
                                editingModel={editingModel()}
                                onSaved={handleModelSaved}
                                onCancelEdit={resetEditor}
                            />
                        )}
                    </Show>
                </div>
            </Show>
        </section>
    );
}
