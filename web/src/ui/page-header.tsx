import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

export type PageHeaderProps = {
    readonly title: string;
    readonly description: string;
    readonly actions?: JSX.Element;
};

export function PageHeader(props: PageHeaderProps): JSX.Element {
    return (
        <div class="flex flex-wrap items-start justify-between gap-4">
            <div>
                <h1 class="text-2xl font-bold tracking-tight text-slate-900">{props.title}</h1>
                <p class="mt-1 max-w-2xl text-sm text-slate-600">{props.description}</p>
            </div>
            <Show when={props.actions !== undefined}>
                <div class="flex flex-wrap items-center gap-2">{props.actions}</div>
            </Show>
        </div>
    );
}
