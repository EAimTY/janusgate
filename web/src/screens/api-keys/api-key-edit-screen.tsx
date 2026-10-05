import type { JSX } from "@solidjs/web";
import { createMemo, createSignal } from "solid-js";
import {
    type AccessInventory,
    isSamePolicy,
    missingReferences,
} from "/src/features/api-keys/access-control";
import { ApiKeyAccess } from "/src/features/api-keys/api-key-access";
import {
    ApiKeyNameField,
    ApiKeySecretField,
    apiKeyNameError,
} from "/src/features/api-keys/api-key-form";
import { isSameApiKeyRow } from "/src/features/api-keys/row-equality";
import { readApiKeys, readPublishedInventory } from "/src/features/inventory/read";
import { translation } from "/src/i18n";
import { deleteKeyMessage, deleteLastKeyMessage, editNameMessage } from "/src/i18n/rich";
import { toUpdateApiKeyRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyAccessControl, ApiKeyResponse } from "/src/management-api/generated/v0";
import { bearerSecretError, isBearerSecret } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import { createDeleteAction, createEditForm } from "/src/screen-state/edit-form";
import { createScreenRead, readAll, type ScreenRead } from "/src/screen-state/read";
import { createEditRow } from "/src/screen-state/row-state";
import { apiKeySavedNotice } from "/src/screen-state/success-notices";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Switch } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

/**
 * One serving key's edit page: its settings and its deletion. The form opens
 * prefilled, tracks dirt, and saves the complete settings back. Reading the
 * key — its secret, its facts, its usage — lives on the detail page.
 */
export function ApiKeyEditScreen(props: {
    readonly api: ManagementApi;
    readonly apiKeyId: number;
}): JSX.Element {
    const [inventory, setInventory] = createSignal<AccessInventory | null>(null);
    const [keys, setKeys] = createSignal<ReadonlyArray<ApiKeyResponse>>([]);

    const latestKey = createMemo(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
    );

    // The enabled count over the OTHER keys: the save notice's last-enabled
    // wording reads it at the moment the save settles, and the delete card's
    // blast-radius question reads it beside the row's own enabledness. Both
    // judge a row the form may hold against a listing that may already answer
    // a write: a refresh read in flight when Save was clicked may land after
    // the gateway applied the write, so the standing listing can already
    // answer the post-write state — where the saved row itself has left the
    // enabled count, and its figure there proves nothing about the others.
    // Excluding the row by id is correct under both orderings: a pre-write
    // listing still counts the row enabled, but the id filter drops it; a
    // post-write listing has already dropped it from the enabled set. Each
    // question's row-side half — whether this row is enabled — comes from the
    // row prop, not the listing.
    const otherEnabledCount = createMemo(
        () =>
            keys().filter((candidate) => candidate.is_enabled && candidate.id !== props.apiKeyId)
                .length,
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
            const [keysResult, inventoryResult] = await readAll(
                signal,
                readApiKeys(props.api),
                readPublishedInventory(props.api, null),
            );
            setInventory(inventoryResult);

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
                    title={translation()(($) => $.screens.apiKeyEditScreen.editAPIKey)}
                    backHref={hrefs.apiKeyDetail(props.apiKeyId)}
                    backLabel={translation()(($) => $.additional.apiKeyEditScreen.apiKey)}
                />
            }
            notFound={
                <RowNotFound
                    kind="api_key"
                    headerTitle={translation()(($) => $.additional.apiKeyEditScreen.editAPIKey)}
                />
            }
        >
            {(row) => (
                <>
                    <PageHeader
                        title={editNameMessage(row.name)}
                        backHref={hrefs.apiKeyDetail(row.id)}
                        backLabel={translation()(($) => $.additional.apiKeyEditScreen.apiKey)}
                    />

                    <HoldNotice
                        isHeld={isRowHeld()}
                        subject="key"
                    />

                    <SettingsCard
                        api={props.api}
                        apiKey={row}
                        inventory={inventory()}
                        otherEnabledCount={otherEnabledCount()}
                        read={read}
                        onDirtyChange={edit.reportFormDirty}
                        onSaved={edit.reportSaved}
                    />

                    <DangerCard
                        api={props.api}
                        apiKey={row}
                        isLastEnabled={row.is_enabled && otherEnabledCount() === 0}
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
    readonly inventory: AccessInventory | null;
    readonly otherEnabledCount: number;
    readonly read: ScreenRead;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    // The settings as the gateway last returned them; a save re-baselines
    // them onto the values it sent, because api-keys:update answers `{}` and
    // the write's result is otherwise known only to this form.
    const [baseline, setBaseline] = createSignal(props.apiKey);
    const [draft, setDraft] = createSignal(props.apiKey);
    const name = (): string => draft().name;
    const secret = (): string => draft().key;
    const isEnabled = (): boolean => draft().is_enabled;
    const policy = (): ApiKeyAccessControl => draft().access_control;
    const setName = (name: string): void => {
        setDraft((current) => ({
            ...current,
            name,
        }));
    };
    const setSecret = (key: string): void => {
        setDraft((current) => ({
            ...current,
            key,
        }));
    };
    const setIsEnabled = (is_enabled: boolean): void => {
        setDraft((current) => ({
            ...current,
            is_enabled,
        }));
    };
    const setPolicy = (access_control: ApiKeyAccessControl): void => {
        setDraft((current) => ({
            ...current,
            access_control,
        }));
    };

    const isDirty = (): boolean =>
        isEnabled() !== baseline().is_enabled ||
        name() !== baseline().name ||
        secret() !== baseline().key ||
        !isSamePolicy(policy(), baseline().access_control);

    /** Inline error for the name, shown where it was typed. The stored name
     * is non-empty by contract, so an empty value here is the operator's
     * doing. */
    const nameError = (): string | null => apiKeyNameError(name());

    /** Inline error for the secret, shown where it was typed. Unlike the
     * provider form's write-only field — whose empty value keeps the stored
     * secret — this update restates the secret whole, so an emptied Key is
     * named here: the save gate cannot pass without it. */
    const secretError = (): string | null =>
        secret() === ""
            ? translation()(($) => $.helpers.apiKeyEditScreen.aKeyNeedsASecret)
            : bearerSecretError(secret(), "key");

    const canSave = (): boolean => {
        const inventory = props.inventory;
        return (
            isDirty() &&
            nameError() === null &&
            isBearerSecret(secret()) &&
            inventory !== null &&
            missingReferences(policy(), inventory).length === 0
        );
    };

    const revert = (): void => {
        setDraft(baseline());
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
                    access_control: policy(),
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
                access_control: policy(),
            });

            // Both consequences read the row the form opened on: the save
            // latches its hold and starts its reload only after this notice
            // is built (createEditForm), so `props.apiKey` still answers the
            // pre-save truth here. The blast radius reads the OTHER keys'
            // enablement at the moment this notice is built: the standing
            // listing may already answer the write being reported, so the
            // saved row's own figure there proves nothing about the others.
            const isRotatingSecret = secret() !== props.apiKey.key;
            const isTurningOffLastEnabled =
                !isEnabled() && props.apiKey.is_enabled && props.otherEnabledCount === 0;

            return apiKeySavedNotice({
                name: name(),
                isRotatingSecret,
                isTurningOffLastEnabled,
            });
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });

    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.screens.apiKeyEditScreen.settings)}
                description={translation()(
                    ($) => $.screens.apiKeyEditScreen.rotatingMeansGenerateThenSaveMessage,
                )}
            />
            <form
                novalidate={true}
                class="mt-4 flex max-w-2xl flex-col gap-4"
                onSubmit={form.submit}
            >
                <Switch
                    label={translation()(($) => $.screens.apiKeyEditScreen.enabled)}
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
                    // The field opens prefilled with the row's serving
                    // secret — nothing was typed — so the confirm names the
                    // current value this save will retire, and the reveal is
                    // how the operator inspects that live value before
                    // rotating.
                    generateConfirmSubject="current"
                    revealLabel={translation()(
                        ($) => $.additional.apiKeyEditScreen.showTheCurrentKey,
                    )}
                    hint={translation()(
                        ($) =>
                            $.screens.apiKeyEditScreen
                                .replacingTheSecretRevokesTheOldValueOnMessage,
                    )}
                    error={secretError()}
                />
                {props.inventory && (
                    <ApiKeyAccess
                        policy={policy()}
                        inventory={props.inventory}
                        onChange={setPolicy}
                        isDisabled={form.isSaving()}
                    />
                )}
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
            title={translation()(($) => $.screens.apiKeyEditScreen.deleteThisKey)}
            description={translation()(
                ($) => $.screens.apiKeyEditScreen.deletingRevokesTheKeyAtOnceClientsHoldingMessage,
            )}
            question={
                props.isLastEnabled
                    ? deleteLastKeyMessage(props.apiKey.name)
                    : deleteKeyMessage(translation(), props.apiKey.name)
            }
            onConfirm={confirmDelete}
        />
    );
}
