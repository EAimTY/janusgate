export type TranslationErrorCode =
    | "invalid_json"
    | "unsupported_parameter"
    | "unsupported_tool_type"
    | "upstream_failure";

export class TranslationError extends Error {
    readonly code: TranslationErrorCode;

    constructor(code: TranslationErrorCode, message: string) {
        super(message);
        this.name = "TranslationError";
        this.code = code;
    }
}
