import type { JSX } from "@solidjs/web";
import { navigate } from "/src/router";

export type AppLinkProps = {
    readonly href: string;
    readonly children: JSX.Element;
    readonly class?: JSX.ClassValue | undefined;
    /** Marks the link as the current page for assistive technology. */
    readonly isCurrent?: boolean | undefined;
};

/**
 * An in-app link. It renders a real anchor — so a deep link can be opened in a
 * new tab and its target is visible on hover — and intercepts only the plain
 * left click that the SPA can handle itself.
 */
export function AppLink(props: AppLinkProps): JSX.Element {
    return (
        <a
            href={props.href}
            class={props.class}
            aria-current={(props.isCurrent ?? false) ? "page" : undefined}
            onClick={(event) => {
                if (
                    event.defaultPrevented ||
                    event.button !== 0 ||
                    event.metaKey ||
                    event.ctrlKey ||
                    event.shiftKey ||
                    event.altKey
                ) {
                    return;
                }

                event.preventDefault();
                navigate(props.href);
            }}
        >
            {props.children}
        </a>
    );
}
