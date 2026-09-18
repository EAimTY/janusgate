import type { JSX } from "@solidjs/web";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { EmptyState } from "/src/ui/feedback";

/** An address this app has no page for, with the ways back. */
export function NotFoundScreen(): JSX.Element {
    return (
        <EmptyState
            title="Page not found"
            description={`Nothing lives at ${window.location.pathname}. It may have been deleted, or the address was typed wrong.`}
        >
            <ButtonLink
                href={hrefs.home()}
                tone="primary"
            >
                Go to Overview
            </ButtonLink>
            <ButtonLink href={hrefs.providers()}>Providers</ButtonLink>
            <ButtonLink href={hrefs.models(null)}>Models</ButtonLink>
        </EmptyState>
    );
}
