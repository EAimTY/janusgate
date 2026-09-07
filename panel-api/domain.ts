/** Counts reported by a completed model-discovery run (§12.11). */
export interface DiscoveryResult {
    readonly total: number;
    readonly created: number;
    readonly removed: number;
    readonly skipped: number;
}
