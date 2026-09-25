import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import { bearerSecretError, isBearerSecret } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { ApiKeyNameField, ApiKeySecretField, apiKeyNameError } from "/src/ui/api-key-form";
import { Button, Switch } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";
import { createCreateAction } from "./create-form";
import type { ScreenNotice } from "./read";

/**
 * One creation page for a serving key: whether it works from the start, a
 * human label, and the secret (typed or generated). Creating lands on the
 * new key's page, like a provider creation does.
 */
export function ApiKeyNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [name, setName] = createSignal("");
    // The name's error is stated only once the operator has typed into the
    // field — a pristine form is not an error, an emptied name is.
    const [hasNameInput, setHasNameInput] = createSignal(false);
    const [secret, setSecret] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    /** Inline error for the name, shown where it was typed. */
    const nameError = (): string | null => (hasNameInput() ? apiKeyNameError(name()) : null);

    /** Inline composition error for the secret, shown where it was typed. */
    const secretError = (): string | null => bearerSecretError(secret(), "Key");

    /** The creation's composition rules: a named key carrying a complete secret. */
    const canCompose = (): boolean => name() !== "" && isBearerSecret(secret());

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
                }),
            ),
        }),
        onCreated: (result) => {
            navigate(hrefs.apiKeyDetail(result.api_key.id));
        },
    });

    const canCreate = (): boolean => canCompose() && !create.isCreating();

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Create API key"
                description="One secret your serving clients share."
                backHref={hrefs.apiKeys()}
                backLabel="API keys"
            />

            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Card>
                <CardHeader
                    title="Key"
                    description="The secret holds letters, numbers, and symbols only — no spaces — and is held by no other key."
                />
                <form
                    novalidate={true}
                    class="mt-4 flex max-w-2xl flex-col gap-4"
                    onSubmit={create.submit}
                >
                    <Switch
                        label="Enabled"
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
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!canCreate()}
                            isBusy={create.isCreating()}
                        >
                            {create.isCreating() ? "Creating…" : "Create API key"}
                        </Button>
                    </div>
                </form>
            </Card>
        </div>
    );
}
