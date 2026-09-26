import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { EmptyState } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";

/** The rows whose detail and edit screens share this not-found state. */
type RowNotFoundKind = "api_key" | "model" | "provider";

/** The door to creating a replacement, offered only where one is a way forward. */
type CreateAction = {
    readonly href: string;
    readonly label: string;
};

/** The state's copy per row kind: the names it composes, the listing, and what the gateway's silence means. */
const KIND_COPY: Record<
    RowNotFoundKind,
    {
        readonly rowName: string;
        readonly listLabel: string;
        readonly listHref: string;
        readonly backActionLabel: string;
        readonly description: string;
    }
> = {
    api_key: {
        rowName: "API key",
        listLabel: "API keys",
        listHref: hrefs.apiKeys(),
        backActionLabel: "Back to API keys",
        description:
            "The gateway holds no API key here. It was deleted, or the address was typed wrong. Nothing here can bring it back.",
    },
    model: {
        rowName: "Model",
        listLabel: "Models",
        listHref: hrefs.models(null),
        backActionLabel: "Back to models",
        description:
            "The gateway holds no model here. It was deleted, or the address was typed wrong — a deleted discovered model returns as new on the next discovery.",
    },
    provider: {
        rowName: "Provider",
        listLabel: "Providers",
        listHref: hrefs.providers(),
        backActionLabel: "Back to providers",
        description:
            "The gateway holds no provider here. It was deleted, or the address was typed wrong. Nothing here can bring it back.",
    },
};

/**
 * The row screens' shared not-found state (README §6.3, §6.4): the screen's
 * own header kept over the state announcing that the addressed row does not
 * exist, with the way back to the listing — in the header and as the primary
 * action — and, where creating a replacement is a way forward, the creation
 * action. The copy is the row kind's, stated here once; the screens differ
 * only in their header title and in whether they offer the creation action.
 */
export function RowNotFound(props: {
    /** Which row kind's copy to state. */
    readonly kind: RowNotFoundKind;
    /** The screen's own header title in this state ("API key", "Edit provider"). */
    readonly headerTitle: string;
    /** The creation action, where creating a replacement is a way forward. */
    readonly createAction?: CreateAction | undefined;
}): JSX.Element {
    const copy = KIND_COPY[props.kind];

    return (
        <>
            <PageHeader
                title={props.headerTitle}
                backHref={copy.listHref}
                backLabel={copy.listLabel}
            />
            <EmptyState
                title={`${copy.rowName} not found`}
                description={copy.description}
            >
                <ButtonLink
                    href={copy.listHref}
                    tone="primary"
                >
                    {copy.backActionLabel}
                </ButtonLink>
                <Show when={props.createAction}>
                    {(createAction) => (
                        <ButtonLink href={createAction().href}>{createAction().label}</ButtonLink>
                    )}
                </Show>
            </EmptyState>
        </>
    );
}
