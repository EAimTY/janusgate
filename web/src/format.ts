/** A unix timestamp (seconds) in the browser's locale. */
export function formatTimestamp(unixSeconds: number): string {
    return new Date(unixSeconds * 1000).toLocaleString();
}
