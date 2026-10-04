import type { JSX } from "@solidjs/web";
import { translation } from "/src/i18n";
import { missingPathMessage } from "/src/i18n/rich";
import { hrefs } from "/src/router";
import { ButtonLink } from "/src/ui/controls";
import { EmptyState } from "/src/ui/feedback";

/** An address this app has no page for, with the ways back. */
export function NotFoundScreen(): JSX.Element {
    return (
        <EmptyState
            title={translation()(($) => $.screens.notFoundScreen.pageNotFound)}
            description={missingPathMessage(window.location.pathname)}
        >
            <ButtonLink
                href={hrefs.home()}
                tone="primary"
            >
                {translation()(($) => $.screens.notFoundScreen.goToOverview)}
            </ButtonLink>
            <ButtonLink href={hrefs.providers()}>
                {translation()(($) => $.screens.notFoundScreen.providers)}
            </ButtonLink>
            <ButtonLink href={hrefs.models(null)}>
                {translation()(($) => $.screens.notFoundScreen.models)}
            </ButtonLink>
        </EmptyState>
    );
}
