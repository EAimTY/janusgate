import { type Connection, MANAGEMENT_API_PATH } from "/src/connection/model";

import {
    ContractViolationError,
    describeManagementFailure,
    isAbortFailure,
    ManagementApiError,
} from "/src/management-api/errors";

import {
    type CompleteProviderOauthRequest,
    type CreateProviderRequest,
    type DeleteProviderRequest,
    type DiscoverModelsRequest,
    type DiscoverModelsResponse,
    ErrorCode,
    type ListModelsRequest,
    type ListModelsResponse,
    type ListProvidersRequest,
    type ListProvidersResponse,
    type ModelEnvelope,
    type Path,
    type ProviderEnvelope,
    type ReauthorizeProviderRequest,
    type ReauthorizeProviderResponse,
    type StartProviderOauthRequest,
    type StartProviderOauthResponse,
    type UpdateModelRequest,
    type UpdateProviderRequest,
} from "/src/management-api/generated/v0";

import {
    parseDiscoverModelsResponse,
    parseEmptyResponse,
    parseErrorResponse,
    parseListModelsResponse,
    parseListProvidersResponse,
    parseModelEnvelope,
    parseProviderEnvelope,
    parseReauthorizeProviderResponse,
    parseStartProviderOauthResponse,
} from "/src/management-api/validation";

/**
 * The management API as the application addresses it: one method per route of
 * the generated route table, every method a `POST` to
 * `{baseUrl}/management-api/v0{path}` (README §3.2, §4.2). Each method rejects
 * with a `ManagementApiError` whose category and errno drive the caller's
 * reaction, and an `AbortSignal` cancels the call at the transport.
 */ export type ManagementApi = {
    readonly listProviders: (signal?: AbortSignal) => Promise<ListProvidersResponse>;
    readonly createProvider: (
        request: CreateProviderRequest,
        signal?: AbortSignal,
    ) => Promise<ProviderEnvelope>;
    readonly updateProvider: (
        request: UpdateProviderRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly deleteProvider: (
        request: DeleteProviderRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly startProviderOauth: (
        request: StartProviderOauthRequest,
        signal?: AbortSignal,
    ) => Promise<StartProviderOauthResponse>;
    readonly completeProviderOauth: (
        request: CompleteProviderOauthRequest,
        signal?: AbortSignal,
    ) => Promise<ProviderEnvelope>;
    readonly reauthorizeProvider: (
        request: ReauthorizeProviderRequest,
        signal?: AbortSignal,
    ) => Promise<ReauthorizeProviderResponse>;
    readonly listModels: (
        request: ListModelsRequest,
        signal?: AbortSignal,
    ) => Promise<ListModelsResponse>;
    readonly discoverModels: (
        request: DiscoverModelsRequest,
        signal?: AbortSignal,
    ) => Promise<DiscoverModelsResponse>;
    readonly updateModel: (
        request: UpdateModelRequest,
        signal?: AbortSignal,
    ) => Promise<ModelEnvelope>;
};

export type ManagementApiOptions = {
    /**
     * Called when a call is answered with the version's `unauthorized` errno:
     * the stored pair is proven dead (README §3.1), and the caller continues
     * to see the failure as usual.
     */
    readonly onUnauthorized?: (notice: string) => void;
};

/** A no-argument route's empty request body (README §4.2). */
const emptyRequest: ListProvidersRequest = {};

const isJsonBody = (
    bodyText: string,
):
    | {
          readonly isJson: true;
          readonly payload: unknown;
      }
    | {
          readonly isJson: false;
      } => {
    try {
        return {
            isJson: true,
            payload: JSON.parse(bodyText),
        };
    } catch {
        return {
            isJson: false,
        };
    }
};

function decodeResponse<TValue>(payload: unknown, validate: (value: unknown) => TValue): TValue {
    try {
        return validate(payload);
    } catch (error) {
        if (error instanceof ContractViolationError) {
            throw new ManagementApiError(
                "contract",
                `Unexpected management API response: ${error.message}`,
                {
                    cause: error,
                },
            );
        }

        throw error;
    }
}

export function createManagementApi(
    connection: Connection,
    options: ManagementApiOptions = {},
): ManagementApi {
    async function post(path: Path, request: unknown, signal?: AbortSignal): Promise<unknown> {
        const url = `${connection.baseUrl}${MANAGEMENT_API_PATH}${path}`;

        let response: Response;

        try {
            response = await fetch(url, {
                method: "POST",
                headers: {
                    // The management token travels verbatim: no `Bearer` prefix
                    // (README §3.3).
                    Authorization: connection.token,
                    "Content-Type": "application/json",
                },
                body: JSON.stringify(request),
                credentials: "omit",
                signal: signal ?? null,
            });
        } catch (error) {
            if (isAbortFailure(error)) {
                throw error;
            }

            throw new ManagementApiError(
                "transport",
                `Network request failed: ${describeManagementFailure(error)}`,
                {
                    cause: error,
                },
            );
        }

        let bodyText: string;

        try {
            bodyText = await response.text();
        } catch (error) {
            if (isAbortFailure(error)) {
                throw error;
            }

            throw new ManagementApiError(
                "transport",
                `Network request failed: ${describeManagementFailure(error)}`,
                {
                    httpStatus: response.status,
                    cause: error,
                },
            );
        }

        const body = isJsonBody(bodyText);

        if (!response.ok) {
            const errorResponse = body.isJson ? parseErrorResponse(body.payload) : null;

            if (errorResponse === null) {
                // Not the management API's error domain: only the status is
                // reportable (README §4.2).
                throw new ManagementApiError("transport", `HTTP ${response.status}`, {
                    httpStatus: response.status,
                });
            }

            if (errorResponse.errno === ErrorCode.Unauthorized) {
                options.onUnauthorized?.(errorResponse.message);
            }

            throw new ManagementApiError("gateway", errorResponse.message, {
                httpStatus: response.status,
                errno: errorResponse.errno,
            });
        }

        if (!body.isJson) {
            throw new ManagementApiError("transport", `HTTP ${response.status}`, {
                httpStatus: response.status,
            });
        }

        return body.payload;
    }

    return {
        listProviders: async (signal) =>
            decodeResponse(
                await post("/providers:list", emptyRequest, signal),
                parseListProvidersResponse,
            ),
        createProvider: async (request, signal) =>
            decodeResponse(await post("/providers:create", request, signal), parseProviderEnvelope),
        updateProvider: async (request, signal) => {
            decodeResponse(await post("/providers:update", request, signal), parseEmptyResponse);
        },
        deleteProvider: async (request, signal) => {
            decodeResponse(await post("/providers:delete", request, signal), parseEmptyResponse);
        },
        startProviderOauth: async (request, signal) =>
            decodeResponse(
                await post("/oauth:start", request, signal),
                parseStartProviderOauthResponse,
            ),
        completeProviderOauth: async (request, signal) =>
            decodeResponse(await post("/oauth:complete", request, signal), parseProviderEnvelope),
        reauthorizeProvider: async (request, signal) =>
            decodeResponse(
                await post("/oauth:reauthorize", request, signal),
                parseReauthorizeProviderResponse,
            ),
        listModels: async (request, signal) =>
            decodeResponse(await post("/models:list", request, signal), parseListModelsResponse),
        discoverModels: async (request, signal) =>
            decodeResponse(
                await post("/models:discover", request, signal),
                parseDiscoverModelsResponse,
            ),
        updateModel: async (request, signal) =>
            decodeResponse(await post("/models:update", request, signal), parseModelEnvelope),
    };
}
