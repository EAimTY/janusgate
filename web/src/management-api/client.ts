import {
    parseDefinitionEnvelope,
    parseListProviderDefinitionsResponse,
} from "./definitions/payload";
import {
    ContractViolationError,
    describeManagementFailure,
    isAbortFailure,
    ManagementApiError,
    type ManagementApiErrorDetails,
    type ManagementFailureCategory,
} from "./errors";
import {
    type ApiKeyEnvelope,
    type CompleteProviderOauthRequest,
    type CompleteProviderOauthResponse,
    type CreateApiKeyRequest,
    type CreateModelRequest,
    type CreateModelRouteRequest,
    type CreateModelRouteResponse,
    type CreateProviderDefinitionRequest,
    type CreateProviderPoolRequest,
    type CreateProviderPoolResponse,
    type CreateProviderRequest,
    type DefinitionEnvelope,
    type DeleteApiKeyRequest,
    type DeleteModelRequest,
    type DeleteModelRouteRequest,
    type DeleteProviderPoolRequest,
    type DeleteProviderRequest,
    type DiscoverModelsRequest,
    type DiscoverModelsResponse,
    ErrorCode,
    type GetProviderAvailabilityRequest,
    type GetProviderAvailabilityResponse,
    type GetProviderQuotaRequest,
    type GetProviderQuotaResponse,
    type ListApiKeysRequest,
    type ListApiKeysResponse,
    type ListModelRoutesRequest,
    type ListModelRoutesResponse,
    type ListModelsRequest,
    type ListModelsResponse,
    type ListProviderDefinitionsRequest,
    type ListProviderDefinitionsResponse,
    type ListProviderPoolsRequest,
    type ListProviderPoolsResponse,
    type ListProvidersRequest,
    type ListProvidersResponse,
    type ListUsageRequest,
    type ListUsageResponse,
    type ModelEnvelope,
    type Path,
    type ProviderEnvelope,
    type QueryUsageRequest,
    type QueryUsageResponse,
    type ReauthorizeProviderRequest,
    type ResetProviderAvailabilityRequest,
    type StartProviderOauthRequest,
    type StartProviderOauthResponse,
    type UpdateApiKeyRequest,
    type UpdateModelRequest,
    type UpdateModelRouteRequest,
    type UpdateProviderDefinitionRequest,
    type UpdateProviderPoolRequest,
    type UpdateProviderRequest,
} from "./generated/v0";
import { parseApiKeyEnvelope, parseListApiKeysResponse } from "./responses/api-keys";
import { parseEmptyResponse, parseErrorResponse } from "./responses/common";
import {
    parseDiscoverModelsResponse,
    parseListModelsResponse,
    parseModelEnvelope,
} from "./responses/models";
import {
    parseCompleteProviderOauthResponse,
    parseStartProviderOauthResponse,
} from "./responses/oauth";
import { parseListProvidersResponse, parseProviderEnvelope } from "./responses/providers";
import { parseGetProviderQuotaResponse } from "./responses/quota";
import { parseListUsageResponse, parseQueryUsageResponse } from "./responses/usage";
import {
    parseCreateModelRouteResponse,
    parseCreateProviderPoolResponse,
    parseGetProviderAvailabilityResponse,
    parseListModelRoutesResponse,
    parseListProviderPoolsResponse,
} from "./routing";

/**
 * The connection facts one client instance is built from: the origin the
 * management paths compose onto and the token sent verbatim. The structural
 * shape is all the client reads — the app's connection pair satisfies it,
 * and this tier never imports the connection module, so the dependency edge
 * reads one way (connection → management-api).
 */
type ManagementConnection = {
    readonly baseUrl: string;
    readonly token: string;
};

/**
 * The management API's version segment. It is fixed by the generated bindings
 * (`./generated/v0.ts`), not by the user: the route table's keys are paths
 * within this version.
 */
const MANAGEMENT_API_PATH = "/management-api/v0";

/**
 * The management API as the application addresses it: one method per route of
 * the generated route table, every method a `POST` to
 * `{baseUrl}/management-api/v0{path}` (README §3.2, §4.2). Each method rejects
 * with a `ManagementApiError` whose category and errno drive the caller's
 * reaction, and an `AbortSignal` cancels the call at the transport.
 */
export type ManagementApi = {
    readonly listProviderPools: (signal?: AbortSignal) => Promise<ListProviderPoolsResponse>;
    readonly createProviderPool: (
        request: CreateProviderPoolRequest,
        signal?: AbortSignal,
    ) => Promise<CreateProviderPoolResponse>;
    readonly updateProviderPool: (
        request: UpdateProviderPoolRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly deleteProviderPool: (
        request: DeleteProviderPoolRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly listModelRoutes: (signal?: AbortSignal) => Promise<ListModelRoutesResponse>;
    readonly createModelRoute: (
        request: CreateModelRouteRequest,
        signal?: AbortSignal,
    ) => Promise<CreateModelRouteResponse>;
    readonly updateModelRoute: (
        request: UpdateModelRouteRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly deleteModelRoute: (
        request: DeleteModelRouteRequest,
        signal?: AbortSignal,
    ) => Promise<void>;
    readonly getProviderAvailability: (
        request: GetProviderAvailabilityRequest,
        signal?: AbortSignal,
    ) => Promise<GetProviderAvailabilityResponse>;
    readonly resetProviderAvailability: (
        request: ResetProviderAvailabilityRequest,
        signal?: AbortSignal,
    ) => Promise<void>;

    readonly listProviderDefinitions: (
        signal?: AbortSignal,
    ) => Promise<ListProviderDefinitionsResponse>;
    readonly createProviderDefinition: (
        request: CreateProviderDefinitionRequest,
        signal?: AbortSignal,
    ) => Promise<DefinitionEnvelope>;
    readonly updateProviderDefinition: (
        request: UpdateProviderDefinitionRequest,
        signal?: AbortSignal,
    ) => Promise<DefinitionEnvelope>;
    readonly getProviderQuota: (
        request: GetProviderQuotaRequest,
        signal?: AbortSignal,
    ) => Promise<GetProviderQuotaResponse>;
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
    ) => Promise<CompleteProviderOauthResponse>;
    readonly reauthorizeProvider: (
        request: ReauthorizeProviderRequest,
        signal?: AbortSignal,
    ) => Promise<StartProviderOauthResponse>;
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
    readonly createModel: (
        request: CreateModelRequest,
        signal?: AbortSignal,
    ) => Promise<ModelEnvelope>;
    readonly deleteModel: (request: DeleteModelRequest, signal?: AbortSignal) => Promise<void>;
    readonly listUsage: (
        request: ListUsageRequest,
        signal?: AbortSignal,
    ) => Promise<ListUsageResponse>;
    readonly queryUsage: (
        request: QueryUsageRequest,
        signal?: AbortSignal,
    ) => Promise<QueryUsageResponse>;
    readonly listApiKeys: (signal?: AbortSignal) => Promise<ListApiKeysResponse>;
    readonly createApiKey: (
        request: CreateApiKeyRequest,
        signal?: AbortSignal,
    ) => Promise<ApiKeyEnvelope>;
    readonly updateApiKey: (request: UpdateApiKeyRequest, signal?: AbortSignal) => Promise<void>;
    readonly deleteApiKey: (request: DeleteApiKeyRequest, signal?: AbortSignal) => Promise<void>;
};

type ManagementApiOptions = {
    /**
     * Called when a call is answered with the version's `unauthorized` errno:
     * the stored pair is proven dead (README §3.1), and the caller continues
     * to see the failure as usual.
     */
    readonly onUnauthorized?: (notice: string) => void;
};

/** The providers listing's empty request body (README §4.2). */
const emptyProvidersRequest: ListProvidersRequest = {};

/** The api-keys listing's empty request body (README §4.2). */
const emptyApiKeysRequest: ListApiKeysRequest = {};

/** The definitions listing's empty request body. */
const emptyDefinitionsRequest: ListProviderDefinitionsRequest = {};
const emptyPoolsRequest: ListProviderPoolsRequest = {};
const emptyRoutesRequest: ListModelRoutesRequest = {};

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
    connection: ManagementConnection,
    options: ManagementApiOptions = {},
): ManagementApi {
    /**
     * Raises a failure the way every `post` failure is raised: retaining the
     * address the call went to, so the diagnosis of a failure whose
     * connection is already cleared — a 401's siblings still in flight, a
     * disconnect mid-read — still names a real origin (README §6.4).
     */
    function failure(
        category: ManagementFailureCategory,
        message: string,
        details: Omit<ManagementApiErrorDetails, "baseUrl"> = {},
    ): ManagementApiError {
        return new ManagementApiError(category, message, {
            baseUrl: connection.baseUrl,
            ...details,
        });
    }

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

            throw failure(
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

            throw failure(
                "transport",
                `Network request failed: ${describeManagementFailure(error)}`,
                {
                    httpStatus: response.status,
                    cause: error,
                },
            );
        }

        signal?.throwIfAborted();
        const body = isJsonBody(bodyText);

        if (!response.ok) {
            const errorResponse = body.isJson ? parseErrorResponse(body.payload) : null;

            if (errorResponse === null) {
                // Not the management API's error domain: only the status is
                // reportable (README §4.2).
                throw failure("transport", `HTTP ${response.status}`, {
                    httpStatus: response.status,
                });
            }

            if (errorResponse.errno === ErrorCode.Unauthorized) {
                options.onUnauthorized?.(errorResponse.message);
            }

            throw failure("gateway", errorResponse.message, {
                httpStatus: response.status,
                errno: errorResponse.errno,
            });
        }

        if (!body.isJson) {
            throw failure("transport", `HTTP ${response.status}`, {
                httpStatus: response.status,
            });
        }

        return body.payload;
    }

    return {
        listProviderPools: async (signal) =>
            decodeResponse(
                await post("/provider-pools:list", emptyPoolsRequest, signal),
                parseListProviderPoolsResponse,
            ),
        createProviderPool: async (request, signal) =>
            decodeResponse(
                await post("/provider-pools:create", request, signal),
                parseCreateProviderPoolResponse,
            ),
        updateProviderPool: async (request, signal) => {
            decodeResponse(
                await post("/provider-pools:update", request, signal),
                parseEmptyResponse,
            );
        },
        deleteProviderPool: async (request, signal) => {
            decodeResponse(
                await post("/provider-pools:delete", request, signal),
                parseEmptyResponse,
            );
        },
        listModelRoutes: async (signal) =>
            decodeResponse(
                await post("/model-routes:list", emptyRoutesRequest, signal),
                parseListModelRoutesResponse,
            ),
        createModelRoute: async (request, signal) =>
            decodeResponse(
                await post("/model-routes:create", request, signal),
                parseCreateModelRouteResponse,
            ),
        updateModelRoute: async (request, signal) => {
            decodeResponse(await post("/model-routes:update", request, signal), parseEmptyResponse);
        },
        deleteModelRoute: async (request, signal) => {
            decodeResponse(await post("/model-routes:delete", request, signal), parseEmptyResponse);
        },
        getProviderAvailability: async (request, signal) =>
            decodeResponse(
                await post("/providers:availability", request, signal),
                parseGetProviderAvailabilityResponse,
            ),
        resetProviderAvailability: async (request, signal) => {
            decodeResponse(
                await post("/providers:reset-availability", request, signal),
                parseEmptyResponse,
            );
        },
        listProviderDefinitions: async (signal) =>
            decodeResponse(
                await post("/provider-definitions:list", emptyDefinitionsRequest, signal),
                parseListProviderDefinitionsResponse,
            ),
        createProviderDefinition: async (request, signal) =>
            decodeResponse(
                await post("/provider-definitions:create", request, signal),
                parseDefinitionEnvelope,
            ),
        updateProviderDefinition: async (request, signal) =>
            decodeResponse(
                await post("/provider-definitions:update", request, signal),
                parseDefinitionEnvelope,
            ),
        getProviderQuota: async (request, signal) =>
            decodeResponse(
                await post("/providers:quota", request, signal),
                parseGetProviderQuotaResponse,
            ),
        listProviders: async (signal) =>
            decodeResponse(
                await post("/providers:list", emptyProvidersRequest, signal),
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
            decodeResponse(
                await post("/oauth:complete", request, signal),
                parseCompleteProviderOauthResponse,
            ),
        reauthorizeProvider: async (request, signal) =>
            decodeResponse(
                await post("/oauth:reauthorize", request, signal),
                parseStartProviderOauthResponse,
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
        createModel: async (request, signal) =>
            decodeResponse(await post("/models:create", request, signal), parseModelEnvelope),
        deleteModel: async (request, signal) => {
            decodeResponse(await post("/models:delete", request, signal), parseEmptyResponse);
        },
        listUsage: async (request, signal) =>
            decodeResponse(await post("/usage:list", request, signal), parseListUsageResponse),
        queryUsage: async (request, signal) =>
            decodeResponse(await post("/usage:query", request, signal), parseQueryUsageResponse),
        listApiKeys: async (signal) =>
            decodeResponse(
                await post("/api-keys:list", emptyApiKeysRequest, signal),
                parseListApiKeysResponse,
            ),
        createApiKey: async (request, signal) =>
            decodeResponse(await post("/api-keys:create", request, signal), parseApiKeyEnvelope),
        updateApiKey: async (request, signal) => {
            decodeResponse(await post("/api-keys:update", request, signal), parseEmptyResponse);
        },
        deleteApiKey: async (request, signal) => {
            decodeResponse(await post("/api-keys:delete", request, signal), parseEmptyResponse);
        },
    };
}
