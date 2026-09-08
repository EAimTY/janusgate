// Neutral, cross-layer error types: no HTTP status and no wire-format semantics, so each entry
// point maps them onto its own error surface. The OpenAI gateway envelope vocabulary lives in
// /api/errors.

export class ConfigurationError extends Error {
    public constructor(message: string) {
        super(message);
        this.name = "ConfigurationError";
    }
}

export class ValidationError extends Error {
    public constructor(message: string) {
        super(message);
        this.name = "ValidationError";
    }
}

export class CredentialDecryptionError extends Error {
    public constructor() {
        super("Stored credentials could not be decrypted");
        this.name = "CredentialDecryptionError";
    }
}

interface ProviderAccessErrorOptions {
    /** Authentication was definitively rejected and no refresh recovered it (README §7). */
    readonly isAuthenticationRejected?: boolean;
}

/**
 * A credential-access failure (README §7): the identity provider rejected a token refresh, a
 * forced refresh could not run, or credentials remain rejected upstream. Deliberately neutral —
 * no HTTP status and no envelope semantics — so each entry point maps it onto its own error
 * surface. The message is the sanitized, length-bounded diagnostic, safe to surface; when the
 * identity provider supplied an RFC 6749 error code, it travels inside that message.
 */
export class ProviderAccessError extends Error {
    public readonly isAuthenticationRejected: boolean;

    public constructor(message: string, options: ProviderAccessErrorOptions = {}) {
        super(message);
        this.name = "ProviderAccessError";
        this.isAuthenticationRejected = options.isAuthenticationRejected ?? false;
    }
}
