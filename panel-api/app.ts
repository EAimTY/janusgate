import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { CredentialsCipher } from "/common/credentials";
import { ConfigurationError, ValidationError } from "/common/errors";
import { isJsonObject } from "/common/validation";
import { discoverProviderModels } from "./discovery";
import { PanelApiError } from "./errors";
import {
    parseCompleteOAuthInput,
    parseDeleteModelInput,
    parseDeleteProviderInput,
    parseDiscoverModelsInput,
    parseSaveModelInput,
    parseSaveProviderInput,
    parseStartOAuthInput,
} from "./input";
import { deleteModel, listPanelModels, upsertModel } from "./model-repository";
import { completeProviderOAuth, startProviderOAuth } from "./oauth";
import {
    deleteProviderByName,
    listProviderSummaries,
    requireProviderByName,
    saveProvider,
} from "./provider-repository";

interface PanelApiOptions {
    readonly fetchUpstream?: typeof fetch;
}

type PanelApp = Hono<{
    Bindings: Env;
}>;

async function readRequestObject(request: Request): Promise<Record<string, unknown>> {
    let value: unknown;
    try {
        value = await request.json();
    } catch {
        throw new PanelApiError(400, "Request body must be a JSON object");
    }
    if (!isJsonObject(value)) {
        throw new PanelApiError(400, "Request body must be a JSON object");
    }
    return value;
}

async function savePanelModel(
    database: D1Database,
    body: Record<string, unknown>,
): Promise<{
    name: string;
}> {
    const input = parseSaveModelInput(body);
    const provider = await requireProviderByName(database, input.providerName);
    const name = await upsertModel(database, {
        enabledOverride: input.enabledOverride,
        modelPrefix: provider.modelPrefix,
        nameOverride: input.nameOverride,
        providerId: provider.id,
        thinkingLevelsOverride: input.thinkingLevelsOverride,
        upstreamName: input.upstreamName,
    });
    return {
        name,
    };
}

export function createPanelApi(options: PanelApiOptions = {}): PanelApp {
    const panelApi: PanelApp = new Hono<{
        Bindings: Env;
    }>();
    const fetchUpstream = options.fetchUpstream ?? fetch;

    panelApi.use("*", async (context, next) => {
        const isAuthenticated = await verifyBearerAuthorization(
            context.req.header("Authorization") ?? null,
            context.env.PANEL_TOKEN_HASH,
        );
        if (!isAuthenticated)
            return context.json(
                {
                    error: "Unauthorized",
                },
                401,
            );

        if (context.req.method === "POST") {
            const requestOrigin = new URL(context.req.url).origin;
            if (context.req.header("Origin") !== requestOrigin) {
                return context.json(
                    {
                        error: "Forbidden",
                    },
                    403,
                );
            }
        }
        return next();
    });

    panelApi.get("/list-providers", async (context) => {
        return context.json({
            providers: await listProviderSummaries(context.env.DATABASE),
        });
    });

    panelApi.post("/save-provider", async (context) => {
        const input = parseSaveProviderInput(await readRequestObject(context.req.raw));
        const cipher = await CredentialsCipher.create(context.env.CREDENTIALS_AES_KEY);
        return context.json(await saveProvider(context.env.DATABASE, cipher, input));
    });

    panelApi.post("/delete-provider", async (context) => {
        const input = parseDeleteProviderInput(await readRequestObject(context.req.raw));
        await deleteProviderByName(context.env.DATABASE, input.providerName);
        return context.json({
            ok: true,
        });
    });

    panelApi.post("/start-provider-oauth", async (context) => {
        const input = parseStartOAuthInput(await readRequestObject(context.req.raw));
        const authorizeUrl = await startProviderOAuth(context.env.DATABASE, input);
        return context.json({
            authorizeUrl,
        });
    });

    panelApi.post("/complete-provider-oauth", async (context) => {
        const input = parseCompleteOAuthInput(await readRequestObject(context.req.raw));
        const cipher = await CredentialsCipher.create(context.env.CREDENTIALS_AES_KEY);
        await completeProviderOAuth(context.env.DATABASE, cipher, input, {
            fetchUpstream,
            signal: context.req.raw.signal,
        });
        return context.json({
            ok: true,
        });
    });

    panelApi.get("/list-models", async (context) => {
        return context.json({
            models: await listPanelModels(context.env.DATABASE),
        });
    });

    panelApi.post("/save-model", async (context) => {
        return context.json(
            await savePanelModel(context.env.DATABASE, await readRequestObject(context.req.raw)),
        );
    });

    panelApi.post("/delete-model", async (context) => {
        const input = parseDeleteModelInput(await readRequestObject(context.req.raw));
        const provider = await requireProviderByName(context.env.DATABASE, input.providerName);
        await deleteModel(context.env.DATABASE, provider.id, input.upstreamName);
        return context.json({
            ok: true,
        });
    });

    panelApi.post("/discover-models", async (context) => {
        const input = parseDiscoverModelsInput(await readRequestObject(context.req.raw));
        const provider = await requireProviderByName(context.env.DATABASE, input.providerName);
        const cipher = await CredentialsCipher.create(context.env.CREDENTIALS_AES_KEY);
        const result = await discoverProviderModels(
            context.env.DATABASE,
            provider,
            cipher,
            fetchUpstream,
            context.req.raw.signal,
        );
        return context.json(result);
    });

    panelApi.onError((error, context) => {
        if (error instanceof ConfigurationError) throw error;
        if (error instanceof PanelApiError) {
            return context.json(
                {
                    error: error.message,
                },
                error.status,
            );
        }
        if (error instanceof ValidationError) {
            return context.json(
                {
                    error: error.message,
                },
                400,
            );
        }
        return context.json(
            {
                error: "An internal error occurred",
            },
            500,
        );
    });

    return panelApi;
}

const panelApi = createPanelApi();

export default panelApi;
