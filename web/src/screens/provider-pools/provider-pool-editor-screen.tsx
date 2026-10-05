import type { JSX } from "@solidjs/web";
import { createSignal, For, Show } from "solid-js";
import { PoolAvailabilityCard } from "/src/features/provider-pools/pool-availability-card";
import { translation } from "/src/i18n";
import { editNameMessage } from "/src/i18n/rich";
import type { ManagementApi } from "/src/management-api/client";
import type {
    ModelResponse,
    ProviderPoolResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { isRoutingToken, routingTokenError } from "/src/management-api/identifiers";
import { hrefs, navigate } from "/src/router";
import { createDeleteAction, createEditForm } from "/src/screen-state/edit-form";
import { createScreenRead, readAll, type ScreenRead } from "/src/screen-state/read";
import { createEditRow } from "/src/screen-state/row-state";
import { ConfirmDeleteCard } from "/src/ui/confirm-delete-card";
import { Button, SelectField, Switch, TextField } from "/src/ui/controls";
import { DirtyFormFooter, HoldNotice } from "/src/ui/dirty-form-footer";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { RowNotFound } from "/src/ui/row-not-found";
import { ScreenFrame } from "/src/ui/screen-frame";

type PoolSeed = Omit<ProviderPoolResponse, "id"> & {
    readonly id: number | null;
};

/** Ordered membership is authored configuration; identity compares the complete state. */
function isSamePool(left: PoolSeed, right: PoolSeed): boolean {
    return (
        left.id === right.id &&
        left.name === right.name &&
        left.model_prefix === right.model_prefix &&
        left.is_enabled === right.is_enabled &&
        left.members.length === right.members.length &&
        left.members.every(
            (member, index) =>
                member.provider_id === right.members[index]?.provider_id &&
                member.priority === right.members[index]?.priority,
        )
    );
}

/** Creates or edits a pool, holding an open form against background refreshes. */
export function ProviderPoolEditorScreen(props: {
    readonly api: ManagementApi;
    readonly poolId: number | null;
}): JSX.Element {
    const [seed, setSeed] = createSignal<PoolSeed | null>(null);
    const [providers, setProviders] = createSignal<ReadonlyArray<ProviderResponse>>([]);
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [inspectionId, setInspectionId] = createSignal("");
    const edit = createEditRow(seed, isSamePool);
    const read = createScreenRead({
        isNoticeKeptOnSuccess: true,
        load: async (signal) => {
            const [pools, accounts, inventory] = await readAll(
                signal,
                {
                    run: (signal) => props.api.listProviderPools(signal),
                },
                {
                    run: (signal) => props.api.listProviders(signal),
                },
                {
                    run: (signal) => props.api.listModels({}, signal),
                },
            );
            setProviders(accounts.providers);
            setModels(inventory.models);
            setSeed(
                props.poolId === null
                    ? {
                          id: null,
                          name: "",
                          model_prefix: "",
                          is_enabled: true,
                          members: [],
                      }
                    : (pools.provider_pools.find((pool) => pool.id === props.poolId) ?? null),
            );
            edit.reportReloaded();
        },
    });
    return (
        <ScreenFrame
            read={read}
            row={edit.row}
            firstReadHeader={
                <PageHeader
                    title={
                        props.poolId === null
                            ? translation()(($) => $.additional.providerPoolEditorScreen.createPool)
                            : translation()(($) => $.additional.providerPoolEditorScreen.editPool)
                    }
                    backHref={hrefs.providerPools()}
                    backLabel={translation()(($) => $.additional.providerPoolEditorScreen.pools)}
                />
            }
            notFound={
                <RowNotFound
                    kind="pool"
                    headerTitle={translation()(
                        ($) => $.additional.providerPoolEditorScreen.editPool,
                    )}
                />
            }
        >
            {(pool) => {
                const savedPool =
                    pool.id === null
                        ? null
                        : {
                              ...pool,
                              id: pool.id,
                          };
                return (
                    <>
                        <PageHeader
                            title={
                                pool.id === null
                                    ? translation()(
                                          ($) => $.additional.providerPoolEditorScreen.createPool,
                                      )
                                    : editNameMessage(pool.name)
                            }
                            description={translation()(
                                ($) =>
                                    $.screens.providerPoolEditorScreen
                                        .interchangeableAccountsForTheSameUpstreamServiceTriedMessage,
                            )}
                            backHref={hrefs.providerPools()}
                            backLabel={translation()(
                                ($) => $.additional.providerPoolEditorScreen.pools,
                            )}
                        />
                        <HoldNotice
                            isHeld={edit.isRowHeld()}
                            subject="pool"
                        />
                        <PoolForm
                            api={props.api}
                            seed={pool}
                            providers={providers()}
                            read={read}
                            onDirtyChange={edit.reportFormDirty}
                            onSaved={edit.reportSaved}
                        />
                        <Show
                            when={savedPool}
                            keyed
                        >
                            {(savedPool) => (
                                <>
                                    <TextField
                                        label={translation()(
                                            ($) =>
                                                $.screens.providerPoolEditorScreen
                                                    .inspectUpstreamID,
                                        )}
                                        value={inspectionId()}
                                        onInput={setInspectionId}
                                        hint={translation()(
                                            ($) =>
                                                $.screens.providerPoolEditorScreen
                                                    .inspectsSavedMembershipLeavingThisEmptyShowsAccountwideMessage,
                                        )}
                                    />
                                    <PoolAvailabilityCard
                                        api={props.api}
                                        pool={savedPool}
                                        providers={providers()}
                                        models={models()}
                                        upstreamId={inspectionId() === "" ? null : inspectionId()}
                                    />
                                </>
                            )}
                        </Show>
                        {pool.id !== null && (
                            <PoolDelete
                                api={props.api}
                                poolId={pool.id}
                                read={read}
                            />
                        )}
                    </>
                );
            }}
        </ScreenFrame>
    );
}

function PoolDelete(props: {
    readonly api: ManagementApi;
    readonly poolId: number;
    readonly read: ScreenRead;
}): JSX.Element {
    const remove = createDeleteAction({
        read: props.read,
        action: "delete-pool",
        perform: () =>
            props.api.deleteProviderPool({
                provider_pool_id: props.poolId,
            }),
        onDeleted: () => navigate(hrefs.providerPools()),
    });
    return (
        <ConfirmDeleteCard
            title={translation()(($) => $.screens.providerPoolEditorScreen.deleteThisPool)}
            description={translation()(
                ($) =>
                    $.screens.providerPoolEditorScreen
                        .deletingRemovesAutomaticPoolEntriesAndRestoresStandaloneMessage,
            )}
            question={translation()(
                ($) =>
                    $.additional.providerPoolEditorScreen
                        .deleteThisPoolItsMembershipIsRemovedMessage,
            )}
            onConfirm={remove}
        />
    );
}

function PoolForm(props: {
    readonly api: ManagementApi;
    readonly seed: PoolSeed;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly read: ScreenRead;
    readonly onDirtyChange: (isDirty: boolean) => void;
    readonly onSaved: () => void;
}): JSX.Element {
    const [baseline, setBaseline] = createSignal(props.seed);
    const [name, setName] = createSignal(props.seed.name);
    const [modelPrefix, setModelPrefix] = createSignal<string | null>(
        props.seed.id === null ? null : props.seed.model_prefix,
    );
    const resolvedPrefix = (): string =>
        modelPrefix() ?? (isRoutingToken(name()) ? `${name()}-` : "");
    const [isEnabled, setIsEnabled] = createSignal(props.seed.is_enabled);
    const [members, setMembers] = createSignal(props.seed.members);
    const [selectedAccount, setSelectedAccount] = createSignal("");
    const desired = (): PoolSeed => ({
        id: baseline().id,
        name: name(),
        model_prefix: resolvedPrefix(),
        is_enabled: isEnabled(),
        members: members(),
    });
    const isDirty = (): boolean => !isSamePool(baseline(), desired());
    const canSave = (): boolean =>
        name() !== "" &&
        (resolvedPrefix() === "" || isRoutingToken(resolvedPrefix())) &&
        (baseline().id === null || isDirty());
    const form = createEditForm({
        read: props.read,
        action: props.seed.id === null ? "create-pool" : "update-pool",
        isDirty,
        canSave,
        perform: async () => {
            const state = desired();
            if (state.id === null) {
                const created = await props.api.createProviderPool({
                    name: state.name,
                    model_prefix: state.model_prefix,
                    is_enabled: state.is_enabled,
                    members: state.members,
                });
                return created.provider_pool_id;
            }
            await props.api.updateProviderPool({
                provider_pool_id: state.id,
                name: state.name,
                model_prefix: state.model_prefix,
                is_enabled: state.is_enabled,
                members: state.members,
            });
            return state.id;
        },
        onSucceeded: (id) => {
            setBaseline({
                ...desired(),
                id,
            });
            if (props.seed.id === null) navigate(hrefs.providerPoolEdit(id));
            return {
                tone: "ok",
                get title() {
                    return translation()(($) => $.notices.poolSaved);
                },
                detail: null,
            };
        },
        onDirtyChange: props.onDirtyChange,
        onSaved: props.onSaved,
    });
    const accountName = (id: number): string =>
        props.providers.find((provider) => provider.id === id)?.name ??
        translation()(($) => $.domainMessages.accountUnlisted, {
            id,
        });
    // Swapping priority values preserves authored gaps and commits no partial write.
    const move = (index: number, offset: number): void => {
        const next = [
            ...members(),
        ];
        const current = next[index];
        const neighbor = next[index + offset];
        if (current === undefined || neighbor === undefined)
            throw new Error("Pool reorder addressed no adjacent member.");
        next[index] = {
            ...neighbor,
            priority: current.priority,
        };
        next[index + offset] = {
            ...current,
            priority: neighbor.priority,
        };
        setMembers(next);
    };
    const nextPriority = (): number =>
        members().reduce((maximum, member) => Math.max(maximum, member.priority), -1) + 1;
    const canAdd = (): boolean =>
        selectedAccount() !== "" &&
        !members().some((member) => member.provider_id === Number(selectedAccount())) &&
        Number.isSafeInteger(nextPriority());
    const add = (): void => {
        if (!canAdd()) return;
        setMembers([
            ...members(),
            {
                provider_id: Number(selectedAccount()),
                priority: nextPriority(),
            },
        ]);
        setSelectedAccount("");
    };
    const revert = (): void => {
        setName(baseline().name);
        setModelPrefix(baseline().model_prefix);
        setIsEnabled(baseline().is_enabled);
        setMembers(baseline().members);
        setSelectedAccount("");
        props.read.reload();
    };
    return (
        <Card>
            <CardHeader
                title={translation()(($) => $.screens.providerPoolEditorScreen.poolSettings)}
            />
            <form
                novalidate
                class="mt-4 flex max-w-2xl flex-col gap-5"
                onSubmit={form.submit}
            >
                <Switch
                    label={translation()(($) => $.screens.providerPoolEditorScreen.enabled)}
                    isOn={isEnabled()}
                    onToggle={setIsEnabled}
                    isDisabled={form.isSaving()}
                />
                <TextField
                    label={translation()(($) => $.screens.providerPoolEditorScreen.name)}
                    value={name()}
                    onInput={setName}
                    error={
                        name() === ""
                            ? translation()(
                                  ($) =>
                                      $.additional.providerPoolEditorScreen.nameIsRequiredMessage,
                              )
                            : null
                    }
                    isDisabled={form.isSaving()}
                />
                <TextField
                    label={translation()(($) => $.screens.providerPoolEditorScreen.modelPrefix)}
                    value={resolvedPrefix()}
                    onInput={setModelPrefix}
                    error={routingTokenError(resolvedPrefix(), "modelPrefix")}
                    isDisabled={form.isSaving()}
                    hint={translation()(
                        ($) =>
                            $.screens.providerPoolEditorScreen
                                .thePoolAutomaticallyExposesMemberModelsAsThisMessage,
                    )}
                />
                <div class="flex flex-col gap-3">
                    <h3 class="text-[13px] font-medium">
                        {translation()(
                            ($) => $.screens.providerPoolEditorScreen.accountsInPriorityOrder,
                        )}
                    </h3>
                    <p class="text-[12.5px] text-ink-faint">
                        {translation()(
                            ($) =>
                                $.screens.providerPoolEditorScreen
                                    .membersMustServeTheSameUpstreamServiceTheirMessage,
                        )}
                    </p>
                    <ol class="flex flex-col gap-3">
                        <For each={members()}>
                            {(member, index) => (
                                <li class="flex flex-col items-start gap-3 border-b border-rule pb-3 sm:flex-row sm:items-center">
                                    <span class="min-w-0 break-words text-[13px] sm:flex-1">
                                        <bdi dir="auto">{accountName(member.provider_id)}</bdi>{" "}
                                        <span class="tnum text-ink-faint">
                                            ·{" "}
                                            {translation()(($) => $.quotaMessages.priority, {
                                                priority: member.priority,
                                            })}
                                        </span>
                                    </span>
                                    <div class="flex shrink-0 flex-wrap gap-2">
                                        <Button
                                            title={translation()(($) => $.domainMessages.moveUp, {
                                                name: accountName(member.provider_id),
                                            })}
                                            isDisabled={form.isSaving() || index() === 0}
                                            onClick={() => move(index(), -1)}
                                        >
                                            {translation()(
                                                ($) => $.screens.providerPoolEditorScreen.moveUp,
                                            )}
                                        </Button>
                                        <Button
                                            title={translation()(($) => $.domainMessages.moveDown, {
                                                name: accountName(member.provider_id),
                                            })}
                                            isDisabled={
                                                form.isSaving() || index() === members().length - 1
                                            }
                                            onClick={() => move(index(), 1)}
                                        >
                                            {translation()(
                                                ($) => $.screens.providerPoolEditorScreen.moveDown,
                                            )}
                                        </Button>
                                        <Button
                                            title={translation()(($) => $.domainMessages.remove, {
                                                name: accountName(member.provider_id),
                                            })}
                                            isDisabled={form.isSaving()}
                                            onClick={() =>
                                                setMembers(
                                                    members().filter(
                                                        (candidate) =>
                                                            candidate.provider_id !==
                                                            member.provider_id,
                                                    ),
                                                )
                                            }
                                        >
                                            {translation()(
                                                ($) => $.screens.providerPoolEditorScreen.remove,
                                            )}
                                        </Button>
                                    </div>
                                </li>
                            )}
                        </For>
                    </ol>
                    <SelectField
                        label={translation()(($) => $.screens.providerPoolEditorScreen.addAccount)}
                        value={selectedAccount()}
                        options={[
                            {
                                value: "",
                                label: translation()(
                                    ($) => $.additional.providerPoolEditorScreen.chooseAnAccount,
                                ),
                            },
                            ...props.providers
                                .filter(
                                    (provider) =>
                                        !members().some(
                                            (member) => member.provider_id === provider.id,
                                        ),
                                )
                                .map((provider) => ({
                                    value: String(provider.id),
                                    label: provider.name,
                                })),
                        ]}
                        onChange={setSelectedAccount}
                        isDisabled={form.isSaving()}
                    />
                    <div>
                        <Button
                            isDisabled={form.isSaving() || !canAdd()}
                            onClick={add}
                        >
                            {translation()(($) => $.screens.providerPoolEditorScreen.addMember)}
                        </Button>
                    </div>
                </div>
                {props.seed.id === null ? (
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!canSave()}
                            isBusy={form.isSaving()}
                        >
                            {translation()(($) => $.screens.providerPoolEditorScreen.createPool)}
                        </Button>
                    </div>
                ) : (
                    <DirtyFormFooter
                        isDirty={isDirty()}
                        isSaving={form.isSaving()}
                        canSave={canSave()}
                        onRevert={revert}
                    />
                )}
            </form>
        </Card>
    );
}
