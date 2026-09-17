import type { JSX } from "@solidjs/web";
import { hrefs } from "/src/router";
import { Card } from "/src/ui/card";
import { LinkButton } from "/src/ui/link-button";
import { PageHeader } from "/src/ui/page-header";

/** Anything the router does not recognize (README §5). */
export function NotFoundScreen(): JSX.Element {
    return (
        <div class="flex flex-col gap-6">
            <PageHeader
                title="Page not found"
                description="This app has no route for the address that was opened."
            />
            <Card title="Nothing here">
                <LinkButton
                    href={hrefs.providers()}
                    variant="secondary"
                >
                    Back to providers
                </LinkButton>
            </Card>
        </div>
    );
}
