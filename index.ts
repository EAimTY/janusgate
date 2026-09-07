import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import api, { gatewayUnauthorizedResponse } from "./api/app";
import { API_ROOT_PATH } from "./api/config";
import { GatewayError, gatewayErrorBody } from "./api/errors";
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
        // Unmatched gateway paths never reach the API middleware, so this fallback enforces the
        // same bearer check itself.
        const isAuthorized = await verifyBearerAuthorization(
            context.req.raw.headers.get("Authorization"),
            context.env.API_KEY_HASH,
        );
        if (!isAuthorized) {
            return gatewayUnauthorizedResponse();
        }
        return context.json(
            gatewayErrorBody(
                new GatewayError(
                    404,
                    "not_found_error",
                    "The requested API route was not found",
                    "not_found",
                ),
            ),
            404,
        );
    }
    // Unknown /panel-api/* routes only reach this handler after the panel middleware already
    // enforced bearer auth and POST origin, so no re-verification is needed here.
    if (requestUrl.pathname.startsWith(`${PANEL_API_BASE_PATH}/`)) {
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
