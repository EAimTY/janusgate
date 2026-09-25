/**
 * The static requiredness line a field the submit gate demands states from
 * first paint: one quiet, always-visible sentence naming what a disabled
 * submit is waiting for, before any interaction. It is the static
 * counterpart of the field's lazy inline error — the error fires only once
 * the field is typed and never stands in for this line; both channels
 * render, the error above the hint, each explaining its own moment.
 */
export function requiredHint(detail: string): string {
    return `Required. ${detail}`;
}
