/**
 * Reads the authorization code out of whatever the operator pastes after
 * approving access: either the bare code or the whole callback URL. The
 * gateway only accepts the code itself, so a pasted URL is reduced to its
 * `code` parameter here instead of asking the operator to find it by hand.
 * A paste that carries neither yields `null`, and the caller says what to
 * paste instead of sending a value the gateway would burn the flow on.
 */
export function extractAuthorizationCode(input: string): string | null {
    const trimmed = input.trim();

    if (trimmed === "") {
        return null;
    }

    const parameter = /[?&#]code=([^&#\s]*)/.exec(trimmed);

    if (parameter !== null) {
        const raw = parameter[1] ?? "";

        if (raw === "") {
            return null;
        }

        try {
            const decoded = decodeURIComponent(raw);

            return decoded === "" ? null : decoded;
        } catch {
            return raw;
        }
    }

    if (trimmed.startsWith("code=")) {
        const raw = trimmed.slice("code=".length).trim();

        return raw === "" ? null : raw;
    }

    // Anything left carrying URL structure or whitespace is a paste mistake,
    // mirroring the gateway's own rejection, so the flow survives it.
    if (/[\s?#&]/.test(trimmed)) {
        return null;
    }

    return trimmed;
}
