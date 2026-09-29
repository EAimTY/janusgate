/**
 * The inline confirms' shared focus contract. Arming an inline confirm swaps
 * its trigger for the armed question, which would silently drop keyboard
 * focus onto the document body and leave a screen reader unaware that a
 * question replaced the button: arming records the focused element as the
 * restore point and moves focus onto the armed question (carrying
 * `tabindex="-1"` and focused through its `ref`), so the question is what a
 * screen reader reads next and the next Tab lands on the confirmation
 * button beside it. Disarming swaps the question back for the trigger, and
 * the restore hands focus back to the arming trigger — the element itself
 * where it survives the swap (a picker arming a confirm beside itself),
 * otherwise the trigger control the disarmed branch remounts (claimed
 * through the `ref` on that branch's wrapper).
 *
 * Each confirm keeps its own instance: overlapping arms of different
 * confirms cannot misplace each other's restore point.
 *
 * Where the contract runs from a `ref`, its focus work is deferred by one
 * macrotask: Solid creates a branch's elements before inserting them, so
 * the element is focusable only once the swap has settled into the
 * document.
 */

/** The calls one inline confirm's handlers make and the two `ref` callbacks its branches carry. */
export interface InlineConfirmFocus {
    /** Records the arm moment's focus; call from the arming handler, before the swap. */
    readonly arm: () => void;
    /**
     * Focuses the armed question once it mounts; attach to the question
     * with `ref` and give it `tabindex="-1"` — the armed branches do so
     * through the shared `InlineConfirmQuestion` fragment
     * (`./inline-confirm-question`), which renders exactly that element. A
     * branch remount no arm asked for — the armed state yielding to an
     * unrelated signal — takes no focus.
     */
    readonly focusArmed: (question: HTMLElement) => void;
    /**
     * Returns focus to the arming trigger; call from every disarm that
     * answers an arm — the cancel, and the confirm that acts. Where the
     * swap unmounted the trigger, the remounted trigger's `ref` performs
     * the restore.
     */
    readonly disarm: () => void;
    /**
     * Restores the remounted trigger's focus; attach with `ref` to the
     * disarmed branch's wrapper, whose first button is the trigger. Mounts
     * no disarm asked for — the initial one, or a disarmed branch coming
     * back for an unrelated reason — restore nothing.
     */
    readonly restoreDisarmed: (disarmedBranch: HTMLElement) => void;
}

/**
 * The focus contract of one inline confirm: `arm` and `disarm` go into its
 * event handlers, `focusArmed` onto the armed question, `restoreDisarmed`
 * onto the disarmed branch's trigger wrapper.
 */
export function createInlineConfirmFocus(): InlineConfirmFocus {
    /** The element focus must return to, as the arm moment recorded it. */
    let restoreElement: HTMLElement | null = null;
    /** Whether the armed branch's next mount is the one the arm asked for. */
    let isArmFocusPending = false;
    /** Whether a disarm still owes the restore to the arming trigger. */
    let isRestorePending = false;

    return {
        arm: () => {
            restoreElement =
                document.activeElement instanceof HTMLElement ? document.activeElement : null;
            isArmFocusPending = true;
            isRestorePending = false;
        },
        focusArmed: (question) => {
            if (!isArmFocusPending) {
                return;
            }

            isArmFocusPending = false;
            setTimeout(() => {
                question.focus();
            }, 0);
        },
        disarm: () => {
            if (restoreElement === null) {
                return;
            }

            isRestorePending = true;
            const element = restoreElement;
            setTimeout(() => {
                // The remounted trigger's `ref` claims the restore first
                // (its branch mounts on a microtask, this runs on a timer),
                // so this path is the restore only where the trigger
                // survived the swap.
                if (!isRestorePending) {
                    return;
                }

                isRestorePending = false;
                restoreElement = null;

                if (element.isConnected) {
                    element.focus();
                }
            }, 0);
        },
        restoreDisarmed: (disarmedBranch) => {
            if (!isRestorePending) {
                return;
            }

            isRestorePending = false;
            restoreElement = null;
            setTimeout(() => {
                for (const candidate of disarmedBranch.querySelectorAll("button")) {
                    if (candidate instanceof HTMLElement) {
                        candidate.focus();
                        return;
                    }
                }
            }, 0);
        },
    };
}
