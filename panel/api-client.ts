import {
    assembleProviderSummary,
    type ModelSummary,
    type OAuthFlowType,
    type ProviderSummary,
    type ProviderType,
    type UpstreamProtocol,
} from "/common/domain";
import {
    isJsonObject,
    parseProviderType,
    parseThinkingLevels,
    parseUpstreamProtocol,
} from "/common/validation";
import type { ThinkingLevel } from "/config";
import type { DiscoveryResult } from "/panel-api/domain";
import type { CompleteOAuthInput, SaveModelInput, StartOAuthInput } from "/panel-api/input";
import { panelConfig } from "./config";

interface SaveCompatibleProviderInput {
    name: string;
    protocol: UpstreamProtocol;
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
    try {
        return parseProviderType(record[key]);
    } catch {
        throw new PanelContractError(`${key} is not a supported provider type`);
    }
}

function readProtocol(record: Record<string, unknown>): UpstreamProtocol | null {
    if (record["protocol"] === null) return null;
    try {
        return parseUpstreamProtocol(record["protocol"]);
    } catch {
        throw new PanelContractError("protocol is not supported");
    }
}

function readThinkingLevels(value: unknown): readonly ThinkingLevel[] | null {
    try {
        // The shared parser is normative: it rejects duplicates and returns the canonical order.
        return parseThinkingLevels(value);
    } catch {
        throw new PanelContractError("thinkingLevels must contain only canonical levels");
    }
}

function parseProvider(value: unknown): ProviderSummary {
    const record = readRecord(value, "provider");
    try {
        // The discriminated-union invariant is assembled by the one shared enforcement point,
        // so the SPA and the repository cannot disagree about what a valid provider is.
        return assembleProviderSummary({
            baseUrl: readNullableString(record, "baseUrl"),
            expiresAt: readNullableInteger(record, "expiresAt"),
            isEnabled: readBoolean(record, "enabled"),
            modelPrefix: readString(record, "modelPrefix"),
            name: readString(record, "name"),
            protocol: readProtocol(record),
            type: readProviderType(record),
        });
    } catch (error) {
        if (error instanceof PanelContractError) throw error;
        throw new PanelContractError("provider fields contradict the provider type");
    }
}

function parseProviders(value: unknown): ProviderSummary[] {
    const record = readRecord(value, "providers response");
    const providers = record["providers"];
    if (!Array.isArray(providers)) {
        throw new PanelContractError("providers must be an array");
    }
    return providers.map(parseProvider);
}

function parseModel(value: unknown): ModelSummary {
    const record = readRecord(value, "model");
    return {
        enabled: readBoolean(record, "enabled"),
        name: readString(record, "name"),
        nameOverride: readNullableString(record, "nameOverride"),
        providerName: readString(record, "providerName"),
        providerType: readProviderType(record, "providerType"),
        thinkingLevels: readThinkingLevels(record["thinkingLevels"]),
        upstreamName: readString(record, "upstreamName"),
    };
}

function parseModels(value: unknown): ModelSummary[] {
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
    name: string;
} {
    return {
        name: readString(readRecord(value, "saved model"), "name"),
    };
}

function parseDiscovery(value: unknown): DiscoveryResult {
    const record = readRecord(value, "discovery result");
    return {
        created: readInteger(record, "created"),
        removed: readInteger(record, "removed"),
        skipped: readInteger(record, "skipped"),
        total: readInteger(record, "total"),
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

    public async probe(token: string): Promise<ProviderSummary[]> {
        return this.#requestWithToken(
            panelConfig.panelEndpoints.listProviders,
            "GET",
            token,
            parseProviders,
            false,
        );
    }

    public async listProviders(): Promise<ProviderSummary[]> {
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

    public async startProviderOAuth(type: OAuthFlowType, providerName?: string): Promise<string> {
        const body: StartOAuthInput =
            providerName === undefined
                ? {
                      type,
                  }
                : {
                      type,
                      providerName,
                  };
        return this.#request(
            panelConfig.panelEndpoints.startProviderOAuth,
            "POST",
            body,
            parseAuthorizeUrl,
        );
    }

    public async completeProviderOAuth(input: CompleteOAuthInput): Promise<void> {
        return this.#request(
            panelConfig.panelEndpoints.completeProviderOAuth,
            "POST",
            input,
            parseOk,
        );
    }

    public async listModels(): Promise<ModelSummary[]> {
        return this.#request(panelConfig.panelEndpoints.listModels, "GET", undefined, parseModels);
    }

    public async saveModel(input: SaveModelInput): Promise<{
        name: string;
    }> {
        return this.#request(panelConfig.panelEndpoints.saveModel, "POST", input, parseSavedModel);
    }

    public async deleteModel(providerName: string, upstreamName: string): Promise<void> {
        return this.#request(
            panelConfig.panelEndpoints.deleteModel,
            "POST",
            {
                providerName,
                upstreamName,
            },
            parseOk,
        );
    }

    public async discoverModels(providerName: string): Promise<DiscoveryResult> {
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
