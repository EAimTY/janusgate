import { onCleanup } from "solid-js";

/**
 * The one disposal guard the fragments that own late-answering calls — the
 * OAuth flow, the creation, the save, the confirmed delete — install and
 * read back: whether the reactive owner this runs in has gone away, so a
 * call answering afterwards states nothing and acts on nothing (`web/docs/architecture.md`). Must run inside a reactive owner: the `onCleanup` it registers
 * belongs to the caller's owner, the way every lifecycle registration here
 * does.
 */
export function createDisposalGuard(): () => boolean {
    let isDisposed = false;
    onCleanup(() => {
        isDisposed = true;
    });

    return () => isDisposed;
}
