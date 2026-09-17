import type { JSX } from "@solidjs/web";

export type ButtonVariant = "primary" | "secondary" | "danger";

const baseClass =
    "inline-flex items-center justify-center gap-1 rounded-md px-3 py-2 text-sm font-medium whitespace-nowrap transition-colors disabled:cursor-not-allowed disabled:opacity-50";

/**
 * The application's button styling, shared by `<Button>` and links that look
 * like buttons so a control renders identically whichever element wraps it.
 */
export function buttonClass(variant: ButtonVariant): JSX.ClassValue {
    return [
        baseClass,
        {
            "bg-slate-900 text-white hover:bg-slate-700": variant === "primary",
            "border border-slate-300 bg-white text-slate-700 hover:bg-slate-100":
                variant === "secondary",
            "bg-red-600 text-white hover:bg-red-700": variant === "danger",
        },
    ];
}

export type ButtonProps = {
    readonly children: JSX.Element;
    readonly variant?: ButtonVariant;
    readonly type?: "button" | "submit";
    readonly disabled?: boolean;
    readonly onClick?: () => void;
    readonly ref?: (element: HTMLButtonElement) => void;
};

export function Button(props: ButtonProps): JSX.Element {
    return (
        <button
            type={props.type ?? "button"}
            disabled={props.disabled ?? false}
            ref={props.ref}
            onClick={() => props.onClick?.()}
            class={buttonClass(props.variant ?? "primary")}
        >
            {props.children}
        </button>
    );
}
