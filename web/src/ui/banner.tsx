import type { JSX } from "@solidjs/web";

export type BannerVariant = "error" | "info" | "success";

export type BannerProps = {
    readonly variant: BannerVariant;
    readonly children: JSX.Element;
};

export function Banner(props: BannerProps): JSX.Element {
    return (
        <div
            role={props.variant === "error" ? "alert" : "status"}
            class={[
                "rounded-md border px-4 py-3 text-sm",
                {
                    "border-red-200 bg-red-50 text-red-800": props.variant === "error",
                    "border-slate-200 bg-slate-50 text-slate-700": props.variant === "info",
                    "border-emerald-200 bg-emerald-50 text-emerald-800":
                        props.variant === "success",
                },
            ]}
        >
            {props.children}
        </div>
    );
}
