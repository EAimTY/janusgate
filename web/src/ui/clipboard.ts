/**
 * Copies one value to the operating-system clipboard, reporting whether any
 * path took it. The async Clipboard API is tried first; a browser that
 * refuses it — an insecure context leaves `navigator.clipboard` undefined,
 * and a denied permission or an unfocused document rejects the write — falls
 * back to the deprecated `document.execCommand("copy")` path through a
 * transient textarea, which reports its own failure as `false`. The textarea
 * is `readonly` and pinned fixed off-screen so Safari's implicit focus on
 * `select()` cannot scroll the document to reveal it.
 */
export async function copyText(value: string): Promise<boolean> {
    if (navigator.clipboard !== undefined) {
        try {
            await navigator.clipboard.writeText(value);
            return true;
        } catch {
            // Refused — the legacy path below is the second chance.
        }
    }

    const area = document.createElement("textarea");
    area.value = value;
    // `readonly` keeps iOS Safari from opening the keyboard on the implicit
    // focus, and the fixed off-screen placement means that focus can never
    // scroll the document — fixed-position elements are unreachable by it.
    area.setAttribute("readonly", "");
    area.style.position = "fixed";
    area.style.top = "-9999px";
    area.style.left = "-9999px";
    document.body.appendChild(area);
    area.select();

    try {
        return document.execCommand("copy");
    } finally {
        area.remove();
    }
}
