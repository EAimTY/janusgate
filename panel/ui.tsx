import type { JSX } from "@solidjs/web";
import { Portal } from "@solidjs/web";
import type { ParentComponent } from "solid-js";

interface DialogProps {
    title: string;
    description?: string;
    isBusy?: boolean;
    onClose: () => void;
}

/** Modal dialog shell. Callers govern mounting (e.g. via `<Show>`); there is no open/closed prop. */
export const Dialog: ParentComponent<DialogProps> = (props): JSX.Element => (
    <Portal>
        <div class="fixed inset-0 z-100 grid place-items-center overflow-y-auto bg-[#080e0ba6] p-5 font-sans text-[#18211d] backdrop-blur-[5px] [&_button]:focus-visible:outline-3 [&_button]:focus-visible:outline-offset-2 [&_button]:focus-visible:outline-[#29a66f]/25 [&_a]:focus-visible:outline-3 [&_a]:focus-visible:outline-offset-2 [&_a]:focus-visible:outline-[#29a66f]/25 [&_button:disabled]:cursor-not-allowed [&_button:disabled]:opacity-[.58] max-[540px]:place-items-end max-[540px]:p-0">
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

// --- Shared class vocabulary ---------------------------------------------------
// Every panel surface reuses these strings verbatim so controls stay visually
// aligned across pages; Tailwind picks the utilities up from this module.

const buttonBaseClasses =
    "inline-flex min-h-10 items-center justify-center rounded-[10px] border px-[15px] py-[9px] text-[13px] leading-[1.2] font-[720] transition-[background,border-color,transform] duration-[140ms] ease-[ease] enabled:active:translate-y-px focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 disabled:cursor-not-allowed disabled:opacity-[.58]";

export const buttonPrimaryClasses = `${buttonBaseClasses} border-[#19784e] bg-[#19784e] text-white shadow-[0_8px_20px_rgb(25_120_78_/_17%)] enabled:hover:bg-[#126a42]`;
export const buttonSecondaryClasses = `${buttonBaseClasses} border-[#cbd8d0] bg-white text-[#28533f] enabled:hover:border-[#b8cbbf] enabled:hover:bg-[#edf4ef]`;
export const buttonGhostClasses = `${buttonBaseClasses} border-[#dbe2dc] bg-transparent text-[#4f5d55] enabled:hover:border-[#b8cbbf] enabled:hover:bg-[#edf4ef]`;

const tableActionBaseClasses =
    "border-0 bg-transparent px-0 py-0.5 text-[11px] font-bold enabled:hover:underline enabled:hover:underline-offset-[3px] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 disabled:cursor-not-allowed disabled:opacity-[.58]";
export const tableActionClasses = `${tableActionBaseClasses} text-[#277351] enabled:hover:text-[#124e32]`;
export const tableActionDangerClasses = `${tableActionBaseClasses} text-[#b53b3b] enabled:hover:text-[#8d2f2f]`;

export const formControlClasses =
    "w-full rounded-lg border border-[#ccd6cf] bg-[#fbfcfb] px-[11px] py-2.5 text-[13px] leading-[1.35] text-[#233029] transition-[border-color,box-shadow,background] duration-150 enabled:hover:border-[#aebdb4] focus:border-[#29a66f] focus:bg-white focus:outline-0 focus:ring-3 focus:ring-[#29a66f]/10 disabled:cursor-not-allowed disabled:opacity-[.58]";

export const checkboxClasses =
    "size-[15px] shrink-0 p-0 accent-[#19784e] focus-visible:outline-3 focus-visible:outline-offset-2 focus-visible:outline-[#29a66f]/25 disabled:cursor-not-allowed disabled:opacity-[.58]";

// Form scaffolding: a field stacks its label text, control, and hint; a checkbox field lays the
// control out inline with its text. Footers right-align the actions, and the dialog variants add
// the narrow-viewport stacking.
export const formFieldClasses = "grid gap-[7px] text-xs font-[720] text-[#344038]";
export const formCheckboxFieldClasses =
    "flex items-center gap-[7px] text-xs font-[720] text-[#344038]";
export const formHintClasses = "text-[10px] leading-[1.4] font-[450] text-[#7c8880]";
export const formFooterClasses = "flex items-center justify-end gap-[9px] pt-[3px]";
export const dialogFooterClasses = `${formFooterClasses} max-[540px]:flex-col-reverse max-[540px]:items-stretch max-[540px]:[&>button]:w-full`;

// The `<form>` element inside a `Dialog`: zeroes the Notice margin and styles bare `<small>` hints.
export const dialogFormClasses =
    "grid gap-[17px] px-6 pt-[22px] pb-6 [&>[role]]:mb-0 [&_label>small]:text-[10px] [&_label>small]:leading-[1.4] [&_label>small]:font-[450] [&_label>small]:text-[#7c8880]";

export const dataTableClasses =
    "w-full border-collapse text-[13px] [&_th]:border-b [&_th]:border-[#e9edea] [&_th]:bg-[#fbfcfb] [&_th]:px-4 [&_th]:py-[11px] [&_th]:text-[10px] [&_th]:font-extrabold [&_th]:tracking-[0.1em] [&_th]:whitespace-nowrap [&_th]:text-[#78847c] [&_th]:uppercase [&_th:not(:last-child)]:text-left [&_th:last-child]:text-right [&_td]:max-w-[310px] [&_td]:border-b [&_td]:border-[#edf0ee] [&_td]:px-4 [&_td]:py-[15px] [&_td]:align-middle [&_td]:text-[#3c4841] [&_tbody_tr:last-child_td]:border-b-0 [&_tbody_tr:hover_td]:bg-[#fcfdfc]";

// --- Shared structural components ----------------------------------------------

export function StatusBadge(props: { isEnabled: boolean }): JSX.Element {
    return (
        <span
            class={[
                "inline-flex items-center gap-[5px] rounded-full bg-[#edf0ee] px-2 py-1 text-[10px] font-[750] text-[#68736c] before:h-[5px] before:w-[5px] before:rounded-full before:bg-[#909a94] before:content-['']",
                {
                    "bg-[#dff5e9] text-[#176943] before:bg-[#29a66f]": props.isEnabled,
                },
            ]}
        >
            {props.isEnabled ? "Enabled" : "Disabled"}
        </span>
    );
}

function CountBadge(props: { count: number }): JSX.Element {
    return (
        <span class="grid h-[29px] min-w-[29px] place-items-center rounded-lg bg-[#eaf0ec] px-2 text-[12px] font-[750] text-[#456052]">
            {props.count}
        </span>
    );
}

interface PanelCardProps {
    title: string;
    description: string;
    count: number;
}

/** White table/section card with the standard gradient header and count chip. */
export const PanelCard: ParentComponent<PanelCardProps> = (props): JSX.Element => (
    <section class="mb-[22px] overflow-hidden rounded-2xl border border-[#dbe2dc] bg-white shadow-[0_9px_28px_rgb(19_37_27_/_4%)]">
        <header class="flex items-center justify-between gap-5 border-b border-[#e6ebe7] bg-[linear-gradient(90deg,#fafcf9,#fff)] px-[22px] py-[19px]">
            <div>
                <h2 class="mb-[3px] text-[16px] font-[760] tracking-[-0.02em] text-[#18211d]">
                    {props.title}
                </h2>
                <p class="m-0 text-[12px] text-[#66726b]">{props.description}</p>
            </div>
            <CountBadge count={props.count} />
        </header>
        {props.children}
    </section>
);

interface PageHeaderProps {
    eyebrow: string;
    title: string;
    description: string;
    actions?: JSX.Element;
}

export function PageHeader(props: PageHeaderProps): JSX.Element {
    return (
        <header class="mb-[30px] flex items-end justify-between gap-7 max-[820px]:flex-col max-[820px]:items-start">
            <div>
                <p class="m-0 text-[11px] font-extrabold tracking-[0.16em] text-[#19784e] uppercase">
                    {props.eyebrow}
                </p>
                <h1 class="mt-[3px] mb-2 text-[clamp(32px,4vw,46px)] leading-none font-[760] tracking-[-0.045em] text-[#18211d] max-[540px]:text-[34px]">
                    {props.title}
                </h1>
                <p class="m-0 max-w-[670px] text-sm text-[#66726b]">{props.description}</p>
            </div>
            {props.actions}
        </header>
    );
}

export const pageShellClasses =
    "mx-auto min-h-screen w-full max-w-[1500px] px-[clamp(24px,4vw,64px)] pt-[52px] pb-20 [&_button]:focus-visible:outline-3 [&_button]:focus-visible:outline-offset-2 [&_button]:focus-visible:outline-[#29a66f]/25 [&_button:disabled]:cursor-not-allowed [&_button:disabled]:opacity-[.58] max-[820px]:min-h-[calc(100vh-66px)] max-[820px]:px-[18px] max-[820px]:pt-8 max-[820px]:pb-[60px]";
