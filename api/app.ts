import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { gatewayErrorBody, invalidApiKeyError } from "/common/errors";
import { API_VERSION_PATH } from "./config";
import { handleGatewayRequest } from "./gateway";
import { listEnabledPublicModels } from "./model-routing";

/** Shared gateway 401 envelope: an invalid bearer token always answers with this response. */
export function gatewayUnauthorizedResponse(): Response {
    const error = invalidApiKeyError();
    return Response.json(gatewayErrorBody(error), {
        status: error.status,
    });
}

export async function isGatewayAuthorized(request: Request, apiKeyHash: string): Promise<boolean> {
    return verifyBearerAuthorization(request.headers.get("Authorization"), apiKeyHash);
}

const api = new Hono<{
    Bindings: Env;
}>();

api.get(`${API_VERSION_PATH}/models`, async (context) => {
    if (!(await isGatewayAuthorized(context.req.raw, context.env.API_KEY_HASH))) {
        return gatewayUnauthorizedResponse();
    }
    return context.json(await listEnabledPublicModels(context.env.DATABASE));
});

api.post(`${API_VERSION_PATH}/responses`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "openai_response"),
);

api.post(`${API_VERSION_PATH}/chat/completions`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "openai_chat_completion"),
);

export default api;
