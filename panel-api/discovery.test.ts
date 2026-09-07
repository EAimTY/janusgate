import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { CredentialsCipher } from "/common/credentials";
import type { ProviderCredentials, ProviderType } from "/common/domain";
import { ProviderAccessError } from "/common/errors";
import {
    ANTIGRAVITY_FETCH_MODELS_URL,
    ANTIGRAVITY_HEADER_CLIENT_METADATA,
    ANTIGRAVITY_TOKEN_URL,
} from "/providers/antigravity";
import { OPENAI_CODEX_MODELS } from "/providers/openai";
import {
    clearTestDatabase,
    initializeTestDatabase,
    seedProvider,
    TEST_CREDENTIALS_AES_KEY,
} from "/tests/database";
import { discoverProviderModels } from "./discovery";
import { findProviderByName } from "./provider-repository";

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function insertProvider(
    name: string,
    type: ProviderType,
    credentials: ProviderCredentials | Uint8Array,
): Promise<void> {
    await seedProvider(env.DATABASE, {
        credentials,
        modelPrefix: `${name}/`,
        name,
        type,
    });
}

async function insertModel(
    providerName: string,
    upstreamName: string,
    name: string,
    overrides: {
        nameOverride?: string;
        enabledOverride?: number;
    } = {},
): Promise<void> {
    await env.DATABASE.prepare(
        `INSERT INTO models (
            provider_id, upstream_name, name, name_override, enabled_override,
            thinking_levels_override
        ) VALUES (
            (SELECT id FROM providers WHERE name = ?), ?, ?, ?, ?, NULL
        )`,
    )
        .bind(
            providerName,
            upstreamName,
            name,
            overrides.nameOverride ?? null,
            overrides.enabledOverride ?? null,
        )
        .run();
}

function compatibleFetchUpstream(modelIds: readonly string[]): typeof fetch {
    return async () =>
        Response.json({
            data: modelIds.map((id) => ({
                id,
            })),
        });
}

describe("model discovery", () => {
    it("authenticates compatible discovery and atomically creates enabled unoverridden mappings", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        expect(provider).not.toBeNull();
        if (provider === null) throw new Error("provider missing");

        const fetchUpstream = vi.fn<typeof fetch>(async () =>
            Response.json({
                data: [
                    {
                        id: "alpha",
                    },
                    {
                        id: "beta",
                    },
                ],
            }),
        );
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: 2,
            created: 2,
            removed: 0,
            skipped: 0,
        });
        expect(fetchUpstream).toHaveBeenCalledWith(
            "https://upstream.test/v1/models",
            expect.objectContaining({
                headers: {
                    Authorization: "Bearer key",
                },
                method: "GET",
                signal: expect.anything(),
            }),
        );

        const rows = await env.DATABASE.prepare(
            `SELECT provider_id, upstream_name, name, name_override, enabled_override,
                thinking_levels_override
            FROM models ORDER BY upstream_name`,
        ).all<{
            provider_id: number;
            upstream_name: string;
            name: string;
            name_override: string | null;
            enabled_override: number | null;
            thinking_levels_override: string | null;
        }>();
        expect(rows.results).toEqual([
            {
                enabled_override: null,
                name: "custom/alpha",
                name_override: null,
                provider_id: provider.id,
                thinking_levels_override: null,
                upstream_name: "alpha",
            },
            {
                enabled_override: null,
                name: "custom/beta",
                name_override: null,
                provider_id: provider.id,
                thinking_levels_override: null,
                upstream_name: "beta",
            },
        ]);

        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: 2,
            created: 0,
            removed: 0,
            skipped: 2,
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM models").first("count"),
        ).resolves.toBe(2);
    });

    it("reconciles by identity: prunes unoverridden stale rows and keeps overridden rows", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");

        await discoverProviderModels(
            env.DATABASE,
            provider,
            cipher,
            compatibleFetchUpstream([
                "alpha",
                "beta",
                "gamma",
            ]),
        );
        // alpha is renamed and beta is disabled; both overrides shield the rows from pruning.
        await env.DATABASE.prepare(
            `UPDATE models SET name = 'custom/renamed-alpha', name_override = 'renamed-alpha'
            WHERE provider_id = ? AND upstream_name = 'alpha'`,
        )
            .bind(provider.id)
            .run();
        await env.DATABASE.prepare(
            "UPDATE models SET enabled_override = 0 WHERE provider_id = ? AND upstream_name = 'beta'",
        )
            .bind(provider.id)
            .run();

        await expect(
            discoverProviderModels(
                env.DATABASE,
                provider,
                cipher,
                compatibleFetchUpstream([
                    "alpha",
                    "beta",
                    "delta",
                ]),
            ),
        ).resolves.toEqual({
            total: 3,
            created: 1,
            removed: 1,
            skipped: 2,
        });
        const rows = await env.DATABASE.prepare(
            `SELECT upstream_name, name, name_override, enabled_override
            FROM models WHERE provider_id = ? ORDER BY upstream_name`,
        )
            .bind(provider.id)
            .all<{
                upstream_name: string;
                name: string;
                name_override: string | null;
                enabled_override: number | null;
            }>();
        expect(rows.results).toEqual([
            {
                enabled_override: null,
                name: "custom/renamed-alpha",
                name_override: "renamed-alpha",
                upstream_name: "alpha",
            },
            {
                enabled_override: 0,
                name: "custom/beta",
                name_override: null,
                upstream_name: "beta",
            },
            {
                enabled_override: null,
                name: "custom/delta",
                name_override: null,
                upstream_name: "delta",
            },
        ]);

        // Only overridden rows survive the disappearance of their upstream models.
        await expect(
            discoverProviderModels(
                env.DATABASE,
                provider,
                cipher,
                compatibleFetchUpstream([
                    "delta",
                ]),
            ),
        ).resolves.toEqual({
            total: 1,
            created: 0,
            removed: 0,
            skipped: 1,
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM models").first("count"),
        ).resolves.toBe(3);
    });

    it("re-derives unoverridden names from the current prefix and skips occupied names", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");

        await discoverProviderModels(
            env.DATABASE,
            provider,
            cipher,
            compatibleFetchUpstream([
                "alpha",
                "beta",
            ]),
        );
        // Another provider's mapping occupies the name the new prefix would derive for alpha.
        await insertProvider("blocker", "openai_compatible", {
            apiKey: "key",
        });
        await insertModel("blocker", "unrelated", "x/alpha");
        await env.DATABASE.prepare(
            "UPDATE providers SET model_prefix = 'x/' WHERE name = 'custom'",
        ).run();
        const rederivedProvider = await findProviderByName(env.DATABASE, "custom");
        if (rederivedProvider === null) throw new Error("provider missing");

        await expect(
            discoverProviderModels(
                env.DATABASE,
                rederivedProvider,
                cipher,
                compatibleFetchUpstream([
                    "alpha",
                    "beta",
                ]),
            ),
        ).resolves.toEqual({
            total: 2,
            created: 0,
            removed: 0,
            skipped: 2,
        });
        const rows = await env.DATABASE.prepare(
            "SELECT upstream_name, name FROM models WHERE provider_id = ? ORDER BY upstream_name",
        )
            .bind(provider.id)
            .all<{
                upstream_name: string;
                name: string;
            }>();
        // The occupied rename is skipped; the free one propagates.
        expect(rows.results).toEqual([
            {
                name: "custom/alpha",
                upstream_name: "alpha",
            },
            {
                name: "x/beta",
                upstream_name: "beta",
            },
        ]);
    });

    it("skips creating a model whose derived name is already occupied", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");
        await insertProvider("blocker", "openai_compatible", {
            apiKey: "key",
        });
        await insertModel("blocker", "unrelated", "custom/alpha");

        await expect(
            discoverProviderModels(
                env.DATABASE,
                provider,
                cipher,
                compatibleFetchUpstream([
                    "alpha",
                    "beta",
                ]),
            ),
        ).resolves.toEqual({
            total: 2,
            created: 1,
            removed: 0,
            skipped: 1,
        });
        const names = await env.DATABASE.prepare("SELECT name FROM models ORDER BY name").all<{
            name: string;
        }>();
        expect(names.results).toEqual([
            {
                name: "custom/alpha",
            },
            {
                name: "custom/beta",
            },
        ]);
    });

    it("rejects duplicate upstream IDs before changing the database", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");

        const fetchUpstream: typeof fetch = async () =>
            Response.json({
                data: [
                    {
                        id: "same",
                    },
                    {
                        id: "same",
                    },
                ],
            });
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).rejects.toMatchObject({
            status: 502,
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM models").first("count"),
        ).resolves.toBe(0);
    });

    it("rejects an empty upstream model ID before changing the database", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("custom", "openai_compatible", {
            apiKey: "key",
        });
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");

        const fetchUpstream: typeof fetch = async () =>
            Response.json({
                data: [
                    {
                        id: "",
                        object: "model",
                        owned_by: "upstream",
                    },
                ],
            });
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).rejects.toMatchObject({
            message: "The upstream returned an invalid model list",
            status: 502,
        });
        await expect(
            env.DATABASE.prepare("SELECT COUNT(*) AS count FROM models").first("count"),
        ).resolves.toBe(0);
    });

    it("discovers the authenticated Antigravity runtime catalog", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("google", "antigravity_oauth", {
            accessToken: "google-access",
            projectId: "google-project",
            refreshToken: "google-refresh",
        });
        const provider = await findProviderByName(env.DATABASE, "google");
        if (provider === null) throw new Error("provider missing");
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            expect(String(input)).toBe(ANTIGRAVITY_FETCH_MODELS_URL);
            expect(JSON.parse(String(init?.body))).toEqual({
                project: "google-project",
            });
            const headers = new Headers(init?.headers);
            expect(headers.get("Authorization")).toBe("Bearer google-access");
            expect(headers.get("Client-Metadata")).toBe(ANTIGRAVITY_HEADER_CLIENT_METADATA);
            return Response.json({
                models: {
                    "gemini-3.7-flash-tiered": {
                        displayName: "Gemini 3.7 Flash",
                    },
                    "gemini-3.6-flash-low": {
                        displayName: "Gemini 3.6 Flash (Low)",
                    },
                    "claude-sonnet-4-6": {},
                    "claude-internal": {
                        isInternal: true,
                    },
                    "gpt-oss-120b-medium": {},
                    MODEL_PLACEHOLDER_FLASH: {},
                },
            });
        });

        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: 4,
            created: 4,
            removed: 0,
            skipped: 0,
        });
        expect(fetchUpstream).toHaveBeenCalledOnce();
    });

    it("forces one token refresh and retries Antigravity discovery after a 401", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("google", "antigravity_oauth", {
            accessToken: "stale-access",
            projectId: "google-project",
            refreshToken: "google-refresh",
        });
        const provider = await findProviderByName(env.DATABASE, "google");
        if (provider === null) throw new Error("provider missing");
        let discoveryAttempts = 0;
        const fetchUpstream = vi.fn<typeof fetch>(async (input, init) => {
            const url = String(input);
            if (url === ANTIGRAVITY_TOKEN_URL) {
                return Response.json({
                    access_token: "fresh-access",
                    expires_in: 3600,
                });
            }
            expect(url).toBe(ANTIGRAVITY_FETCH_MODELS_URL);
            discoveryAttempts += 1;
            if (discoveryAttempts === 1)
                return new Response(null, {
                    status: 401,
                });
            expect(new Headers(init?.headers).get("Authorization")).toBe("Bearer fresh-access");
            return Response.json({
                models: {
                    "gemini-3.7-flash-tiered": {},
                },
            });
        });

        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: 1,
            created: 1,
            removed: 0,
            skipped: 0,
        });
        expect(discoveryAttempts).toBe(2);
    });

    it("forwards the bounded identity-provider diagnostic when the token refresh is rejected", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("google", "antigravity_oauth", {
            accessToken: "google-access",
            projectId: "google-project",
            refreshToken: "google-refresh",
        });
        // Expire the stored token so discovery must refresh before listing models.
        await env.DATABASE.prepare("UPDATE providers SET expires_at = ? WHERE name = 'google'")
            .bind(1_000_000_000)
            .run();
        const provider = await findProviderByName(env.DATABASE, "google");
        if (provider === null) throw new Error("provider missing");
        const fetchUpstream = vi.fn<typeof fetch>(async () =>
            Response.json(
                {
                    error: "invalid_grant",
                },
                {
                    status: 400,
                },
            ),
        );

        // The RFC 6749 code (README §7) must survive into the panel message.
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).rejects.toMatchObject({
            message: "Identity provider rejected the token refresh (invalid_grant)",
            status: 502,
        });
    });

    it("reports undecryptable credentials distinctly from a refresh failure", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider(
            "google",
            "antigravity_oauth",
            new Uint8Array([
                1,
            ]),
        );
        await env.DATABASE.prepare("UPDATE providers SET expires_at = ? WHERE name = 'google'")
            .bind(1_000_000_000)
            .run();
        const provider = await findProviderByName(env.DATABASE, "google");
        if (provider === null) throw new Error("provider missing");
        const fetchUpstream = vi.fn<typeof fetch>();

        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).rejects.toMatchObject({
            message: "Antigravity credentials could not be decrypted",
            status: 502,
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });

    it("does not mask a client-disconnect abort as a discovery failure", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider("google", "antigravity_oauth", {
            accessToken: "google-access",
            projectId: "google-project",
            refreshToken: "google-refresh",
        });
        await env.DATABASE.prepare("UPDATE providers SET expires_at = ? WHERE name = 'google'")
            .bind(1_000_000_000)
            .run();
        const provider = await findProviderByName(env.DATABASE, "google");
        if (provider === null) throw new Error("provider missing");
        const abortController = new AbortController();
        abortController.abort("client disconnected");
        const fetchUpstream = vi.fn<typeof fetch>(async (_input, init) => {
            // A compliant fetch rejects immediately with the abort reason.
            throw init?.signal?.reason;
        });

        // The abort propagates as-is instead of becoming a fabricated 502.
        await expect(
            discoverProviderModels(
                env.DATABASE,
                provider,
                cipher,
                fetchUpstream,
                abortController.signal,
            ),
        ).rejects.toBeInstanceOf(ProviderAccessError);
    });

    it("uses the pinned OpenAI OAuth catalog without an upstream model-list request", async () => {
        const cipher = await CredentialsCipher.create(TEST_CREDENTIALS_AES_KEY);
        await insertProvider(
            "openai",
            "openai_oauth",
            new Uint8Array([
                1,
            ]),
        );
        const provider = await findProviderByName(env.DATABASE, "openai");
        if (provider === null) throw new Error("provider missing");
        const fetchUpstream = vi.fn<typeof fetch>();

        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: OPENAI_CODEX_MODELS.length,
            created: OPENAI_CODEX_MODELS.length,
            removed: 0,
            skipped: 0,
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });
});
