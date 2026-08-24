import type { JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import type { ParentComponent } from "solid-js";

interface DialogProps {
    title: string;
    description?: string;
    isOpen: boolean;
    isBusy?: boolean;
    onClose: () => void;
}

export const Dialog: ParentComponent<DialogProps> = (props): JSX.Element => (
    <Portal>
        <div
            class={[
                "fixed inset-0 z-100 grid place-items-center overflow-y-auto bg-[#080e0ba6] p-5 font-sans text-[#18211d] backdrop-blur-[5px] [&_button]:focus-visible:outline-3 [&_button]:focus-visible:outline-offset-2 [&_button]:focus-visible:outline-[#29a66f]/25 [&_a]:focus-visible:outline-3 [&_a]:focus-visible:outline-offset-2 [&_a]:focus-visible:outline-[#29a66f]/25 [&_button:disabled]:cursor-not-allowed [&_button:disabled]:opacity-[.58] max-[540px]:place-items-end max-[540px]:p-0",
                {
                    hidden: !props.isOpen,
                },
            ]}
        >
            <section
                class="max-h-[calc(100vh-40px)] w-full max-w-[590px] overflow-y-auto rounded-[18px] border border-white/40 bg-white shadow-[0_30px_90px_rgb(0_0_0/30%)] max-[540px]:max-h-[95vh] max-[540px]:max-w-none max-[540px]:rounded-b-none"
                role="dialog"
                aria-modal="true"
                aria-label={props.title}
            >
                <header class="flex items-start justify-between gap-5 border-b border-[#dbe2dc] px-6 pt-[22px] pb-[18px]">
                    <div>
                        <h2 class="mb-[3px] text-xl font-[760] tracking-[-0.02em] text-[#18211d]">
                            {props.title}
                        </h2>
                        {props.description === undefined ? null : (
                            <p class="mt-1.5 text-xs text-[#66726b]">{props.description}</p>
                        )}
                    </div>
                    <button
                        class="grid size-[31px] shrink-0 place-items-center rounded-lg border-0 bg-[#f0f3f1] p-0 text-xl text-[#637068] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 disabled:cursor-not-allowed disabled:opacity-60"
                        type="button"
                        aria-label="Close dialog"
                        disabled={props.isBusy}
                        onClick={props.onClose}
                    >
                        ×
                    </button>
                </header>
                {props.children}
            </section>
        </div>
    </Portal>
);

interface NoticeProps {
    kind: "error" | "success" | "info";
    children: JSX.Element;
}

const noticeKindClasses: Record<NoticeProps["kind"], string> = {
    error: "border-[#f1c5c2] bg-[#fbe8e7] text-[#8d2f2f]",
    success: "border-[#bce7ce] bg-[#dff5e9] text-[#176943]",
    info: "border-[#cfe3d7] bg-[#ebf5ef] text-[#335d49]",
};

export function Notice(props: NoticeProps): JSX.Element {
    return (
        <div
            class={`mb-[18px] rounded-[10px] border px-3.5 py-[11px] text-[13px] leading-[1.45] ${noticeKindClasses[props.kind]}`}
            role={props.kind === "error" ? "alert" : "status"}
        >
            {props.children}
        </div>
    );
}

export function LoadingBlock(): JSX.Element {
    return (
        <div
            class="flex min-h-45 items-center justify-center gap-2.5 text-[13px] text-[#66726b]"
            role="status"
        >
            <span class="size-[18px] animate-spin rounded-full border-2 border-[#c9d8cf] border-t-[#29a66f]" />
            Loading…
        </div>
    );
}
