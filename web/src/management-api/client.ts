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
    type CompleteProviderOauthRequest,
    type CreateApiKeyRequest,
    type CreateModelRequest,
    type CreateModelRouteRequest,
    type CreateProviderDefinitionRequest,
    type CreateProviderPoolRequest,
    type CreateProviderRequest,
    type DeleteApiKeyRequest,
    type DeleteModelRequest,
    type DeleteModelRouteRequest,
    type DeleteProviderPoolRequest,
    type DeleteProviderRequest,
    type DiscoverModelsRequest,
    type EmptyResponse,
    ErrorCode,
    type GetProviderAvailabilityRequest,
    type GetProviderQuotaRequest,
    type ListApiKeysRequest,
    type ListModelRoutesRequest,
    type ListModelsRequest,
    type ListProviderDefinitionsRequest,
    type ListProviderPoolsRequest,
    type ListProvidersRequest,
    type ListUsageRequest,
    type Method,
    type Path,
    type QueryUsageRequest,
    type ReauthorizeProviderRequest,
    type ResetProviderAvailabilityRequest,
    type Request as RouteRequest,
    type Response as RouteResponse,
    type StartProviderOauthRequest,
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
 * One route's dispatch row: the client method name, how the call's request
 * becomes the wire body, and how the wire response narrows back. The
 * `satisfies` check below ties every row to the generated route table's own
 * entry for its path, so a request builder or response parser that drifts
 * from the bindings cannot compile. The rows parsed by `parseEmptyResponse`
 * are exactly the writes whose generated answer is the empty record; their
 * methods resolve as `void` below.
 */
type RouteRow<P extends Path> = {
    readonly method: string;
    readonly request: (request: RouteRequest<P, Method<P>>) => RouteRequest<P, Method<P>>;
    readonly parse: (value: unknown) => RouteResponse<P, Method<P>>;
};

/** The request builder of the routes whose wire body is the caller's request verbatim. */
function passthrough<TRequest>(request: TRequest): TRequest {
    return request;
}

/** The providers listing's empty request body (README §4.2). */
const emptyProvidersRequest: ListProvidersRequest = {};

/** The api-keys listing's empty request body (README §4.2). */
const emptyApiKeysRequest: ListApiKeysRequest = {};

/** The definitions listing's empty request body. */
const emptyDefinitionsRequest: ListProviderDefinitionsRequest = {};
const emptyPoolsRequest: ListProviderPoolsRequest = {};
const emptyRoutesRequest: ListModelRoutesRequest = {};

/**
 * The whole dispatch table: one row per route of the generated route table.
 * The routes whose callers send no arguments build their fixed empty body
 * here (README §4.2); every other route sends the caller's request verbatim;
 * the writes whose generated answer is the empty record parse through
 * `parseEmptyResponse` and resolve as `void` below.
 */
const routeTable = {
    "/provider-pools:list": {
        method: "listProviderPools",
        request: () => emptyPoolsRequest,
        parse: parseListProviderPoolsResponse,
    },
    "/provider-pools:create": {
        method: "createProviderPool",
        request: passthrough<CreateProviderPoolRequest>,
        parse: parseCreateProviderPoolResponse,
    },
    "/provider-pools:update": {
        method: "updateProviderPool",
        request: passthrough<UpdateProviderPoolRequest>,
        parse: parseEmptyResponse,
    },
    "/provider-pools:delete": {
        method: "deleteProviderPool",
        request: passthrough<DeleteProviderPoolRequest>,
        parse: parseEmptyResponse,
    },
    "/model-routes:list": {
        method: "listModelRoutes",
        request: () => emptyRoutesRequest,
        parse: parseListModelRoutesResponse,
    },
    "/model-routes:create": {
        method: "createModelRoute",
        request: passthrough<CreateModelRouteRequest>,
        parse: parseCreateModelRouteResponse,
    },
    "/model-routes:update": {
        method: "updateModelRoute",
        request: passthrough<UpdateModelRouteRequest>,
        parse: parseEmptyResponse,
    },
    "/model-routes:delete": {
        method: "deleteModelRoute",
        request: passthrough<DeleteModelRouteRequest>,
        parse: parseEmptyResponse,
    },
    "/providers:availability": {
        method: "getProviderAvailability",
        request: passthrough<GetProviderAvailabilityRequest>,
        parse: parseGetProviderAvailabilityResponse,
    },
    "/providers:reset-availability": {
        method: "resetProviderAvailability",
        request: passthrough<ResetProviderAvailabilityRequest>,
        parse: parseEmptyResponse,
    },
    "/provider-definitions:list": {
        method: "listProviderDefinitions",
        request: () => emptyDefinitionsRequest,
        parse: parseListProviderDefinitionsResponse,
    },
    "/provider-definitions:create": {
        method: "createProviderDefinition",
        request: passthrough<CreateProviderDefinitionRequest>,
        parse: parseDefinitionEnvelope,
    },
    "/provider-definitions:update": {
        method: "updateProviderDefinition",
        request: passthrough<UpdateProviderDefinitionRequest>,
        parse: parseDefinitionEnvelope,
    },
    "/providers:quota": {
        method: "getProviderQuota",
        request: passthrough<GetProviderQuotaRequest>,
        parse: parseGetProviderQuotaResponse,
    },
    "/providers:list": {
        method: "listProviders",
        request: () => emptyProvidersRequest,
        parse: parseListProvidersResponse,
    },
    "/providers:create": {
        method: "createProvider",
        request: passthrough<CreateProviderRequest>,
        parse: parseProviderEnvelope,
    },
    "/providers:update": {
        method: "updateProvider",
        request: passthrough<UpdateProviderRequest>,
        parse: parseEmptyResponse,
    },
    "/providers:delete": {
        method: "deleteProvider",
        request: passthrough<DeleteProviderRequest>,
        parse: parseEmptyResponse,
    },
    "/oauth:start": {
        method: "startProviderOauth",
        request: passthrough<StartProviderOauthRequest>,
        parse: parseStartProviderOauthResponse,
    },
    "/oauth:complete": {
        method: "completeProviderOauth",
        request: passthrough<CompleteProviderOauthRequest>,
        parse: parseCompleteProviderOauthResponse,
    },
    "/oauth:reauthorize": {
        method: "reauthorizeProvider",
        request: passthrough<ReauthorizeProviderRequest>,
        parse: parseStartProviderOauthResponse,
    },
    "/models:list": {
        method: "listModels",
        request: passthrough<ListModelsRequest>,
        parse: parseListModelsResponse,
    },
    "/models:discover": {
        method: "discoverModels",
        request: passthrough<DiscoverModelsRequest>,
        parse: parseDiscoverModelsResponse,
    },
    "/models:update": {
        method: "updateModel",
        request: passthrough<UpdateModelRequest>,
        parse: parseModelEnvelope,
    },
    "/models:create": {
        method: "createModel",
        request: passthrough<CreateModelRequest>,
        parse: parseModelEnvelope,
    },
    "/models:delete": {
        method: "deleteModel",
        request: passthrough<DeleteModelRequest>,
        parse: parseEmptyResponse,
    },
    "/usage:list": {
        method: "listUsage",
        request: passthrough<ListUsageRequest>,
        parse: parseListUsageResponse,
    },
    "/usage:query": {
        method: "queryUsage",
        request: passthrough<QueryUsageRequest>,
        parse: parseQueryUsageResponse,
    },
    "/api-keys:list": {
        method: "listApiKeys",
        request: () => emptyApiKeysRequest,
        parse: parseListApiKeysResponse,
    },
    "/api-keys:create": {
        method: "createApiKey",
        request: passthrough<CreateApiKeyRequest>,
        parse: parseApiKeyEnvelope,
    },
    "/api-keys:update": {
        method: "updateApiKey",
        request: passthrough<UpdateApiKeyRequest>,
        parse: parseEmptyResponse,
    },
    "/api-keys:delete": {
        method: "deleteApiKey",
        request: passthrough<DeleteApiKeyRequest>,
        parse: parseEmptyResponse,
    },
} as const satisfies { [P in Path]: RouteRow<P> };

type RouteTable = typeof routeTable;

/**
 * The promise one client method resolves: the route's own response, or `void`
 * for the routes whose generated answer is the empty record — the writes
 * whose answer carries no fact the row is not already refetched for
 * (README §4.5).
 */
type MethodPromise<P extends Path> = Promise<
    EmptyResponse extends RouteResponse<P, Method<P>> ? void : RouteResponse<P, Method<P>>
>;

/**
 * The management API as the application addresses it: one method per route of
 * the generated route table, derived from the dispatch table above — the
 * method names, the request payloads, and the response types are the table's
 * own, so nothing restates the bindings. Every method is a `POST` to
 * `{baseUrl}/management-api/v0{path}` (README §3.2, §4.2), rejects with a
 * `ManagementApiError` whose category and errno drive the caller's reaction,
 * and takes an `AbortSignal` that cancels the call at the transport.
 */
export type ManagementApi = {
    readonly [P in Path as RouteTable[P]["method"]]: Parameters<
        RouteTable[P]["request"]
    > extends readonly [
        infer TRequest,
    ]
        ? (request: TRequest, signal?: AbortSignal) => MethodPromise<P>
        : (signal?: AbortSignal) => MethodPromise<P>;
};

type ManagementApiOptions = {
    /**
     * Called when a call is answered with the version's `unauthorized` errno:
     * the stored pair is proven dead (README §3.1), and the caller continues
     * to see the failure as usual.
     */
    readonly onUnauthorized?: (notice: string) => void;
};

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

    /**
     * One route whose callers send no arguments: the row builds the fixed
     * empty body, and the response narrows through the row's parser.
     */
    function fixedRequestMethod<TResponse>(
        path: Path,
        route: {
            readonly request: () => unknown;
            readonly parse: (value: unknown) => TResponse;
        },
    ): (signal?: AbortSignal) => Promise<TResponse> {
        return async (signal) =>
            decodeResponse(await post(path, route.request(), signal), route.parse);
    }

    /** One route whose caller supplies the wire body verbatim. */
    function callerRequestMethod<TRequest, TResponse>(
        path: Path,
        route: {
            readonly request: (request: TRequest) => unknown;
            readonly parse: (value: unknown) => TResponse;
        },
    ): (request: TRequest, signal?: AbortSignal) => Promise<TResponse> {
        return async (request, signal) =>
            decodeResponse(await post(path, route.request(request), signal), route.parse);
    }

    /**
     * One route whose validated answer carries no fact: the empty record is
     * still parsed — a foreign body is a contract violation — then dropped,
     * so the method resolves as `void` (README §4.5).
     */
    function voidResponseMethod<TRequest>(
        path: Path,
        route: {
            readonly request: (request: TRequest) => unknown;
            readonly parse: (value: unknown) => EmptyResponse;
        },
    ): (request: TRequest, signal?: AbortSignal) => Promise<void> {
        return async (request, signal) => {
            decodeResponse(await post(path, route.request(request), signal), route.parse);
        };
    }

    return {
        listProviderPools: fixedRequestMethod(
            "/provider-pools:list",
            routeTable["/provider-pools:list"],
        ),
        createProviderPool: callerRequestMethod(
            "/provider-pools:create",
            routeTable["/provider-pools:create"],
        ),
        updateProviderPool: voidResponseMethod(
            "/provider-pools:update",
            routeTable["/provider-pools:update"],
        ),
        deleteProviderPool: voidResponseMethod(
            "/provider-pools:delete",
            routeTable["/provider-pools:delete"],
        ),
        listModelRoutes: fixedRequestMethod("/model-routes:list", routeTable["/model-routes:list"]),
        createModelRoute: callerRequestMethod(
            "/model-routes:create",
            routeTable["/model-routes:create"],
        ),
        updateModelRoute: voidResponseMethod(
            "/model-routes:update",
            routeTable["/model-routes:update"],
        ),
        deleteModelRoute: voidResponseMethod(
            "/model-routes:delete",
            routeTable["/model-routes:delete"],
        ),
        getProviderAvailability: callerRequestMethod(
            "/providers:availability",
            routeTable["/providers:availability"],
        ),
        resetProviderAvailability: voidResponseMethod(
            "/providers:reset-availability",
            routeTable["/providers:reset-availability"],
        ),
        listProviderDefinitions: fixedRequestMethod(
            "/provider-definitions:list",
            routeTable["/provider-definitions:list"],
        ),
        createProviderDefinition: callerRequestMethod(
            "/provider-definitions:create",
            routeTable["/provider-definitions:create"],
        ),
        updateProviderDefinition: callerRequestMethod(
            "/provider-definitions:update",
            routeTable["/provider-definitions:update"],
        ),
        getProviderQuota: callerRequestMethod("/providers:quota", routeTable["/providers:quota"]),
        listProviders: fixedRequestMethod("/providers:list", routeTable["/providers:list"]),
        createProvider: callerRequestMethod("/providers:create", routeTable["/providers:create"]),
        updateProvider: voidResponseMethod("/providers:update", routeTable["/providers:update"]),
        deleteProvider: voidResponseMethod("/providers:delete", routeTable["/providers:delete"]),
        startProviderOauth: callerRequestMethod("/oauth:start", routeTable["/oauth:start"]),
        completeProviderOauth: callerRequestMethod(
            "/oauth:complete",
            routeTable["/oauth:complete"],
        ),
        reauthorizeProvider: callerRequestMethod(
            "/oauth:reauthorize",
            routeTable["/oauth:reauthorize"],
        ),
        listModels: callerRequestMethod("/models:list", routeTable["/models:list"]),
        discoverModels: callerRequestMethod("/models:discover", routeTable["/models:discover"]),
        updateModel: callerRequestMethod("/models:update", routeTable["/models:update"]),
        createModel: callerRequestMethod("/models:create", routeTable["/models:create"]),
        deleteModel: voidResponseMethod("/models:delete", routeTable["/models:delete"]),
        listUsage: callerRequestMethod("/usage:list", routeTable["/usage:list"]),
        queryUsage: callerRequestMethod("/usage:query", routeTable["/usage:query"]),
        listApiKeys: fixedRequestMethod("/api-keys:list", routeTable["/api-keys:list"]),
        createApiKey: callerRequestMethod("/api-keys:create", routeTable["/api-keys:create"]),
        updateApiKey: voidResponseMethod("/api-keys:update", routeTable["/api-keys:update"]),
        deleteApiKey: voidResponseMethod("/api-keys:delete", routeTable["/api-keys:delete"]),
    };
}
