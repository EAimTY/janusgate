import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, Show } from "solid-js";
import { toUpdateApiKeyRequest } from "/src/management-api/adapters";
import type { ManagementApi } from "/src/management-api/client";
import type { ApiKeyResponse } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { ApiKeyNameField, ApiKeySecretField } from "/src/ui/api-key-form";
import { Button, ButtonLink, Switch } from "/src/ui/controls";
import { EmptyState, Notice, type NoticeTone, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

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
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);

    let controller: AbortController | null = null;

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const outcome = await performManagementCall(() => props.api.listApiKeys(call.signal));

        if (outcome.status === "aborted") {
            return;
        }

        setIsReading(false);

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause);

            setNotice({
                tone: "bad",
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return;
        }

        setNotice(null);
        setKeys(outcome.result.api_keys);
        setHasRead(true);
        reportRefresh();
    };

    createTrackedEffect(() => {
        props.apiKeyId;
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const apiKey = createMemo(
        () => keys().find((candidate) => candidate.id === props.apiKeyId) ?? null,
    );

    const reportFailure = (cause: unknown, action: "update-api-key" | "delete-api-key"): void => {
        const diagnosis = describeCallFailure(cause, action);

        setNotice({
            tone: "bad",
            title: diagnosis.headline,
            detail: diagnosis.detail,
            fix: diagnosis.fix,
        });
    };

    return (
        <div class="rise-in flex flex-col gap-5">
            <Show when={isReading() && !hasRead() && notice() === null}>
                <PageHeader
                    title="Edit API key"
                    backHref={hrefs.apiKeyDetail(props.apiKeyId)}
                    backLabel="API key"
                />
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead() && apiKey() === null}>
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

                        <Show when={notice()}>
                            {(current) => (
                                <Notice
                                    tone={current().tone}
                                    title={current().title}
                                    detail={current().detail}
                                    fix={current().fix}
                                />
                            )}
                        </Show>

                        <SettingsCard
                            api={props.api}
                            apiKey={row}
                            enabledCount={keys().filter((candidate) => candidate.is_enabled).length}
                            onReload={() => void read()}
                            onNotice={setNotice}
                            onFailure={reportFailure}
                        />

                        <DangerCard
                            api={props.api}
                            apiKey={row}
                            isLastEnabled={
                                row.is_enabled &&
                                keys().filter((candidate) => candidate.is_enabled).length <= 1
                            }
                            onFailure={reportFailure}
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
    readonly onNotice: (notice: ScreenNotice) => void;
    readonly onFailure: (cause: unknown, action: "update-api-key") => void;
}): JSX.Element {
    const [isEnabled, setIsEnabled] = createSignal(props.apiKey.is_enabled);
    const [name, setName] = createSignal(props.apiKey.name);
    const [secret, setSecret] = createSignal(props.apiKey.key);
    const [isSaving, setIsSaving] = createSignal(false);

    const isDirty = (): boolean =>
        isEnabled() !== props.apiKey.is_enabled ||
        name() !== props.apiKey.name ||
        secret() !== props.apiKey.key;

    const revert = (): void => {
        setIsEnabled(props.apiKey.is_enabled);
        setName(props.apiKey.name);
        setSecret(props.apiKey.key);
    };

    const save = async (): Promise<void> => {
        if (isSaving()) {
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
            props.onFailure(outcome.cause, "update-api-key");
            return;
        }

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
                />
                <ApiKeySecretField
                    secret={secret()}
                    onSecretInput={setSecret}
                    isDisabled={isSaving()}
                    hint="Replacing the secret revokes the old value on save."
                />
                <div class="flex flex-wrap items-center gap-2">
                    <Button
                        type="submit"
                        tone="primary"
                        isDisabled={!isDirty()}
                        isBusy={isSaving()}
                    >
                        {isSaving() ? "Saving…" : "Save changes"}
                    </Button>
                    <Button
                        isDisabled={!isDirty() || isSaving()}
                        onClick={revert}
                    >
                        Revert
                    </Button>
                    <Show when={isDirty() && !isSaving()}>
                        <span
                            class="text-[13px] font-medium text-warn"
                            role="status"
                        >
                            Unsaved changes
                        </span>
                    </Show>
                </div>
            </form>
        </Card>
    );
}

/** Revocation, confirmed where it states its blast radius. */
function DangerCard(props: {
    readonly api: ManagementApi;
    readonly apiKey: ApiKeyResponse;
    readonly isLastEnabled: boolean;
    readonly onFailure: (cause: unknown, action: "delete-api-key") => void;
}): JSX.Element {
    const [isArmed, setIsArmed] = createSignal(false);
    const [isDeleting, setIsDeleting] = createSignal(false);

    const remove = async (): Promise<void> => {
        if (isDeleting()) {
            return;
        }

        setIsDeleting(true);

        const outcome = await performManagementCall(() =>
            props.api.deleteApiKey({
                api_key_id: props.apiKey.id,
            }),
        );

        setIsDeleting(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            props.onFailure(outcome.cause, "delete-api-key");
            setIsArmed(false);
            return;
        }

        navigate(hrefs.apiKeys());
    };

    return (
        <Card class="border-bad/30">
            <CardHeader
                title="Delete this key"
                description="Deleting revokes the key at once. Clients holding it start getting 401s. Nothing else is removed."
            />
            <Show
                when={isArmed()}
                fallback={
                    <div class="mt-4">
                        <Button
                            tone="danger"
                            glyph="trash"
                            onClick={() => setIsArmed(true)}
                        >
                            Delete…
                        </Button>
                    </div>
                }
            >
                <div class="mt-4 flex flex-wrap items-center gap-2">
                    <p class="w-full text-[13.5px] text-ink-dim">
                        {props.isLastEnabled
                            ? `Delete ${props.apiKey.name}? It is the last enabled key — the gateway will serve nothing until another key is turned on.`
                            : `Delete ${props.apiKey.name}? Clients holding this secret lose access.`}
                    </p>
                    <Button
                        tone="danger"
                        isBusy={isDeleting()}
                        onClick={() => void remove()}
                    >
                        {isDeleting() ? "Deleting…" : "Delete for good"}
                    </Button>
                    <Button
                        isDisabled={isDeleting()}
                        onClick={() => setIsArmed(false)}
                    >
                        Keep it
                    </Button>
                </div>
            </Show>
        </Card>
    );
}
