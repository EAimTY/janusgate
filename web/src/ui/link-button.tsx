import type { JSX } from "@solidjs/web";
import { AppLink } from "/src/ui/app-link";
import { type ButtonVariant, buttonClass } from "/src/ui/button";

export type LinkButtonProps = {
    readonly href: string;
    readonly children: JSX.Element;
    readonly variant?: ButtonVariant;
};

/** An in-app link rendered with the button styling. */
export function LinkButton(props: LinkButtonProps): JSX.Element {
    return (
        <AppLink
            href={props.href}
            class={buttonClass(props.variant ?? "primary")}
        >
            {props.children}
        </AppLink>
    );
}
