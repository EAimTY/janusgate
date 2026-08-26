import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import { UPSTREAM_PROTOCOLS } from "/common/domain";
import {
    MODEL_PREFIX_MAX_LENGTH,
    MODEL_PREFIX_PATTERN,
    PROVIDER_NAME_MAX_LENGTH,
    PROVIDER_NAME_PATTERN,
} from "/config";
import {
    type CompatibleProtocol,
    errorMessage,
    type PanelApiClient,
    type Provider,
} from "./api-client";
import {
    buttonGhostClasses,
    buttonPrimaryClasses,
    checkboxClasses,
    Dialog,
    formControlClasses,
    Notice,
} from "./ui";

interface ProviderEditorDialogProps {
    client: PanelApiClient;
    /** `null` opens the API-key creation form; a provider opens its edit form. */
    editingProvider: Provider | null;
    onClose: () => void;
    /** Reloads the list, records the outcome, and closes the dialog. */
    onSaved: (providerName: string, isCreation: boolean) => Promise<void>;
}

export function ProviderEditorDialog(props: ProviderEditorDialogProps): JSX.Element {
    const editingProvider = props.editingProvider;
    const isCompatible = editingProvider === null || editingProvider.type === "openai_compatible";
    const [editorName, setEditorName] = createSignal(editingProvider?.name ?? "");
    const [editorPrefix, setEditorPrefix] = createSignal(editingProvider?.modelPrefix ?? "");
    const [editorBaseUrl, setEditorBaseUrl] = createSignal(editingProvider?.baseUrl ?? "");
    const [editorProtocol, setEditorProtocol] = createSignal<CompatibleProtocol>(
        editingProvider?.protocol ?? "openai_response",
    );
    const [editorApiKey, setEditorApiKey] = createSignal("");
    const [isEditorEnabled, setIsEditorEnabled] = createSignal(editingProvider?.enabled ?? true);
    const [isSavingEditor, setIsSavingEditor] = createSignal(false);
    const [editorError, setEditorError] = createSignal<string | null>(null);

    async function saveEditor(event: SubmitEvent): Promise<void> {
        event.preventDefault();
        const isCreation = editingProvider === null;
        setIsSavingEditor(true);
        setEditorError(null);
        try {
            let savedName: string;
            if (editingProvider === null || editingProvider.type === "openai_compatible") {
                const input = {
                    name: editorName().trim(),
                    protocol: editorProtocol(),
                    baseUrl: editorBaseUrl().trim(),
                    enabled: isEditorEnabled(),
                    modelPrefix: editorPrefix(),
                    ...(isCreation
                        ? {
                              type: "openai_compatible" as const,
                          }
                        : {}),
                    ...(editorApiKey().length > 0
                        ? {
                              apiKey: editorApiKey(),
                          }
                        : {}),
                };
                savedName = await props.client.saveCompatibleProvider(input);
            } else {
                savedName = await props.client.saveOAuthProvider({
                    name: editingProvider.name,
                    enabled: isEditorEnabled(),
                    modelPrefix: editorPrefix(),
                });
            }
            await props.onSaved(savedName, isCreation);
        } catch (caughtError) {
            setEditorError(errorMessage(caughtError));
        } finally {
            setIsSavingEditor(false);
        }
    }

    return (
        <Dialog
            isOpen
            isBusy={isSavingEditor()}
            title={editingProvider === null ? "Add API-key provider" : "Edit provider"}
            description="Changing a prefix affects only models created or discovered later."
            onClose={props.onClose}
        >
            <form
                class="grid gap-[17px] px-6 pt-[22px] pb-6 [&>[role]]:mb-0 [&_label>small]:text-[10px] [&_label>small]:leading-[1.4] [&_label>small]:font-[450] [&_label>small]:text-[#7c8880]"
                onSubmit={(event) => void saveEditor(event)}
            >
                <Show when={editorError() !== null}>
                    <Notice kind="error">{editorError()}</Notice>
                </Show>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Provider name
                    <input
                        class={formControlClasses}
                        required
                        pattern={PROVIDER_NAME_PATTERN.source}
                        maxlength={PROVIDER_NAME_MAX_LENGTH}
                        disabled={editingProvider !== null}
                        value={editorName()}
                        onInput={(event) => setEditorName(event.currentTarget.value)}
                    />
                    <small>
                        1–{PROVIDER_NAME_MAX_LENGTH} lowercase letters, numbers, underscores, or
                        hyphens.
                    </small>
                </label>
                <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                    Model prefix
                    <input
                        class={formControlClasses}
                        maxlength={MODEL_PREFIX_MAX_LENGTH}
                        pattern={MODEL_PREFIX_PATTERN.source}
                        value={editorPrefix()}
                        onInput={(event) => setEditorPrefix(event.currentTarget.value)}
                    />
                    <small>Verbatim prefix; include your own separator. Blank means none.</small>
                </label>
                <Show when={isCompatible}>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Base URL
                        <input
                            class={formControlClasses}
                            required
                            type="url"
                            placeholder="https://api.example.com/v1"
                            value={editorBaseUrl()}
                            onInput={(event) => setEditorBaseUrl(event.currentTarget.value)}
                        />
                    </label>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        Protocol
                        <select
                            class={formControlClasses}
                            value={editorProtocol()}
                            onChange={(event) => {
                                const value = event.currentTarget.value;
                                for (const protocol of UPSTREAM_PROTOCOLS) {
                                    if (value === protocol) setEditorProtocol(protocol);
                                }
                            }}
                        >
                            <option value="openai_response">Responses</option>
                            <option value="openai_chat_completion">Chat Completions</option>
                            <option value="gemini_generate_content">Gemini GenerateContent</option>
                        </select>
                        <small>
                            Gemini endpoints use the x-goog-api-key header; the others use a bearer
                            token.
                        </small>
                    </label>
                    <label class="grid gap-[7px] text-xs font-[720] text-[#344038]">
                        API key {editingProvider === null ? "" : "(leave blank to preserve)"}
                        <input
                            class={formControlClasses}
                            required={editingProvider === null}
                            type="password"
                            autocomplete="off"
                            value={editorApiKey()}
                            onInput={(event) => setEditorApiKey(event.currentTarget.value)}
                        />
                    </label>
                </Show>
                <label class="flex items-center gap-[7px] text-xs font-[720] text-[#344038]">
                    <input
                        class={checkboxClasses}
                        type="checkbox"
                        checked={isEditorEnabled()}
                        onChange={(event) => setIsEditorEnabled(event.currentTarget.checked)}
                    />
                    Enabled
                </label>
                <footer class="flex items-center justify-end gap-[9px] pt-[3px] max-[540px]:flex-col-reverse max-[540px]:items-stretch max-[540px]:[&>button]:w-full">
                    <button
                        class={buttonGhostClasses}
                        type="button"
                        disabled={isSavingEditor()}
                        onClick={props.onClose}
                    >
                        Cancel
                    </button>
                    <button
                        class={buttonPrimaryClasses}
                        type="submit"
                        disabled={isSavingEditor()}
                    >
                        {isSavingEditor() ? "Saving…" : "Save provider"}
                    </button>
                </footer>
            </form>
        </Dialog>
    );
}
