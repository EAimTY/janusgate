import { type Accessor, createEffect, createSignal, onCleanup } from "solid-js";
import type { ActionName } from "/src/management-api/diagnosis";
import { isAbortFailure } from "/src/management-api/errors";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
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

/**
 * Runs a read cycle's calls in parallel under one signal, answering every
 * result in call order. An aborted call ends the cycle ahead of any failure
 * (README §6.4); the first failed call in call order becomes the cycle's
 * `ReadCallFailure`, exactly as the call list is written.
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

/** One mounted screen's read cycle: what every screen shares around its reads. */
export type ScreenRead = {
    /** Whether any read has ever succeeded on this screen. */
    readonly hasRead: Accessor<boolean>;
    /** Whether a read cycle is in flight — including the first one. */
    readonly isReading: Accessor<boolean>;
    /** Whether the screen has gone away: actions answering later must stop. */
    readonly isDisposed: Accessor<boolean>;
    /** The standing notice: the last read failure, or the last action's outcome. */
    readonly notice: Accessor<ScreenNotice | null>;
    /** States an action's outcome where the read failures are stated. */
    readonly showNotice: (notice: ScreenNotice | null) => void;
    /** Diagnoses a failed action call into the standing notice. */
    readonly reportFailure: (cause: unknown, call?: FailureCall) => void;
    /** Runs one read cycle now — a successful action's explicit reload. */
    readonly reload: () => void;
};

/**
 * The screen read harness (README §6.3): one read per mount, repeated when
 * *refresh* is pressed or the screen's route identity moves, cancelled when
 * the screen goes away, with the first failure diagnosed where failures are
 * stated. The screen owns its data signals and the `load` body — what this
 * screen loads and where each answer goes — while the harness owns the
 * signals every screen redeclared, the abort lifecycle, the refresh
 * subscription, and the diagnosis mapping.
 *
 * A failed cycle replaces the standing notice with its diagnosis. A
 * successful cycle clears it — except where `isNoticeKeptOnSuccess` holds,
 * for screens whose notice is also an action's reported outcome, where only
 * a new failure replaces it.
 */
export function createScreenRead(options: {
    /** The screen's reads, run under the cycle's signal; throws `ReadCallFailure` to fail the cycle. */
    readonly load: (signal: AbortSignal) => Promise<void>;
    /** An additional reactive source the read follows beside the refresh pulse (a route id). */
    readonly reReadOn?: Accessor<unknown> | undefined;
    /** Whether a successful read keeps the standing notice (an action's reported outcome). */
    readonly isNoticeKeptOnSuccess?: boolean;
}): ScreenRead {
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(null);
    const [isDisposed, setIsDisposed] = createSignal(false);

    let controller: AbortController | null = null;

    const run = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        try {
            await options.load(call.signal);
        } catch (error) {
            if (error instanceof ReadCallFailure) {
                setIsReading(false);
                setNotice(error.notice);
                return;
            }

            if (isAbortFailure(error) || call.signal.aborted) {
                // Superseded or disposed: an aborted read installs nothing.
                return;
            }

            throw error;
        }

        setIsReading(false);

        if (options.isNoticeKeptOnSuccess !== true) {
            setNotice(null);
        }

        setHasRead(true);
        reportRefresh();
    };

    createEffect(
        () => ({
            generation: refreshRequest(),
            source: options.reReadOn?.(),
        }),
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

    return {
        hasRead,
        isReading,
        isDisposed,
        notice,
        showNotice: (standing) => {
            setNotice(standing);
        },
        reportFailure: (cause, call) => {
            setNotice(screenFailureNotice(cause, call));
        },
        reload: () => {
            if (!isDisposed()) {
                void run();
            }
        },
    };
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
        (isOAuthProviderRow(previous)
            ? isOAuthProviderRow(fresh) && previous.expires_at === fresh.expires_at
            : !isOAuthProviderRow(fresh) && previous.base_url === fresh.base_url)
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
