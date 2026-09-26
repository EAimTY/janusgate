import { type Accessor, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { registerDirtyForm } from "/src/dirty-form-registry";
import type { ActionName } from "/src/management-api/diagnosis";
import { isAbortFailure } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { type ManagementCallOutcome, performManagementCall } from "/src/management-api/outcome";
import { isOAuthProviderRow } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { describeCallFailure, type FailureCall } from "/src/screens/failure";
import type { NoticeTone } from "/src/ui/feedback";

export type { FailureCall };

/** What a screen states about its last read or action, rendered as a `Notice`. */
export type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: string;
    readonly detail?: string | null | undefined;
    readonly fix?: string | null | undefined;
};

/** The bad notice a failed call renders: the app's own diagnosis of the failure. */
export function screenFailureNotice(cause: unknown, call?: FailureCall): ScreenNotice {
    const diagnosis = describeCallFailure(cause, call);

    return {
        tone: "bad",
        title: diagnosis.title,
        detail: diagnosis.detail,
        fix: diagnosis.fix,
    };
}

/**
 * The failure of one read call, carrying the notice the screen states. The
 * screen harness catches it to install the diagnosis where the read's
 * failures are stated; anything else escaping a load is a bug and stays loud.
 */
export class ReadCallFailure extends Error {
    readonly notice: ScreenNotice;

    constructor(cause: unknown, call?: FailureCall) {
        super("A management read failed; the diagnosis is carried in `notice`.");
        this.name = "ReadCallFailure";
        this.notice = screenFailureNotice(cause, call);
    }
}

/** One management call inside a read cycle, run under the cycle's abort signal. */
export type ReadCall<TResult> = {
    /** The action the call attempts, which several failure diagnoses specialize on. */
    readonly action?: ActionName | undefined;
    readonly run: (signal: AbortSignal) => Promise<TResult>;
};

/** What an action's failure reporting accepts beside its diagnosis. */
export type ReportActionOptions = {
    /**
     * Runs when the diagnosis says the row the call addressed is gone — the
     * screen's reload, usually, so the not-found state takes over instead of
     * a ghost form accepting re-saves forever (README §6.4).
     */
    readonly onGone?: (() => void) | undefined;
};

/**
 * Runs a read cycle's calls in parallel under one signal, answering every
 * result in call order. An aborted call ends the cycle ahead of any failure
 * (README §6.4); the first failed call in call order becomes the cycle's
 * `ReadCallFailure`, exactly as the call list is written. A failure that is
 * itself a nested read's `ReadCallFailure` — a composite read call runs an
 * inner `readAll` — is adopted whole rather than re-wrapped: its notice
 * already carries the leaf call's diagnosis, and adoption at every level lets
 * that diagnosis survive any nesting depth, where a re-wrap would bury it
 * under the generic "The call failed." rendering and lose the gateway's
 * verbatim message (README §6.4).
 */
export async function readAll<TResults extends readonly unknown[]>(
    signal: AbortSignal,
    ...calls: { [TIndex in keyof TResults]: ReadCall<TResults[TIndex]> }
): Promise<TResults> {
    const outcomes = await Promise.all(
        calls.map((entry) => performManagementCall(() => entry.run(signal))),
    );

    for (const outcome of outcomes) {
        if (outcome.status === "aborted") {
            throw new DOMException("This read cycle was aborted.", "AbortError");
        }
    }

    const results: unknown[] = [];

    for (const [index, outcome] of outcomes.entries()) {
        if (outcome.status === "failed") {
            if (outcome.cause instanceof ReadCallFailure) {
                throw outcome.cause;
            }

            throw new ReadCallFailure(outcome.cause, {
                action: calls[index]?.action,
            });
        }

        results.push(outcome.status === "ok" ? outcome.result : undefined);
    }

    // Every outcome is known `ok` at this point, so the collected results are
    // exactly the tuple the mapped call list promised; the double assertion
    // only restores that tuple's shape for the caller.
    return results as unknown as TResults;
}

/** Runs one management call as a read cycle of its own. */
export async function readOne<TResult>(
    signal: AbortSignal,
    call: ReadCall<TResult>,
): Promise<TResult> {
    const [result] = await readAll(signal, call);
    return result;
}

/**
 * Whether the skeleton window is open: a read is in flight, no read has ever
 * succeeded, and nothing stands in the notice. The notice clause is
 * load-bearing — it keeps a standing failure visible while its re-read runs
 * (on an `isNoticeKeptOnSuccess` screen a standing notice may also be an
 * action's reported outcome), so the skeleton never replaces it. The
 * harness's `isFirstRead` accessor is this one predicate over its own
 * signals; a screen outside the harness — the usage query screen, whose read
 * cycle is also a form submission — calls it with its own.
 */
export function isSkeletonWindowOpen(
    isReading: boolean,
    hasRead: boolean,
    notice: ScreenNotice | null,
): boolean {
    return isReading && !hasRead && notice === null;
}

/** One mounted screen's read cycle: what every screen shares around its reads. */
export type ScreenRead = {
    /** Whether any read has ever succeeded on this screen. */
    readonly hasRead: Accessor<boolean>;
    /** Whether a read cycle is in flight — including the first one. */
    readonly isReading: Accessor<boolean>;
    /** Whether the skeleton window is open — `isSkeletonWindowOpen` over this harness's signals. */
    readonly isFirstRead: Accessor<boolean>;
    /** Whether the screen has gone away: actions answering later must stop. */
    readonly isDisposed: Accessor<boolean>;
    /** The standing notice: the last read failure, or the last action's outcome. */
    readonly notice: Accessor<ScreenNotice | null>;
    /** States an action's outcome where the read failures are stated. */
    readonly showNotice: (notice: ScreenNotice | null) => void;
    /**
     * States a finished action call's outcome where the read failures are
     * stated, and answers whether the call succeeded, so the caller keeps
     * only its success half. An aborted call states nothing; a failed one is
     * diagnosed into the standing notice — a `gone` diagnosis first runs
     * `onGone`.
     */
    readonly reportAction: <TResult>(
        outcome: ManagementCallOutcome<TResult>,
        call: FailureCall,
        options?: ReportActionOptions,
    ) => outcome is Extract<
        ManagementCallOutcome<TResult>,
        {
            readonly status: "ok";
        }
    >;
    /** Runs one read cycle now — a successful action's explicit reload. */
    readonly reload: () => void;
};

/**
 * Where the harness's standing notice came from: a failed read cycle's
 * diagnosis, or an action's reported outcome. The origin decides what may
 * clear the notice (the two-origin rule, README §6.3).
 */
type NoticeOrigin = "read" | "action";

/** The harness's standing notice, paired with the origin its clearing reads. */
type StandingNotice = {
    readonly notice: ScreenNotice;
    readonly origin: NoticeOrigin;
};

/**
 * The screen read harness (README §6.3): one read per mount, repeated when
 * *refresh* is pressed, cancelled when the screen goes away, with the first
 * failure diagnosed where failures are stated. A route-identity move is a
 * keyed remount (README §6.3) — the fresh mount runs its own first read — so
 * this harness follows the refresh pulse alone. The screen owns its data
 * signals and the `load` body — what this screen loads and where each answer
 * goes — while the harness owns the signals every screen redeclared, the
 * abort lifecycle, the refresh subscription, and the diagnosis mapping.
 *
 * A failed cycle replaces the standing notice with its diagnosis. A
 * successful cycle clears it — except where `isNoticeKeptOnSuccess` holds,
 * for screens whose notice is also an action's reported outcome. The
 * exception is an origin rule, not a blanket (README §6.3): the policy keeps
 * only what an action stated, so a save's outcome survives the reload the
 * save itself triggered, while a read-failure notice diagnoses the last read
 * cycle and is cleared by the first read that succeeds, on kept screens too
 * — the Read-again recovery lands on the row, not under its failure.
 */
export function createScreenRead(options: {
    /** The screen's reads, run under the cycle's signal; throws `ReadCallFailure` to fail the cycle. */
    readonly load: (signal: AbortSignal) => Promise<void>;
    /**
     * Whether a successful read keeps a notice an action stated (a read
     * failure is cleared by the read that succeeds regardless).
     */
    readonly isNoticeKeptOnSuccess?: boolean;
}): ScreenRead {
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [standing, setStanding] = createSignal<StandingNotice | null>(null);
    const [isDisposed, setIsDisposed] = createSignal(false);

    // The notice and its origin are one signal, so every write pairs them
    // and the clearing rule always reads the origin of the notice that
    // actually stands — one notice owner, with no second state to drift.
    const notice = (): ScreenNotice | null => standing()?.notice ?? null;

    let controller: AbortController | null = null;

    const run = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        try {
            await options.load(call.signal);
        } catch (error) {
            if (isAbortFailure(error) || call.signal.aborted) {
                // Superseded or disposed: an aborted read installs nothing —
                // not even its diagnosis, which belongs to a cycle the screen
                // has already replaced.
                return;
            }

            if (error instanceof ReadCallFailure) {
                setIsReading(false);
                setStanding({
                    notice: error.notice,
                    origin: "read",
                });
                return;
            }

            throw error;
        }

        if (call.signal.aborted) {
            // Superseded after the reads resolved but before this
            // continuation ran: the cycle is dead either way, and the newer
            // cycle owns the screen — nothing installs, not even `hasRead`.
            return;
        }

        setIsReading(false);

        // The two-origin rule (README §6.3): the keep policy exists so a
        // save's reported outcome survives the reload the save itself
        // triggered, so it shields only a notice an action stated. A
        // read-failure notice diagnoses the last read cycle — this one,
        // which just succeeded — and is stale on any screen, kept or not,
        // so a successful read clears it. Neither origin reaches across:
        // reads never clear an action's notice on a kept screen, and
        // actions never clear a read's except by stating their own or
        // dismissing explicitly (`showNotice(null)`).
        const isNoticeShielded =
            options.isNoticeKeptOnSuccess === true && standing()?.origin === "action";

        if (!isNoticeShielded) {
            setStanding(null);
        }

        setHasRead(true);
        reportRefresh();
    };

    createEffect(
        () => refreshRequest(),
        () => {
            void run();

            return () => {
                controller?.abort();
            };
        },
    );

    onCleanup(() => {
        setIsDisposed(true);
        controller?.abort();
    });

    // Computed once, so every screen states the skeleton's window through the
    // same one definition instead of re-spelling the three-clause gate.
    const isFirstRead = createMemo(() => isSkeletonWindowOpen(isReading(), hasRead(), notice()));

    return {
        hasRead,
        isReading,
        isFirstRead,
        isDisposed,
        notice,
        showNotice: (stated) => {
            setStanding(
                stated === null
                    ? null
                    : {
                          notice: stated,
                          origin: "action",
                      },
            );
        },
        reportAction: <TResult>(
            outcome: ManagementCallOutcome<TResult>,
            call: FailureCall,
            options?: ReportActionOptions,
        ): outcome is Extract<
            ManagementCallOutcome<TResult>,
            {
                readonly status: "ok";
            }
        > => {
            if (outcome.status === "aborted") {
                return false;
            }

            if (outcome.status === "failed") {
                if (describeCallFailure(outcome.cause, call).kind === "gone") {
                    options?.onGone?.();
                }

                setStanding({
                    notice: screenFailureNotice(outcome.cause, call),
                    origin: "action",
                });

                return false;
            }

            return true;
        },
        reload: () => {
            if (!isDisposed()) {
                void run();
            }
        },
    };
}

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
export function createHeldRow<TRow>(
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
 * `reportFormDirty` also mirrors the form's dirt into the dirty-form registry
 * (`/src/dirty-form-registry`), the live truth the shell's `beforeunload`
 * guard reads: a tab close, a reload, or a navigation off the document asks
 * for confirmation while edits stand (README §6.3).
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

    // The same dirt report feeds the row hold above and the dirty-form
    // registry, whose live truth the shell's `beforeunload` guard reads —
    // there is never a second dirt source (README §6.3). The seat dies with
    // this mount's owner, so a screen that goes away — dirty or not — cannot
    // leave the guard armed behind it; a remounted form registers afresh.
    const formRegistration = registerDirtyForm();
    onCleanup(() => {
        formRegistration.unregister();
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
            formRegistration.reportDirty(isDirty);
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
 * edit's owning provider, re-parsed on every read like every listing.
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

/**
 * Whether a fresh read answers the same row the screen already holds,
 * content and all. An equal row keeps its identity, so a refresh that
 * changes nothing never remounts — and never resets — a form opened on the
 * row (README §6.3's one-pattern reads must not cost an operator input).
 */
export function isSameProviderRow(previous: ProviderResponse, fresh: ProviderResponse): boolean {
    return (
        previous.type === fresh.type &&
        previous.id === fresh.id &&
        previous.name === fresh.name &&
        previous.model_prefix === fresh.model_prefix &&
        previous.is_enabled === fresh.is_enabled &&
        previous.provider_definition_name === fresh.provider_definition_name &&
        previous.provider_definition_version === fresh.provider_definition_version &&
        previous.format === fresh.format &&
        previous.base_url === fresh.base_url &&
        (isOAuthProviderRow(previous)
            ? isOAuthProviderRow(fresh) && previous.expires_at === fresh.expires_at
            : !isOAuthProviderRow(fresh))
    );
}

/** Whether a fresh read answers the same model row, content and all. */
export function isSameModelRow(previous: ModelResponse, fresh: ModelResponse): boolean {
    return (
        previous.id === fresh.id &&
        previous.provider_id === fresh.provider_id &&
        previous.provider === fresh.provider &&
        previous.upstream_id === fresh.upstream_id &&
        previous.public_id === fresh.public_id &&
        previous.id_override === fresh.id_override &&
        previous.is_enabled === fresh.is_enabled &&
        previous.discovery_state === fresh.discovery_state
    );
}

/** Whether a fresh read answers the same serving-key row, content and all. */
export function isSameApiKeyRow(previous: ApiKeyResponse, fresh: ApiKeyResponse): boolean {
    return (
        previous.id === fresh.id &&
        previous.name === fresh.name &&
        previous.key === fresh.key &&
        previous.is_enabled === fresh.is_enabled &&
        previous.created_at === fresh.created_at
    );
}
