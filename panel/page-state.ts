import { type Accessor, createSignal, onSettled, type Setter } from "solid-js";
import { errorMessage } from "./api-client";

/** Loading gate and page-level error state shared by the SPA pages around their data fetches. */
interface PageState {
    readonly isLoading: Accessor<boolean>;
    readonly error: Accessor<string | null>;
    readonly setError: Setter<string | null>;
    /** Re-runs the initial load through the same loading and error handling. */
    readonly retry: () => void;
}

/**
 * Owns the loading and error state around a page's initial data fetch. The loader runs once
 * after the page's first stable render and fetches and writes the page's own data signals, so
 * data reactivity stays page-owned; later action handlers surface their failures through
 * `setError` in the same page-level notice. A failed initial load leaves the page with no
 * data, so `retry` re-enters the identical path instead of forcing a manual reload.
 *
 * Must be called in component scope: `onSettled` binds the initial fetch to the owning
 * component. The state is plain signals, so no cleanup is needed on disposal.
 */
export function createPageState(loader: () => Promise<unknown>): PageState {
    const [isLoading, setIsLoading] = createSignal(true);
    const [error, setError] = createSignal<string | null>(null);

    async function initialLoad(): Promise<void> {
        setIsLoading(true);
        setError(null);
        try {
            await loader();
        } catch (caughtError) {
            setError(errorMessage(caughtError));
        } finally {
            setIsLoading(false);
        }
    }

    function retry(): void {
        void initialLoad();
    }

    onSettled(() => {
        void initialLoad();
    });

    return {
        isLoading,
        error,
        setError,
        retry,
    };
}
