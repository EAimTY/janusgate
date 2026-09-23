import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { toUpdateApiKeyRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { compositionMessage, isBearerSecret } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import {
    createHeldRow,
    createScreenRead,
    type FailureCall,
    isSameApiKeyRow,
    readAll,
    type ScreenNotice,
} from "/src/screens/read";
import { ApiKeyNameField, ApiKeySecretField, apiKeyNameError } from "/src/ui/api-key-form";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { ButtonLink, Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { EmptyState, Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

/**
 * One serving key's edit page: its settings and its deletion. The form opens
 * prefilled, tracks dirt, and saves the complete settings back. Reading the
 * key — its secret, its facts, its usage — lives on the detail page.
 */
export function ApiKeyEditScreen(props: {
    readonly api: ManagementApi;
    readonly apiKeyId: number;
}): JSX.Element {
    const [keys, setKeys] = createSignal<ReadonlyArray<ApiKeyResponse>>([]);
    // Reported by the settings form: an open form with unsaved edits holds its
    // row against refreshes, so an operator's input can never silently vanish.
    const [isFormDirty, setIsFormDirty] = createSignal(false);
    // Set when a save succeeds: the standing listing is then stale by
    // construction — the save overwrote exactly what the listing still
    // shows — so the held row must not adopt it before the save's own reload
    // lands (README §6.3: a save re-baselines the form onto the values it
    // sent). The latch clears on the first read cycle that starts after the
    // save, so a failed reload leaves the hold standing instead of resting
    // the screen on the pre-save rows.
    const [isSaveSettling, setIsSaveSettling] = createSignal(false);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const [keysResult] = await readAll(signal, {
                run: (cycle) => props.api.listApiKeys(cycle),
            });

            setKeys(keysResult.api_keys);
            setIsSaveSettling(false);
        },
    });

    const latestKey = createMemo(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
    );

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the form holds unsaved edits — or while a save is
    // still settling — and the hold is stated.
    const { row: apiKey, isRowHeld } = createHeldRow(
        latestKey,
        () => isFormDirty() || isSaveSettling(),
        isSameApiKeyRow,
    );

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={read.notice()}>
                {(current) => (
                    <Notice
                        tone={current().tone}
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Show when={read.isReading() && !read.hasRead() && read.notice() === null}>
                <PageHeader
                    title="Edit API key"
                    backHref={hrefs.apiKeyDetail(props.apiKeyId)}
                    backLabel="API key"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={read.hasRead() && apiKey() === null}>
                <PageHeader
                    title="Edit API key"
                    backHref={hrefs.apiKeys()}
                    backLabel="API keys"
                />
                <EmptyState
                    title="API key not found"
                    description="The gateway holds no API key here. It was deleted — on this page, in another session, or by hand. Nothing here can bring it back."
                >
                    <ButtonLink
                        href={hrefs.apiKeys()}
                        tone="primary"
                    >
                        Back to API keys
                    </ButtonLink>
                </EmptyState>
            </Show>

            <Show
                when={apiKey()}
                keyed
            >
                {(row) => (
                    <>
                        <PageHeader
                            title={`Edit ${row.name}`}
                            backHref={hrefs.apiKeyDetail(row.id)}
                            backLabel="API key"
                        />

                        <HoldNotice
                            isHeld={isRowHeld()}
                            subject="key"
                        />

                        <SettingsCard
                            api={props.api}
                            apiKey={row}
                            enabledCount={keys().filter((candidate) => candidate.is_enabled).length}
                            onReload={read.reload}
                            onNotice={read.showNotice}
                            onFailure={read.reportFailure}
                            onDirtyChange={setIsFormDirty}
                            onSaved={() => setIsSaveSettling(true)}
                        />

                        <DangerCard
                            api={props.api}
                            apiKey={row}
                            isLastEnabled={
                                row.is_enabled &&
                                keys().filter((candidate) => candidate.is_enabled).length <= 1
                            }
                            onReload={read.reload}
                            onFailure={read.reportFailure}
                        />
                    </>
                )}
            </Show>
        </div>
    );
}

/** The key's editable settings. Saving writes the complete settings back. */
function SettingsCard(props: {
    readonly api: ManagementApi;
    readonly apiKey: ApiKeyResponse;
    readonly enabledCount: number;
    readonly onReload: () => void;
    readonly onNotice: (notice: ScreenNotice | null) => void;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the values it sent, because api-keys:update answers `{}` and
    // the write's result is otherwise known only to this form.
    const [baseline, setBaseline] = createSignal(props.apiKey);
    const [isEnabled, setIsEnabled] = createSignal(props.apiKey.is_enabled);
    const [name, setName] = createSignal(props.apiKey.name);
    const [secret, setSecret] = createSignal(props.apiKey.key);
    const [isSaving, setIsSaving] = createSignal(false);

    const isDirty = (): boolean =>
        isEnabled() !== baseline().is_enabled ||
        name() !== baseline().name ||
        secret() !== baseline().key;

    // The screen holds the form's row while this dirt stands (README §6.3).
    createEffect(
        () => isDirty(),
        (isDirtyValue) => {
            props.onDirtyChange(isDirtyValue);
        },
    );

    /** Inline error for the name, shown where it was typed. The stored name
     * is non-empty by contract, so an empty value here is the operator's
     * doing. */
    const nameError = (): string | null => apiKeyNameError(name());

    /** Inline composition error for the secret, shown where it was typed. */
    const secretError = (): string | null =>
        secret() === "" || isBearerSecret(secret()) ? null : compositionMessage("Key");

    const canSave = (): boolean => isDirty() && nameError() === null && isBearerSecret(secret());

    const revert = (): void => {
        setIsEnabled(baseline().is_enabled);
        setName(baseline().name);
        setSecret(baseline().key);
        // Reverting resolves a held row onto the gateway's current version.
        props.onReload();
    };

    const save = async (): Promise<void> => {
        if (isSaving() || !canSave()) {
            return;
        }

        setIsSaving(true);

        const outcome = await performManagementCall(() =>
            props.api.updateApiKey(
                toUpdateApiKeyRequest(props.apiKey, {
                    name: name(),
                    key: secret(),
                    is_enabled: isEnabled(),
                }),
            ),
        );

        setIsSaving(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, {
                action: "update-api-key",
            });

            if (diagnosis.kind === "gone") {
                // The row is gone behind the screen: the listing re-reads so
                // the not-found state takes over instead of a ghost form
                // accepting re-saves forever (README §6.4).
                props.onReload();
            }

            props.onFailure(outcome.cause, {
                action: "update-api-key",
            });
            return;
        }

        // The write's complete desired state: the update rewrote exactly
        // these fields, so the row they describe is this form's new truth.
        setBaseline({
            ...props.apiKey,
            name: name(),
            key: secret(),
            is_enabled: isEnabled(),
        });
        // The hold stands until this save's reload lands: the listing still
        // answers what the save overwrote.
        props.onSaved();
        props.onNotice({
            tone: "ok",
            title: `${name()} saved.`,
            detail:
                !isEnabled() && props.apiKey.is_enabled && props.enabledCount <= 1
                    ? "This was the last enabled key: the gateway now serves nothing until a key is turned on."
                    : null,
        });
        props.onReload();
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void save();
    };

    return (
        <Card>
            <CardHeader
                title="Settings"
                description="Rotating means Generate, then Save."
            />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={submit}
            >
                <Switch
                    label="Enabled"
                    isOn={isEnabled()}
                    onText="On"
                    offText="Off"
                    isDisabled={isSaving()}
                    onToggle={setIsEnabled}
                />
                <ApiKeyNameField
                    name={name()}
                    onNameInput={setName}
                    isDisabled={isSaving()}
                    error={nameError()}
                />
                <ApiKeySecretField
                    secret={secret()}
                    onSecretInput={setSecret}
                    isDisabled={isSaving()}
                    hint="Replacing the secret revokes the old value on save."
                    error={secretError()}
                />
                <DirtyFormFooter
                    canSave={canSave()}
                    isSaving={isSaving()}
                    isDirty={isDirty()}
                    onRevert={revert}
                />
            </form>
        </Card>
    );
}

/** Revocation, confirmed where it states its blast radius. */
function DangerCard(props: {
    readonly api: ManagementApi;
    readonly apiKey: ApiKeyResponse;
    readonly isLastEnabled: boolean;
    readonly onReload: () => void;
    readonly onFailure: (cause: unknown, call?: FailureCall) => void;
}): JSX.Element {
    const confirmDelete = async (): Promise<boolean> => {
        const outcome = await performManagementCall(() =>
            props.api.deleteApiKey({
                api_key_id: props.apiKey.id,
            }),
        );

        if (outcome.status === "aborted") {
            return false;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, {
                action: "delete-api-key",
            });

            if (diagnosis.kind === "gone") {
                // The row is already gone: the listing re-reads so the
                // not-found state takes over (README §6.4).
                props.onReload();
            }

            props.onFailure(outcome.cause, {
                action: "delete-api-key",
            });
            return false;
        }

        navigate(hrefs.apiKeys());
        return true;
    };

    return (
        <ConfirmDeleteCard
            title="Delete this key"
            description="Deleting revokes the key at once. Clients holding it start getting 401s. Nothing else is removed."
            question={
                props.isLastEnabled
                    ? `Delete ${props.apiKey.name}? It is the last enabled key — the gateway will serve nothing until another key is turned on.`
                    : `Delete ${props.apiKey.name}? Clients holding this secret lose access.`
            }
            onConfirm={confirmDelete}
        />
    );
}
