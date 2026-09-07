import { Hono, type MiddlewareHandler } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { API_VERSION_PATH } from "./config";
import { gatewayErrorBody, invalidApiKeyError } from "./errors";
import { handleGatewayRequest, respondGatewayError } from "./gateway";
import { listEnabledPublicModels } from "./model-routing";

/** Shared gateway 401 envelope: an invalid bearer token always answers with this response. */
export function gatewayUnauthorizedResponse(): Response {
    const error = invalidApiKeyError();
    return Response.json(gatewayErrorBody(error), {
        status: error.status,
    });
}

/**
 * The single app-level enforcement point for gateway bearer auth (README §4.1). It runs before
 * every route handler on this app, so no route can ship unauthenticated by omission. Unknown
 * gateway paths never reach a route handler; the root not-found fallback enforces the same
 * check there.
 */
const requireGatewayAuthorization: MiddlewareHandler<{
    Bindings: Env;
}> = async (context, next) => {
    const isAuthorized = await verifyBearerAuthorization(
        context.req.header("Authorization") ?? null,
        context.env.API_KEY_HASH,
    );
    if (!isAuthorized) return gatewayUnauthorizedResponse();
    return next();
};

const api = new Hono<{
    Bindings: Env;
}>();

api.use(`${API_VERSION_PATH}/*`, requireGatewayAuthorization);

api.get(`${API_VERSION_PATH}/models`, async (context) => {
    try {
        return context.json(await listEnabledPublicModels(context.env.DATABASE));
    } catch (error) {
        return respondGatewayError(error);
    }
});

api.post(`${API_VERSION_PATH}/responses`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "openai_response"),
);

api.post(`${API_VERSION_PATH}/chat/completions`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "openai_chat_completion"),
);

export default api;
