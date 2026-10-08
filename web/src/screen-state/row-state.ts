import type { Accessor } from "solid-js";
import { createMemo, createSignal, onCleanup } from "solid-js";
import { registerLeaveGuardSubject } from "/src/leave-guard-registry";

/**
 * The edit screens' held row (README §6.3): while the settings form holds
 * unsaved edits, the row keeps its identity whatever a read answers, so an
 * open dirty form is never reset; a row that genuinely changed on the
 * gateway while the form is dirty is held back, and `isRowHeld` states it —
 * the Save-or-Revert notice's trigger. Once the form is clean, a read's row
 * is adopted; a read answering the same content keeps the row's identity, so
 * a refresh that changed nothing never remounts the form. The one exception
 * is a read answering null — the row was deleted on the gateway: the row
 * becomes null and nothing is held, dirt included, because deletion ends the
 * form (the screen's not-found state takes over).
 */
function createHeldRow<TRow>(
    latest: Accessor<TRow | null>,
    isFormDirty: Accessor<boolean>,
    isSameRow: (previous: TRow, fresh: TRow) => boolean,
): {
    readonly row: Accessor<TRow | null>;
    readonly isRowHeld: Accessor<boolean>;
} {
    const row = createMemo((previous: TRow | null | undefined) => {
        const fresh = latest();

        if (previous === null || previous === undefined || fresh === null) {
            return fresh;
        }

        return isSameRow(previous, fresh) || isFormDirty() ? previous : fresh;
    });

    const isRowHeld = createMemo(() => {
        const current = row();
        const fresh = latest();

        return current !== null && fresh !== null && !isSameRow(current, fresh);
    });

    return {
        row,
        isRowHeld,
    };
}

/**
 * The edit screens' row (README §6.3): the held row of `createHeldRow` plus
 * the save-settling latch. While the settings form holds unsaved edits — or
 * a save is still settling — the row keeps its identity whatever a read
 * answers, so an open dirty form is never reset; a row that genuinely
 * changed on the gateway while the hold stands is held back, and `isRowHeld`
 * states it — the Save-or-Revert notice's trigger. Once the form is clean
 * and no save is settling, a read's row is adopted; a read answering the
 * same content keeps the row's identity, so a refresh that changed nothing
 * never remounts the form. The one exception is a read answering null — the
 * row was deleted on the gateway: the row becomes null and nothing is held,
 * dirt included, because deletion ends the form (the screen's not-found
 * state takes over).
 *
 * The latch covers the window between a save and its own reload: the save
 * overwrote exactly what the standing listing still shows, so the listing is
 * stale by construction and the held row must not adopt it (a save
 * re-baselines the form onto the values it sent). `reportSaved` sets the
 * latch when a save succeeds; the save's own reload clears it once its reads
 * land, so a failed reload leaves the hold standing instead of resting the
 * screen on the pre-save rows.
 *
 * `reportFormDirty` also mirrors the form's dirt into the leave-guard
 * registry (`/src/leave-guard-registry`), the live truth the shell's
 * `beforeunload` guard reads: a tab close, a reload, or a navigation off
 * the document asks for confirmation while edits stand (README §6.3).
 */
export function createEditRow<TRow>(
    latest: Accessor<TRow | null>,
    isSameRow: (previous: TRow, fresh: TRow) => boolean,
): {
    readonly row: Accessor<TRow | null>;
    readonly isRowHeld: Accessor<boolean>;
    /** Reports the settings form's dirt — the hold's first clause and the leave guard's mirror. */
    readonly reportFormDirty: (isDirty: boolean) => void;
    /** Reports a save that succeeded: the hold stands until its reload lands. */
    readonly reportSaved: () => void;
    /** Reports a read cycle whose reads landed, resolving the save's hold. */
    readonly reportReloaded: () => void;
} {
    const [isFormDirty, setIsFormDirty] = createSignal(false);
    const [isSaveSettling, setIsSaveSettling] = createSignal(false);

    // The same dirt report feeds the row hold above and the leave-guard
    // registry, whose live truth the shell's `beforeunload` guard reads —
    // there is never a second dirt source (README §6.3). The seat dies with
    // this mount's owner, so a screen that goes away — dirty or not — cannot
    // leave the guard armed behind it; a remounted form registers afresh.
    const formSeat = registerLeaveGuardSubject();
    onCleanup(() => {
        formSeat.release();
    });

    const { row, isRowHeld } = createHeldRow(
        latest,
        () => isFormDirty() || isSaveSettling(),
        isSameRow,
    );

    return {
        row,
        isRowHeld,
        reportFormDirty: (isDirty: boolean): void => {
            setIsFormDirty(isDirty);
            formSeat.reportGuarded(isDirty);
        },
        reportSaved: () => {
            setIsSaveSettling(true);
        },
        reportReloaded: () => {
            setIsSaveSettling(false);
        },
    };
}

/**
 * The identity-pinning memo beside `createHeldRow`, without the hold
 * condition (README §6.3): a read answering the same row content keeps the
 * previous object's identity, so a refresh that changed nothing never
 * remounts what is mounted on the row; a changed answer adopts the fresh
 * object; a gone answer passes null through. It is for lookups whose subject
 * is not the form's own row and so cannot hold edits — such as the model
 * detail's owning provider, the lookup beside the model's own pinned row,
 * re-parsed on every read like every listing.
 */
export function createPinnedRow<TRow>(
    latest: Accessor<TRow | null>,
    isSameRow: (previous: TRow, fresh: TRow) => boolean,
): Accessor<TRow | null> {
    return createMemo((previous: TRow | null | undefined) => {
        const fresh = latest();

        if (previous === null || previous === undefined || fresh === null) {
            return fresh;
        }

        return isSameRow(previous, fresh) ? previous : fresh;
    });
}
