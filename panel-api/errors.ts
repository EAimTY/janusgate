export class PanelApiError extends Error {
    public constructor(
        public readonly status: 400 | 401 | 403 | 404 | 409 | 502,
        message: string,
    ) {
        super(message);
        this.name = "PanelApiError";
    }
}

export function isDatabaseConstraintError(error: unknown, constraint: string): boolean {
    return error instanceof Error && error.message.includes(constraint);
}
