import {
    type ModelSummary,
    type OAuthFlowType,
    PROVIDER_TYPES,
    type ProviderSummary,
    type ProviderType as SharedProviderType,
    UPSTREAM_PROTOCOLS,
    type UpstreamProtocol,
} from "/common/domain";
import { isJsonObject } from "/common/validation";
import { panelConfig } from "./config";
import { isThinkingLevel, type ThinkingLevel } from "./model-helpers";

export type ProviderType = SharedProviderType;
export type OAuthType = OAuthFlowType;
export type CompatibleProtocol = UpstreamProtocol;
export type Provider = ProviderSummary;
export type Model = ModelSummary;

interface SaveCompatibleProviderInput {
    name: string;
    protocol: CompatibleProtocol;
    baseUrl: string;
    enabled: boolean;
    modelPrefix: string;
    apiKey?: string;
    type?: "openai_compatible";
}

interface SaveOAuthProviderInput {
    name: string;
    enabled: boolean;
    modelPrefix: string;
}

export interface SaveModelInput {
    id?: number;
    providerName: string;
    upstreamName: string;
    name?: string;
    enabled: boolean;
    thinkingLevels: readonly ThinkingLevel[] | null;
}

export interface OAuthCompletionInput {
    redirectUrl: string;
    name?: string;
    modelPrefix?: string;
    projectId?: string;
}

class PanelRequestError extends Error {
    public readonly status: number;

    public constructor(message: string, status: number) {
        super(message);
        this.name = "PanelRequestError";
        this.status = status;
    }
}

class PanelContractError extends Error {
    public constructor(message: string) {
        super(`Invalid panel API response: ${message}`);
        this.name = "PanelContractError";
    }
}

function readRecord(value: unknown, context: string): Record<string, unknown> {
    if (!isJsonObject(value)) throw new PanelContractError(`${context} must be an object`);
    return value;
}

function readString(record: Record<string, unknown>, key: string): string {
    const value = record[key];
    if (typeof value !== "string") throw new PanelContractError(`${key} must be a string`);
    return value;
}

function readBoolean(record: Record<string, unknown>, key: string): boolean {
    const value = record[key];
    if (typeof value !== "boolean") throw new PanelContractError(`${key} must be a boolean`);
    return value;
}

function readInteger(record: Record<string, unknown>, key: string): number {
    const value = record[key];
    if (typeof value !== "number" || !Number.isInteger(value)) {
        throw new PanelContractError(`${key} must be an integer`);
    }
    return value;
}

function readNullableString(record: Record<string, unknown>, key: string): string | null {
    const value = record[key];
    if (value === null) return null;
    if (typeof value !== "string") {
        throw new PanelContractError(`${key} must be a string or null`);
    }
    return value;
}

function readNullableInteger(record: Record<string, unknown>, key: string): number | null {
    const value = record[key];
    if (value === null) return null;
    if (typeof value !== "number" || !Number.isInteger(value)) {
        throw new PanelContractError(`${key} must be an integer or null`);
    }
    return value;
}

function readProviderType(
    record: Record<string, unknown>,
    key: "type" | "providerType" = "type",
): ProviderType {
    const value = record[key];
    for (const providerType of PROVIDER_TYPES) {
        if (value === providerType) return providerType;
    }
    throw new PanelContractError("type is not a supported provider type");
}

function readProtocol(record: Record<string, unknown>): CompatibleProtocol | null {
    const value = record["protocol"];
    if (value === null) return null;
    for (const protocol of UPSTREAM_PROTOCOLS) {
        if (value === protocol) return protocol;
    }
    throw new PanelContractError("protocol is not supported");
}

function parseProvider(value: unknown): Provider {
    const record = readRecord(value, "provider");
    const type = readProviderType(record);
    const protocol = readProtocol(record);
    const baseUrl = readNullableString(record, "baseUrl");
    const expiresAt = readNullableInteger(record, "expiresAt");
    const shared = {
        enabled: readBoolean(record, "enabled"),
        modelPrefix: readString(record, "modelPrefix"),
        name: readString(record, "name"),
    };
    if (type === "openai_compatible") {
        if (protocol === null || baseUrl === null || expiresAt !== null) {
            throw new PanelContractError("an OpenAI-compatible provider has invalid fields");
        }
        return {
            ...shared,
            baseUrl,
            expiresAt: null,
            protocol,
            type,
        };
    }
    if (protocol !== null || baseUrl !== null || expiresAt === null) {
        throw new PanelContractError("an OAuth provider has invalid fields");
    }
    return {
        ...shared,
        baseUrl: null,
        expiresAt,
        protocol: null,
        type,
    };
}

function parseProviders(value: unknown): Provider[] {
    const record = readRecord(value, "providers response");
    const providers = record["providers"];
    if (!Array.isArray(providers)) {
        throw new PanelContractError("providers must be an array");
    }
    return providers.map(parseProvider);
}

function parseThinkingLevels(value: unknown): ThinkingLevel[] | null {
    if (value === null) return null;
    if (!Array.isArray(value) || !value.every(isThinkingLevel)) {
        throw new PanelContractError("thinkingLevels must contain only canonical levels");
    }
    if (new Set(value).size !== value.length) {
        throw new PanelContractError("thinkingLevels cannot contain duplicates");
    }
    return [
        ...value,
    ];
}

function parseModel(value: unknown): Model {
    const record = readRecord(value, "model");
    return {
        id: readInteger(record, "id"),
        name: readString(record, "name"),
        providerName: readString(record, "providerName"),
        providerType: readProviderType(record, "providerType"),
        upstreamName: readString(record, "upstreamName"),
        enabled: readBoolean(record, "enabled"),
        thinkingLevels: parseThinkingLevels(record["thinkingLevels"]),
    };
}

function parseModels(value: unknown): Model[] {
    const record = readRecord(value, "models response");
    const models = record["models"];
    if (!Array.isArray(models)) throw new PanelContractError("models must be an array");
    return models.map(parseModel);
}

function parseOk(value: unknown): void {
    const record = readRecord(value, "success response");
    if (record["ok"] !== true) throw new PanelContractError("ok must be true");
}

function parseSavedProvider(value: unknown): string {
    return readString(readRecord(value, "saved provider"), "name");
}

function parseAuthorizeUrl(value: unknown): string {
    return readString(readRecord(value, "OAuth start response"), "authorizeUrl");
}

function parseSavedModel(value: unknown): {
    id: number;
    name: string;
} {
    const record = readRecord(value, "saved model");
    return {
        id: readInteger(record, "id"),
        name: readString(record, "name"),
    };
}

function parseDiscovery(value: unknown): {
    total: number;
    created: number;
    removed: number;
    skipped: number;
} {
    const record = readRecord(value, "discovery result");
    return {
        total: readInteger(record, "total"),
        created: readInteger(record, "created"),
        removed: readInteger(record, "removed"),
        skipped: readInteger(record, "skipped"),
    };
}

async function readJson(response: Response): Promise<unknown> {
    try {
        const value: unknown = await response.json();
        return value;
    } catch {
        throw new PanelRequestError(
            `Panel API returned non-JSON (HTTP ${response.status})`,
            response.status,
        );
    }
}

async function readError(response: Response): Promise<PanelRequestError> {
    let message = `Panel request failed (HTTP ${response.status})`;
    try {
        const value: unknown = await response.json();
        if (isJsonObject(value) && typeof value["error"] === "string") message = value["error"];
    } catch {
        // The status remains useful when a proxy or deployment returns a non-JSON error page.
    }
    return new PanelRequestError(message, response.status);
}

export class PanelApiClient {
    readonly #getToken: () => string | null;
    readonly #onUnauthorized: () => void;

    public constructor(getToken: () => string | null, onUnauthorized: () => void) {
        this.#getToken = getToken;
        this.#onUnauthorized = onUnauthorized;
    }

    public async probe(token: string): Promise<Provider[]> {
        return this.#requestWithToken(
            panelConfig.panelEndpoints.listProviders,
            "GET",
            token,
            parseProviders,
            false,
        );
    }

    public async listProviders(): Promise<Provider[]> {
        return this.#request(
            panelConfig.panelEndpoints.listProviders,
            "GET",
            undefined,
            parseProviders,
        );
    }

    public async saveCompatibleProvider(input: SaveCompatibleProviderInput): Promise<string> {
        return this.#request(
            panelConfig.panelEndpoints.saveProvider,
            "POST",
            input,
            parseSavedProvider,
        );
    }

    public async saveOAuthProvider(input: SaveOAuthProviderInput): Promise<string> {
        return this.#request(
            panelConfig.panelEndpoints.saveProvider,
            "POST",
            input,
            parseSavedProvider,
        );
    }

    public async deleteProvider(providerName: string): Promise<void> {
        return this.#request(
            panelConfig.panelEndpoints.deleteProvider,
            "POST",
            {
                providerName,
            },
            parseOk,
        );
    }

    public async startProviderOAuth(type: OAuthType, providerName?: string): Promise<string> {
        const body: {
            type: OAuthType;
            providerName?: string;
        } = {
            type,
        };
        if (providerName !== undefined) body.providerName = providerName;
        return this.#request(
            panelConfig.panelEndpoints.startProviderOAuth,
            "POST",
            body,
            parseAuthorizeUrl,
        );
    }

    public async completeProviderOAuth(input: OAuthCompletionInput): Promise<void> {
        return this.#request(
            panelConfig.panelEndpoints.completeProviderOAuth,
            "POST",
            input,
            parseOk,
        );
    }

    public async listModels(): Promise<Model[]> {
        return this.#request(panelConfig.panelEndpoints.listModels, "GET", undefined, parseModels);
    }

    public async saveModel(input: SaveModelInput): Promise<{
        id: number;
        name: string;
    }> {
        return this.#request(panelConfig.panelEndpoints.saveModel, "POST", input, parseSavedModel);
    }

    public async deleteModel(id: number): Promise<void> {
        return this.#request(
            panelConfig.panelEndpoints.deleteModel,
            "POST",
            {
                id,
            },
            parseOk,
        );
    }

    public async discoverModels(providerName: string): Promise<{
        total: number;
        created: number;
        removed: number;
        skipped: number;
    }> {
        return this.#request(
            panelConfig.panelEndpoints.discoverModels,
            "POST",
            {
                providerName,
            },
            parseDiscovery,
        );
    }

    async #request<T>(
        path: string,
        method: "GET" | "POST",
        body: unknown,
        parse: (value: unknown) => T,
    ): Promise<T> {
        const token = this.#getToken();
        if (token === null) {
            this.#onUnauthorized();
            throw new PanelRequestError("A panel token is required", 401);
        }
        return this.#requestWithToken(path, method, token, parse, true, body);
    }

    async #requestWithToken<T>(
        path: string,
        method: "GET" | "POST",
        token: string,
        parse: (value: unknown) => T,
        shouldHandleUnauthorized: boolean,
        body?: unknown,
    ): Promise<T> {
        const headers = new Headers({
            Authorization: `Bearer ${token}`,
        });
        const init: RequestInit = {
            method,
            headers,
            credentials: "omit",
        };
        if (method === "POST") {
            headers.set("Content-Type", "application/json");
            init.body = JSON.stringify(body);
        }

        const response = await fetch(path, init);
        if (!response.ok) {
            if (response.status === 401 && shouldHandleUnauthorized) this.#onUnauthorized();
            throw await readError(response);
        }
        return parse(await readJson(response));
    }
}

export function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : "An unexpected error occurred";
}
