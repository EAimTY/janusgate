import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import { compositionMessage, isBearerSecret } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { type ScreenNotice, screenFailureNotice } from "/src/screens/read";
import { ApiKeyNameField, ApiKeySecretField, apiKeyNameError } from "/src/ui/api-key-form";
import { Button, Switch } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

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
    const [isCreating, setIsCreating] = createSignal(false);
    const [failure, setFailure] = createSignal<ScreenNotice | null>(null);

    /** Inline error for the name, shown where it was typed. */
    const nameError = (): string | null => (hasNameInput() ? apiKeyNameError(name()) : null);

    /** Inline composition error for the secret, shown where it was typed. */
    const secretError = (): string | null =>
        secret() === "" || isBearerSecret(secret()) ? null : compositionMessage("Key");

    const canCreate = (): boolean => name() !== "" && isBearerSecret(secret()) && !isCreating();

    const create = async (): Promise<void> => {
        if (isCreating() || name() === "" || !isBearerSecret(secret())) {
            return;
        }

        setFailure(null);
        setIsCreating(true);

        const outcome = await performManagementCall(() =>
            props.api.createApiKey({
                name: name(),
                key: secret(),
                is_enabled: isEnabled(),
            }),
        );

        setIsCreating(false);

        if (outcome.status === "aborted") {
            return;
        }

        if (outcome.status === "failed") {
            setFailure(
                screenFailureNotice(outcome.cause, {
                    action: "create-api-key",
                }),
            );
            return;
        }

        navigate(hrefs.apiKeyDetail(outcome.result.api_key.id));
    };

    const submit = (event: SubmitEvent): void => {
        event.preventDefault();
        void create();
    };

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
                    onSubmit={submit}
                >
                    <Switch
                        label="Enabled"
                        isOn={isEnabled()}
                        onText="On"
                        offText="Off"
                        isDisabled={isCreating()}
                        onToggle={setIsEnabled}
                    />
                    <ApiKeyNameField
                        name={name()}
                        onNameInput={(value) => {
                            setHasNameInput(true);
                            setName(value);
                        }}
                        isDisabled={isCreating()}
                        error={nameError()}
                    />
                    <ApiKeySecretField
                        secret={secret()}
                        onSecretInput={setSecret}
                        isDisabled={isCreating()}
                        error={secretError()}
                    />
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isDisabled={!canCreate()}
                            isBusy={isCreating()}
                        >
                            {isCreating() ? "Creating…" : "Create API key"}
                        </Button>
                    </div>
                </form>
            </Card>
        </div>
    );
}
