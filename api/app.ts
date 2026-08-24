import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { gatewayErrorBody, invalidApiKeyError } from "/common/errors";
import { API_VERSION_PATH } from "./config";
import { handleGatewayRequest } from "./gateway";
import { listEnabledPublicModels } from "./model-routing";

const api = new Hono<{
    Bindings: Env;
}>();

api.get(`${API_VERSION_PATH}/models`, async (context) => {
    if (
        !(await verifyBearerAuthorization(
            context.req.header("Authorization") ?? null,
            context.env.API_KEY_HASH,
        ))
    ) {
        const error = invalidApiKeyError();
        return context.json(gatewayErrorBody(error), error.status);
    }
    return context.json(await listEnabledPublicModels(context.env.DATABASE));
});

api.post(`${API_VERSION_PATH}/responses`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "responses"),
);

api.post(`${API_VERSION_PATH}/chat/completions`, (context) =>
    handleGatewayRequest(context.req.raw, context.env, "chat_completions"),
);

export default api;
