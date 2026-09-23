import { type Accessor, createSignal } from "solid-js";

/**
 * The shared refresh pulse. The shell's Refresh button bumps the generation;
 * every mounted screen re-runs its read when the generation moves, and reports
 * the moment its read succeeded — that moment is what the shell stamps next
 * to the instance address.
 */
const [lastRefreshAt, setLastRefreshAt] = createSignal<Date | null>(null);
const [refreshGeneration, bumpRefreshGeneration] = createSignal(0);

/** When the visible state was last read from the gateway, if ever. */
export const refreshStamp: Accessor<Date | null> = lastRefreshAt;

/** Bumps whenever the operator asks every screen to read again. */
export const refreshRequest: Accessor<number> = refreshGeneration;

/** Records a successful read, stamping the shell. */
export function reportRefresh(): void {
    setLastRefreshAt(new Date());
}

/**
 * Forgets the stamp. The shell mounts per connection, and the freshness of
 * the visible state belongs to the instance the reads answer — a different
 * connection starts unrefreshed, whatever the previous instance last showed.
 */
export function resetRefreshStamp(): void {
    setLastRefreshAt(null);
}

/** Asks the mounted screen to read its state from the gateway again. */
export function requestRefresh(): void {
    bumpRefreshGeneration((generation) => generation + 1);
}
