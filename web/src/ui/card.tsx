import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

export type CardProps = {
    readonly title: string;
    readonly description?: string | null;
    readonly actions?: JSX.Element;
    readonly children: JSX.Element;
};

/** A titled panel; the application's one container for grouped content. */
export function Card(props: CardProps): JSX.Element {
    return (
        <section class="rounded-lg border border-slate-200 bg-white shadow-sm">
            <header class="flex flex-wrap items-start justify-between gap-3 border-b border-slate-200 px-5 py-4">
                <div>
                    <h2 class="text-base font-semibold text-slate-900">{props.title}</h2>
                    <Show when={props.description ?? null}>
                        {(description) => (
                            <p class="mt-0.5 text-sm text-slate-500">{description()}</p>
                        )}
                    </Show>
                </div>
                <Show when={props.actions !== undefined}>
                    <div class="flex flex-wrap items-center gap-2">{props.actions}</div>
                </Show>
            </header>
            <div class="px-5 py-4">{props.children}</div>
        </section>
    );
}
