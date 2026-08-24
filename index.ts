import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { gatewayErrorBody, invalidApiKeyError } from "/common/errors";
import api from "./api/app";
import { API_ROOT_PATH } from "./api/config";
import panelApi from "./panel-api/app";
import { PANEL_API_BASE_PATH } from "./panel-api/config";

const app = new Hono<{
    Bindings: Env;
}>();

app.route(API_ROOT_PATH, api);
app.route(PANEL_API_BASE_PATH, panelApi);
app.notFound(async (context) => {
    const requestUrl = new URL(context.req.url);
    if (requestUrl.pathname.startsWith(`${API_ROOT_PATH}/`)) {
        if (
            !(await verifyBearerAuthorization(
                context.req.header("Authorization") ?? null,
                context.env.API_KEY_HASH,
            ))
        ) {
            const error = invalidApiKeyError();
            return context.json(gatewayErrorBody(error), error.status);
        }
        return context.json(
            {
                error: {
                    code: "not_found",
                    message: "The requested API route was not found",
                    type: "not_found_error",
                },
            },
            404,
        );
    }
    if (requestUrl.pathname.startsWith(`${PANEL_API_BASE_PATH}/`)) {
        if (
            !(await verifyBearerAuthorization(
                context.req.header("Authorization") ?? null,
                context.env.PANEL_TOKEN_HASH,
            ))
        ) {
            return context.json(
                {
                    error: "Unauthorized",
                },
                401,
            );
        }
        if (context.req.method === "POST" && context.req.header("Origin") !== requestUrl.origin) {
            return context.json(
                {
                    error: "Forbidden",
                },
                403,
            );
        }
        return context.json(
            {
                error: "Not found",
            },
            404,
        );
    }
    return context.body(null, 404);
});

export default app;
