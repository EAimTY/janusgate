import type { JSX } from "@solidjs/web";
import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { toUpdateApiKeyRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { bearerSecretError, isBearerSecret } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import {
    createEditRow,
    createScreenRead,
    isSameApiKeyRow,
    readAll,
    type ScreenRead,
} from "/src/screens/read";
import { ReadNotice } from "/src/screens/read-notice";
import { ApiKeyNameField, ApiKeySecretField, apiKeyNameError } from "/src/ui/api-key-form";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { ButtonLink, Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { EmptyState, Skeleton } from "/src/ui/feedback";
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
            const [keysResult] = await readAll(signal, {
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
        <div class="rise-in flex flex-col gap-5">
            <ReadNotice read={read} />

            <Show when={read.isFirstRead()}>
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
            </Show>
        </div>
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
    const secretError = (): string | null => bearerSecretError(secret(), "Key");

    const canSave = (): boolean => isDirty() && nameError() === null && isBearerSecret(secret());

    const revert = (): void => {
        setIsEnabled(baseline().is_enabled);
        setName(baseline().name);
        setSecret(baseline().key);
        // Reverting resolves a held row onto the gateway's current version.
        props.read.reload();
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

        if (
            !props.read.reportAction(
                outcome,
                {
                    action: "update-api-key",
                },
                {
                    onGone: props.read.reload,
                },
            )
        ) {
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
        props.read.showNotice({
            tone: "ok",
            title: `${name()} saved.`,
            detail:
                !isEnabled() && props.apiKey.is_enabled && props.enabledCount <= 1
                    ? "This was the last enabled key: the gateway now serves nothing until a key is turned on."
                    : null,
        });
        props.read.reload();
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
    readonly read: ScreenRead;
}): JSX.Element {
    const confirmDelete = async (): Promise<boolean> => {
        const outcome = await performManagementCall(() =>
            props.api.deleteApiKey({
                api_key_id: props.apiKey.id,
            }),
        );

        if (
            !props.read.reportAction(
                outcome,
                {
                    action: "delete-api-key",
                },
                {
                    onGone: props.read.reload,
                },
            )
        ) {
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
