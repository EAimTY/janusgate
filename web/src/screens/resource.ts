import { type Accessor, createSignal, onSettled } from "solid-js";
import { performManagementCall } from "/src/management-api/outcome";

export type AsyncResource<TValue> = {
    /** The loaded value, or `null` while the first load is in flight. */
    readonly value: Accessor<TValue | null>;
    readonly failure: Accessor<string | null>;
    readonly isLoading: Accessor<boolean>;
    readonly reload: () => Promise<void>;
    /** Installs a value a write already returned, without refetching it. */
    readonly setValue: (value: TValue) => void;
};

/**
 * A screen's read of the management API. It loads once when the screen mounts,
 * cancels an in-flight call when the screen goes away, and can be reloaded
 * after a mutation. A failed reload keeps the previous value visible and
 * reports the gateway's message alongside it.
 */
export function createAsyncResource<TValue>(
    load: (signal: AbortSignal) => Promise<TValue>,
): AsyncResource<TValue> {
    const [value, setValue] = createSignal<TValue | null>(null);
    const [failure, setFailure] = createSignal<string | null>(null);
    let controller: AbortController | null = null;

    const reload = async (): Promise<void> => {
        controller?.abort();
        const nextController = new AbortController();
        controller = nextController;

        const outcome = await performManagementCall(() => load(nextController.signal));

        // A superseded or disposed load must not report anything.
        if (nextController.signal.aborted) {
            return;
        }

        if (outcome.status === "ok") {
            setValue(() => outcome.result);
            setFailure(null);
        } else if (outcome.status === "failed") {
            setFailure(outcome.message);
        }
    };

    onSettled(() => {
        void reload();

        return () => {
            controller?.abort();
        };
    });

    return {
        value,
        failure,
        isLoading: () => value() === null && failure() === null,
        reload,
        setValue: (nextValue: TValue) => {
            setValue(() => nextValue);
        },
    };
}
