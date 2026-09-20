import type { JSX } from "@solidjs/web";
import { createSignal, Show } from "solid-js";
import type { ManagementApi } from "/src/management-api/client";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs, navigate } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import { ApiKeyNameField, ApiKeySecretField } from "/src/ui/api-key-form";
import { Button, Switch } from "/src/ui/controls";
import { Notice } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

/**
 * One creation page for a serving key: a human label, the secret (typed or
 * generated), and the serving switch. Creating lands on the new key's page,
 * like a provider creation does.
 */
export function ApiKeyNewScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [name, setName] = createSignal("");
    const [secret, setSecret] = createSignal("");
    const [isEnabled, setIsEnabled] = createSignal(true);
    const [isCreating, setIsCreating] = createSignal(false);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    const create = async (): Promise<void> => {
        if (isCreating()) {
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
            const diagnosis = describeCallFailure(outcome.cause, "create-api-key");

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
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
                title="New API key"
                description="One secret your serving clients share. Copy it into client configuration after creating."
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
                    description="The label is yours; the secret must be non-empty and held by no other key."
                />
                <form
                    novalidate={true}
                    class="mt-4 flex max-w-2xl flex-col gap-4"
                    onSubmit={submit}
                >
                    <ApiKeyNameField
                        name={name()}
                        onNameInput={setName}
                        isDisabled={isCreating()}
                    />
                    <ApiKeySecretField
                        secret={secret()}
                        onSecretInput={setSecret}
                        isDisabled={isCreating()}
                    />
                    <Switch
                        label="Enabled"
                        hint="A disabled key is stored but authorizes nothing until it is turned on."
                        isOn={isEnabled()}
                        onText="On"
                        offText="Off"
                        isDisabled={isCreating()}
                        onToggle={setIsEnabled}
                    />
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isCreating()}
                        >
                            {isCreating() ? "Creating…" : "Create key"}
                        </Button>
                    </div>
                </form>
            </Card>
        </div>
    );
}
