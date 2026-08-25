import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";
import { buttonPrimaryClasses, Notice } from "./ui";

interface AuthOverlayProps {
    token: string;
    error: string | null;
    isChecking: boolean;
    onTokenInput: (token: string) => void;
    onSubmit: () => void;
}

export function AuthOverlay(props: AuthOverlayProps): JSX.Element {
    function submit(event: SubmitEvent): void {
        event.preventDefault();
        props.onSubmit();
    }

    return (
        <div class="fixed inset-0 z-200 grid min-w-80 place-items-center overflow-y-auto bg-[#0d1511] p-[22px] font-sans text-[#ecf6ef] [background:radial-gradient(circle_at_18%_18%,rgb(41_166_111/24%),transparent_26rem),radial-gradient(circle_at_85%_90%,rgb(41_166_111/12%),transparent_30rem),#0d1511] [text-rendering:optimizeLegibility]">
            <main class="w-full max-w-[430px] rounded-3xl border border-white/9 bg-[#18221de0] p-[38px] shadow-[0_35px_100px_rgb(0_0_0/38%)] backdrop-blur-2xl max-[540px]:px-[23px] max-[540px]:py-7">
                <div class="mb-6 grid size-[54px] place-items-center rounded-2xl border border-white/35 bg-linear-to-br from-[#70e2a9] to-[#27a36d] text-base font-[850] tracking-[-0.04em] text-[#0e3323] shadow-[0_12px_30px_rgb(19_113_72/28%)]">
                    JG
                </div>
                <p class="m-0 text-[11px] font-extrabold tracking-[0.16em] text-[#60d99b] uppercase">
                    JanusGate administration
                </p>
                <h1 class="mt-2 mb-3 text-[31px] leading-[1.04] font-bold tracking-[-0.045em] text-white">
                    Unlock the control plane
                </h1>
                <p class="mt-0 mb-6 text-[13px] leading-[1.6] text-[#9daba2]">
                    Enter the shared panel token. It stays only in this browser's local storage and
                    is sent as an Authorization header.
                </p>
                <Show when={props.error !== null}>
                    <Notice kind="error">{props.error}</Notice>
                </Show>
                <form
                    class="grid gap-[9px]"
                    onSubmit={submit}
                >
                    <label
                        class="text-[11px] font-bold text-[#cbd6cf]"
                        for="panel-token"
                    >
                        Panel token
                    </label>
                    <input
                        id="panel-token"
                        class="w-full rounded-lg border border-white/14 bg-white/6 p-3 text-[13px] leading-[1.35] text-[#f1f8f3] transition-[border-color,box-shadow,background] duration-150 hover:border-white/25 focus:border-[#29a66f] focus:bg-white/8 focus:outline-0 focus:ring-3 focus:ring-[#29a66f]/10 disabled:cursor-not-allowed disabled:opacity-60"
                        type="password"
                        autocomplete="current-password"
                        autofocus
                        required
                        value={props.token}
                        disabled={props.isChecking}
                        onInput={(event) => props.onTokenInput(event.currentTarget.value)}
                    />
                    <button
                        class={`${buttonPrimaryClasses} mt-[7px] w-full`}
                        type="submit"
                        disabled={props.isChecking || props.token.length === 0}
                    >
                        {props.isChecking ? "Verifying…" : "Continue"}
                    </button>
                </form>
                <p class="mt-[19px] mb-0 text-center text-[10px] text-[#68766d]">
                    No cookies. No session. One token, verified live.
                </p>
            </main>
        </div>
    );
}
