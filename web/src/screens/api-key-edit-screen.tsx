import type { JSX } from "@solidjs/web";
import { createMemo, createSignal } from "solid-js";
import { toUpdateApiKeyRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { bearerSecretError, isBearerSecret } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import {
    createEditRow,
    createScreenRead,
    isSameApiKeyRow,
    readOne,
    type ScreenRead,
} from "/src/screens/read";
import { ScreenFrame } from "/src/screens/screen-frame";
import { ApiKeyNameField, ApiKeySecretField, apiKeyNameError } from "/src/ui/api-key-form";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { createDeleteAction, createEditForm } from "./edit-form";
import { RowNotFound } from "./row-not-found";

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

    const latestKey = createMemo(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
    );

    // One memo for both consumers — the save notice's last-enabled wording and
    // the delete card's blast-radius question read the same figure.
    const enabledCount = createMemo(
        () => keys().filter((candidate) => candidate.is_enabled).length,
    );

    // The held row with its save-settling latch: an open form with unsaved
    // edits holds its row against refreshes, so an operator's input can never
    // silently vanish, and a succeeded save holds it until its own reload
    // lands (README §6.3).
    const edit = createEditRow(latestKey, isSameApiKeyRow);

    // The notice is also the save's reported outcome, so only a new failure
    // replaces it — a successful read keeps it (README §6.3).
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const keysResult = await readOne(signal, {
                run: (cycle) => props.api.listApiKeys(cycle),
            });

            setKeys(keysResult.api_keys);
            edit.reportReloaded();
        },
    });

    // A refresh answering the same row keeps the row's identity, so the open
    // form is not remounted; a row that genuinely changed on the gateway is
    // held back while the hold stands — and the hold is stated.
    const { row: apiKey, isRowHeld } = edit;

    return (
        <ScreenFrame
            read={read}
            row={apiKey}
            firstReadHeader={
                <PageHeader
                    title="Edit API key"
                    backHref={hrefs.apiKeyDetail(props.apiKeyId)}
                    backLabel="API key"
                />
            }
            notFound={
                <RowNotFound
                    kind="api_key"
                    headerTitle="Edit API key"
                />
            }
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
                        enabledCount={enabledCount()}
                        read={read}
                        onDirtyChange={edit.reportFormDirty}
                        onSaved={edit.reportSaved}
                    />

                    <DangerCard
                        api={props.api}
                        apiKey={row}
                        isLastEnabled={row.is_enabled && enabledCount() <= 1}
                        read={read}
                    />
                </>
            )}
        </ScreenFrame>
    );
}

/** The key's editable settings. Saving writes the complete settings back. */
function SettingsCard(props: {
    readonly api: ManagementApi;
    readonly apiKey: ApiKeyResponse;
    readonly enabledCount: number;
    readonly read: ScreenRead;
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

    const isDirty = (): boolean =>
        isEnabled() !== baseline().is_enabled ||
        name() !== baseline().name ||
        secret() !== baseline().key;

    /** Inline error for the name, shown where it was typed. The stored name
     * is non-empty by contract, so an empty value here is the operator's
     * doing. */
    const nameError = (): string | null => apiKeyNameError(name());

    /** Inline error for the secret, shown where it was typed. Unlike the
     * provider form's write-only field — whose empty value keeps the stored
     * secret — this update restates the secret whole, so an emptied Key is
     * named here: the save gate cannot pass without it. */
    const secretError = (): string | null =>
        secret() === "" ? "A key needs a secret." : bearerSecretError(secret(), "Key");

    const canSave = (): boolean => isDirty() && nameError() === null && isBearerSecret(secret());

    const revert = (): void => {
        setIsEnabled(baseline().is_enabled);
        setName(baseline().name);
        setSecret(baseline().key);
        // Reverting resolves a held row onto the gateway's current version.
        props.read.reload();
    };

    const form = createEditForm({
        read: props.read,
        action: "update-api-key",
        isDirty,
        canSave,
        perform: () =>
            props.api.updateApiKey(
                toUpdateApiKeyRequest(props.apiKey, {
                    name: name(),
                    key: secret(),
                    is_enabled: isEnabled(),
                }),
            ),
        onSucceeded: () => {
            // The write's complete desired state: the update rewrote exactly
            // these fields, so the row they describe is this form's new truth.
            setBaseline({
                ...props.apiKey,
                name: name(),
                key: secret(),
                is_enabled: isEnabled(),
            });

            return {
                tone: "ok",
                title: `${name()} saved.`,
                detail:
                    !isEnabled() && props.apiKey.is_enabled && props.enabledCount <= 1
                        ? "This was the last enabled key: the gateway now serves nothing until a key is turned on."
                        : null,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader
                title="Settings"
                description="Rotating means Generate, then Save."
            />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={form.submit}
            >
                <Switch
                    label="Enabled"
                    isOn={isEnabled()}
                    isDisabled={form.isSaving()}
                    onToggle={setIsEnabled}
                />
                <ApiKeyNameField
                    name={name()}
                    onNameInput={setName}
                    isDisabled={form.isSaving()}
                    error={nameError()}
                />
                <ApiKeySecretField
                    secret={secret()}
                    onSecretInput={setSecret}
                    isDisabled={form.isSaving()}
                    hint="Replacing the secret revokes the old value on save."
                    error={secretError()}
                />
                <DirtyFormFooter
                    canSave={canSave()}
                    isSaving={form.isSaving()}
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
    readonly read: ScreenRead;
}): JSX.Element {
    const confirmDelete = createDeleteAction({
        read: props.read,
        action: "delete-api-key",
        perform: () =>
            props.api.deleteApiKey({
                api_key_id: props.apiKey.id,
            }),
        onDeleted: () => navigate(hrefs.apiKeys()),
    });

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
