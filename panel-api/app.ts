import { Hono } from "hono";
import { verifyBearerAuthorization } from "/common/auth";
import { CredentialsCipher } from "/common/credentials";
import type { StoredProvider } from "/common/database";
import type { ProviderSummary, UpstreamProtocol } from "/common/domain";
import { ConfigurationError, ValidationError } from "/common/errors";
import { composeModelName, isJsonObject } from "/common/validation";
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
import { deleteModel, findModelById, listPanelModels, saveModel } from "./model-repository";
import { completeProviderOAuth, startProviderOAuth } from "./oauth";
import {
    createProvider,
    deleteProviderByName,
    findProviderByName,
    listProviders,
    requireProviderByName,
    updateProvider,
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

function arrayBufferCopy(bytes: Uint8Array): ArrayBuffer {
    const copy = new Uint8Array(bytes.byteLength);
    copy.set(bytes);
    return copy.buffer;
}

function panelProvider(provider: StoredProvider): ProviderSummary {
    const shared = {
        enabled: provider.isEnabled,
        modelPrefix: provider.modelPrefix,
        name: provider.name,
    };
    if (provider.type === "openai_compatible") {
        if (
            provider.protocol === null ||
            provider.baseUrl === null ||
            provider.expiresAt !== null
        ) {
            throw new Error("Invalid stored OpenAI-compatible provider");
        }
        return {
            ...shared,
            baseUrl: provider.baseUrl,
            expiresAt: null,
            protocol: provider.protocol,
            type: provider.type,
        };
    }
    if (provider.protocol !== null || provider.baseUrl !== null || provider.expiresAt === null) {
        throw new Error("Invalid stored OAuth provider");
    }
    return {
        ...shared,
        baseUrl: null,
        expiresAt: provider.expiresAt,
        protocol: null,
        type: provider.type,
    };
}

function assertCompatibleCreationFields(input: {
    protocol?: UpstreamProtocol;
    baseUrl?: string;
    apiKey?: string;
}): asserts input is {
    protocol: UpstreamProtocol;
    baseUrl: string;
    apiKey: string;
} {
    if (input.protocol === undefined || input.baseUrl === undefined || input.apiKey === undefined) {
        throw new PanelApiError(
            400,
            "Creating an OpenAI-compatible provider requires protocol, baseUrl, and apiKey",
        );
    }
}

async function savePanelProvider(
    environment: Env,
    body: Record<string, unknown>,
): Promise<{
    name: string;
}> {
    const input = parseSaveProviderInput(body);
    const existing = await findProviderByName(environment.DATABASE, input.name);
    const cipher = await CredentialsCipher.create(environment.CREDENTIALS_AES_KEY);

    if (existing === null) {
        if (input.type !== "openai_compatible") {
            throw new PanelApiError(404, "Provider not found");
        }
        assertCompatibleCreationFields(input);
        const encryptedCredentials = await cipher.encrypt({
            apiKey: input.apiKey,
        });
        const created = await createProvider(environment.DATABASE, {
            baseUrl: input.baseUrl,
            encryptedCredentials: arrayBufferCopy(encryptedCredentials),
            expiresAt: null,
            isEnabled: input.enabled ?? true,
            modelPrefix: input.modelPrefix ?? input.name,
            name: input.name,
            protocol: input.protocol,
            type: "openai_compatible",
        });
        return {
            name: created.name,
        };
    }

    if (input.type !== undefined) {
        throw new PanelApiError(409, "Provider name already exists");
    }

    if (existing.type === "openai_compatible") {
        if (existing.baseUrl === null || existing.protocol === null) {
            throw new Error("Invalid stored OpenAI-compatible provider");
        }
        const encryptedCredentials =
            input.apiKey === undefined
                ? undefined
                : arrayBufferCopy(
                      await cipher.encrypt({
                          apiKey: input.apiKey,
                      }),
                  );
        await updateProvider(environment.DATABASE, {
            baseUrl: input.baseUrl ?? existing.baseUrl,
            ...(encryptedCredentials === undefined
                ? {}
                : {
                      encryptedCredentials,
                  }),
            expiresAt: null,
            id: existing.id,
            isEnabled: input.enabled ?? existing.isEnabled,
            modelPrefix: input.modelPrefix ?? existing.modelPrefix,
            protocol: input.protocol ?? existing.protocol,
        });
        return {
            name: existing.name,
        };
    }

    if (input.protocol !== undefined || input.baseUrl !== undefined || input.apiKey !== undefined) {
        throw new PanelApiError(400, "OAuth providers only accept enabled and modelPrefix updates");
    }
    await updateProvider(environment.DATABASE, {
        baseUrl: null,
        expiresAt: existing.expiresAt,
        id: existing.id,
        isEnabled: input.enabled ?? existing.isEnabled,
        modelPrefix: input.modelPrefix ?? existing.modelPrefix,
        protocol: null,
    });
    return {
        name: existing.name,
    };
}

async function savePanelModel(
    database: D1Database,
    body: Record<string, unknown>,
): Promise<{
    id: number;
    name: string;
}> {
    const input = parseSaveModelInput(body);
    const provider = await requireProviderByName(database, input.providerName);
    const existing = input.id === undefined ? null : await findModelById(database, input.id);
    if (input.id !== undefined && existing === null) {
        throw new PanelApiError(404, "Model not found");
    }

    const publicName =
        existing !== null && input.name === undefined
            ? existing.name
            : composeModelName(provider.modelPrefix, input.name ?? input.upstreamName);
    return saveModel(database, {
        ...(input.id === undefined
            ? {}
            : {
                  id: input.id,
              }),
        isEnabled: input.enabled ?? existing?.isEnabled ?? true,
        name: publicName,
        providerId: provider.id,
        thinkingLevels: input.thinkingLevels ?? null,
        upstreamName: input.upstreamName,
    });
}

export function createPanelApi(options: PanelApiOptions = {}): PanelApp {
    const panelApi: PanelApp = new Hono<{
        Bindings: Env;
    }>();

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
        const providers = await listProviders(context.env.DATABASE);
        return context.json({
            providers: providers.map(panelProvider),
        });
    });

    panelApi.post("/save-provider", async (context) => {
        return context.json(
            await savePanelProvider(context.env, await readRequestObject(context.req.raw)),
        );
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
            ...(options.fetchUpstream === undefined
                ? {}
                : {
                      fetchUpstream: options.fetchUpstream,
                  }),
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
        await deleteModel(context.env.DATABASE, input.id);
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
            options.fetchUpstream,
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
