import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, onCleanup, Show } from "solid-js";
import {
    type AccessInventory,
    isSamePolicy,
    missingReferences,
    newAccessControl,
} from "/src/features/api-keys/access-control";
import { ApiKeyAccess } from "/src/features/api-keys/api-key-access";
import {
    ApiKeyNameField,
    ApiKeySecretField,
    apiKeyNameError,
} from "/src/features/api-keys/api-key-form";
import { readPublishedInventory } from "/src/features/inventory/read";
import { translation } from "/src/i18n";
import { registerLeaveGuardSubject } from "/src/leave-guard-registry";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyAccessControl } from "/src/management-api/generated/v0";
import { bearerSecretError, isBearerSecret } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { createCreateAction } from "/src/screen-state/create-form";
import type { ScreenNotice } from "/src/screen-state/notices";
import { createScreenRead, readOne } from "/src/screen-state/read";
import { Button, Switch } from "/src/ui/controls";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { ReadNotice, StatedNotice } from "/src/ui/read-notice";

/**
 * One creation page for a serving key: whether it works from the start, a
 * human label, and the secret (typed or generated). Creating lands on the
 * new key's page, like a provider creation does.
 */
export function ApiKeyNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [draft, setDraft] = createSignal({
        name: "",
        key: "",
        is_enabled: true,
        access_control: newAccessControl(),
    });
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
    // The name's error is stated only once the operator has typed into the
    // field — a pristine form is not an error, an emptied name is.
    const [hasNameInput, setHasNameInput] = createSignal(false);
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    const [inventory, setInventory] = createSignal<AccessInventory | null>(null);
    const read = createScreenRead({
        load: async (signal) => {
            setInventory(await readOne(signal, readPublishedInventory(props.api, null)));
        },
    });
    const seat = registerLeaveGuardSubject();
    createEffect(
        () =>
            name() !== "" ||
            secret() !== "" ||
            !isEnabled() ||
            !isSamePolicy(policy(), newAccessControl()),
        seat.reportGuarded,
    );
    onCleanup(seat.release);

    /** Inline error for the name, shown where it was typed. */
    const nameError = (): string | null => (hasNameInput() ? apiKeyNameError(name()) : null);

    /** Inline composition error for the secret, shown where it was typed. */
    const secretError = (): string | null => bearerSecretError(secret(), "key");

    /** The creation's composition rules: a named key carrying a complete secret. */
    const canCompose = (): boolean => {
        const current = inventory();
        return (
            name() !== "" &&
            isBearerSecret(secret()) &&
            current !== null &&
            missingReferences(policy(), current).length === 0
        );
    };

    const create = createCreateAction({
        action: "create-api-key",
        channel: setFailure,
        canRun: canCompose,
        perform: async () => ({
            status: "called" as const,
            outcome: await performManagementCall(() =>
                props.api.createApiKey({
                    name: name(),
                    key: secret(),
                    is_enabled: isEnabled(),
                    access_control: policy(),
                }),
            ),
        }),
        onCreated: (result) => {
            seat.reportGuarded(false);
            navigate(hrefs.apiKeyDetail(result.api_key.id));
        },
    });

    const canCreate = (): boolean => canCompose() && !create.isCreating();

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title={translation()(($) => $.screens.apiKeyNewScreen.createAPIKey)}
                description={translation()(
                    ($) => $.screens.apiKeyNewScreen.oneSecretYourServingClientsShareMessage,
                )}
                backHref={hrefs.apiKeys()}
                backLabel={translation()(($) => $.screens.apiKeyNewScreen.apiKeysBackLabel)}
            />

            <ReadNotice
                read={read}
                retry={read.reload}
            />
            <StatedNotice notice={failure} />

            <Card>
                <CardHeader
                    title={translation()(($) => $.screens.apiKeyNewScreen.key)}
                    description={translation()(
                        ($) =>
                            $.screens.apiKeyNewScreen
                                .theSecretHoldsLettersNumbersAndSymbolsOnlyMessage,
                    )}
                />
                <form
                    novalidate={true}
                    class="mt-4 flex max-w-2xl flex-col gap-4"
                    onSubmit={create.submit}
                >
                    <Switch
                        label={translation()(($) => $.screens.apiKeyNewScreen.enabled)}
                        isOn={isEnabled()}
                        isDisabled={create.isCreating()}
                        onToggle={setIsEnabled}
                    />
                    <ApiKeyNameField
                        name={name()}
                        onNameInput={(value) => {
                            setHasNameInput(true);
                            setName(value);
                        }}
                        isDisabled={create.isCreating()}
                        error={nameError()}
                    />
                    <ApiKeySecretField
                        secret={secret()}
                        onSecretInput={setSecret}
                        isDisabled={create.isCreating()}
                        error={secretError()}
                    />
                    <Show
                        when={inventory()}
                        fallback={
                            <p class="text-[13px] text-ink-faint">
                                {translation()(
                                    ($) =>
                                        $.screens.apiKeyNewScreen
                                            .loadModelAccessInventoryBeforeCreatingAKeyMessage,
                                )}
                            </p>
                        }
                    >
                        {(current) => (
                            <ApiKeyAccess
                                policy={policy()}
                                inventory={current()}
                                onChange={setPolicy}
                                isDisabled={create.isCreating()}
                            />
                        )}
                    </Show>
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!canCreate()}
                            isBusy={create.isCreating()}
                        >
                            {create.isCreating()
                                ? translation()(($) => $.screens.apiKeyNewScreen.creating)
                                : translation()(($) => $.screens.apiKeyNewScreen.createAPIKey)}
                        </Button>
                    </div>
                </form>
            </Card>
        </div>
    );
}
