type GatewayErrorType =
    | "api_error"
    | "authentication_error"
    | "invalid_request_error"
    | "not_found_error"
    | "rate_limit_error"
    | "upstream_error";

interface GatewayErrorBody {
    readonly error: {
        readonly message: string;
        readonly type: GatewayErrorType;
        readonly code?: string;
    };
}

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

export class GatewayError extends Error {
    public readonly status: 400 | 401 | 404 | 413 | 429 | 500 | 502 | 504;
    public readonly type: GatewayErrorType;
    public readonly code: string | undefined;

    public constructor(
        status: 400 | 401 | 404 | 413 | 429 | 500 | 502 | 504,
        type: GatewayErrorType,
        message: string,
        code?: string,
    ) {
        super(message);
        this.name = "GatewayError";
        this.status = status;
        this.type = type;
        this.code = code;
    }
}

export function gatewayErrorBody(error: GatewayError): GatewayErrorBody {
    if (error.code === undefined) {
        return {
            error: {
                message: error.message,
                type: error.type,
            },
        };
    }
    return {
        error: {
            code: error.code,
            message: error.message,
            type: error.type,
        },
    };
}

export function internalGatewayError(): GatewayError {
    return new GatewayError(500, "api_error", "An internal error occurred", "internal_error");
}

export function invalidJsonError(): GatewayError {
    return new GatewayError(
        400,
        "invalid_request_error",
        "The request body is not valid JSON",
        "invalid_json",
    );
}

export function payloadTooLargeError(): GatewayError {
    return new GatewayError(
        413,
        "invalid_request_error",
        "The request body exceeds the maximum allowed size",
        "payload_too_large",
    );
}

export function invalidApiKeyError(): GatewayError {
    return new GatewayError(
        401,
        "authentication_error",
        "The provided API key is invalid",
        "invalid_api_key",
    );
}

export function modelNotFoundError(): GatewayError {
    return new GatewayError(
        404,
        "not_found_error",
        "The requested model was not found",
        "model_not_found",
    );
}

export function upstreamFailureError(): GatewayError {
    return new GatewayError(
        502,
        "upstream_error",
        "The upstream request failed",
        "upstream_failure",
    );
}

export function upstreamTimeoutError(): GatewayError {
    return new GatewayError(
        504,
        "upstream_error",
        "The upstream request timed out",
        "upstream_timeout",
    );
}

export function upstreamResponseTooLargeError(): GatewayError {
    return new GatewayError(
        502,
        "upstream_error",
        "The upstream response exceeds the maximum allowed size",
        "upstream_response_too_large",
    );
}
