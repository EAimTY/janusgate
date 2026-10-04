import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { translation } from "/src/i18n";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { EmptyState } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";

/** The rows whose detail and edit screens share this not-found state. */
type RowNotFoundKind = "api_key" | "model" | "provider" | "pool" | "model_route";

/** The door to creating a replacement, offered only where one is a way forward. */
type CreateAction = {
    readonly href: string;
    readonly label: string;
};

/** Localized listing and recovery copy for each row kind. */
const KIND_COPY: Record<
    RowNotFoundKind,
    {
        readonly listLabel: string;
        readonly listHref: string;
        readonly backActionLabel: string;
        readonly description: string;
    }
> = {
    pool: {
        get listLabel() {
            return translation()(($) => $.navigation.pools);
        },
        listHref: hrefs.providerPools(),
        get backActionLabel() {
            return translation()(($) => $.remainingText.backToPools);
        },
        get description() {
            return translation()(
                ($) =>
                    $.helpers.rowNotFound.theGatewayHoldsNoPoolHereItWasDeletedOrTheAddressWasTyped,
            );
        },
    },
    model_route: {
        get listLabel() {
            return translation()(($) => $.remainingText.publicRoutes);
        },
        listHref: hrefs.modelRoutes(),
        get backActionLabel() {
            return translation()(($) => $.remainingText.backToPublicRoutes);
        },
        get description() {
            return translation()(
                ($) =>
                    $.helpers.rowNotFound
                        .theGatewayHoldsNoPublicRouteHereItWasDeletedOrTheAddressWas,
            );
        },
    },
    api_key: {
        get listLabel() {
            return translation()(($) => $.navigation.apiKeys);
        },
        listHref: hrefs.apiKeys(),
        get backActionLabel() {
            return translation()(($) => $.remainingText.backToAPIKeys);
        },
        get description() {
            return translation()(
                ($) => $.helpers.rowNotFound.theGatewayHoldsNoAPIKeyHereItWasDeletedOrTheAddressWas,
            );
        },
    },
    model: {
        get listLabel() {
            return translation()(($) => $.navigation.inventory);
        },
        listHref: hrefs.models(null),
        get backActionLabel() {
            return translation()(($) => $.remainingText.backToModels);
        },
        get description() {
            return translation()(
                ($) =>
                    $.helpers.rowNotFound
                        .theGatewayHoldsNoModelHereItWasDeletedOrTheAddressWasTyped,
            );
        },
    },
    provider: {
        get listLabel() {
            return translation()(($) => $.navigation.providers);
        },
        listHref: hrefs.providers(),
        get backActionLabel() {
            return translation()(($) => $.remainingText.backToProviders);
        },
        get description() {
            return translation()(
                ($) =>
                    $.helpers.rowNotFound
                        .theGatewayHoldsNoProviderHereItWasDeletedOrTheAddressWasTyped,
            );
        },
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
                title={translation()(($) => $.domainMessages.notFound[props.kind])}
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
