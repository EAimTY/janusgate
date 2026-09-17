import type { JSX } from "@solidjs/web";

export type BadgeTone = "neutral" | "positive" | "negative";

export type BadgeProps = {
    readonly tone: BadgeTone;
    readonly children: JSX.Element;
};

export function Badge(props: BadgeProps): JSX.Element {
    return (
        <span
            class={[
                "inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium whitespace-nowrap",
                {
                    "border-slate-200 bg-slate-100 text-slate-700": props.tone === "neutral",
                    "border-emerald-200 bg-emerald-50 text-emerald-700": props.tone === "positive",
                    "border-amber-200 bg-amber-50 text-amber-700": props.tone === "negative",
                },
            ]}
        >
            {props.children}
        </span>
    );
}
