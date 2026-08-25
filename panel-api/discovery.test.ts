import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import {
    ANTIGRAVITY_FETCH_MODELS_URL,
    ANTIGRAVITY_HEADER_CLIENT_METADATA,
    ANTIGRAVITY_TOKEN_URL,
} from "/api/adapters/antigravity";
import { OPENAI_CODEX_MODELS } from "/api/adapters/openai";
import { CredentialsCipher } from "/common/credentials";
import { clearTestDatabase, initializeTestDatabase } from "/tests/database";
import { discoverProviderModels } from "./discovery";
import { findProviderByName } from "./provider-repository";

const CREDENTIAL_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

beforeAll(async () => initializeTestDatabase(env.DATABASE));
beforeEach(async () => clearTestDatabase(env.DATABASE));

async function insertProvider(
    name: string,
    type: "antigravity_oauth" | "openai_oauth" | "openai_compatible",
    credentials: Uint8Array,
): Promise<void> {
    await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES (?, ?, ?, ?, ?, ?, ?, NULL, 1)`,
    )
        .bind(
            name,
            `${name}/`,
            type,
            type === "openai_compatible" ? "openai_response" : null,
            type === "openai_compatible" ? "https://upstream.test/v1" : null,
            credentials,
            // OAuth providers always store a token expiry; API-key providers never do.
            type === "openai_compatible" ? null : 2_000_000_000,
        )
        .run();
}

describe("model discovery", () => {
    it("authenticates compatible discovery and atomically creates disabled mappings", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await insertProvider(
            "custom",
            "openai_compatible",
            await cipher.encrypt({
                apiKey: "key",
            }),
        );
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
            `SELECT name, upstream_name, thinking_levels, enabled, is_manually_edited
            FROM models ORDER BY id`,
        ).all<{
            name: string;
            upstream_name: string;
            thinking_levels: string | null;
            enabled: number;
            is_manually_edited: number;
        }>();
        expect(rows.results).toEqual([
            {
                enabled: 0,
                is_manually_edited: 0,
                name: "custom/alpha",
                thinking_levels: null,
                upstream_name: "alpha",
            },
            {
                enabled: 0,
                is_manually_edited: 0,
                name: "custom/beta",
                thinking_levels: null,
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
    });

    it("matches by provider and upstream name while pruning only untouched stale models", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await insertProvider(
            "custom",
            "openai_compatible",
            await cipher.encrypt({
                apiKey: "key",
            }),
        );
        const provider = await findProviderByName(env.DATABASE, "custom");
        if (provider === null) throw new Error("provider missing");

        let discoveredNames = [
            "alpha",
            "beta",
            "gamma",
        ];
        const fetchUpstream: typeof fetch = async () =>
            Response.json({
                data: discoveredNames.map((id) => ({
                    id,
                })),
            });
        await discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream);
        await env.DATABASE.prepare(
            `UPDATE models SET name = 'custom/renamed-alpha', is_manually_edited = 1
            WHERE provider_id = ? AND upstream_name = 'alpha'`,
        )
            .bind(provider.id)
            .run();
        await env.DATABASE.prepare(
            `UPDATE models SET enabled = 1, is_manually_edited = 1
            WHERE provider_id = ? AND upstream_name = 'beta'`,
        )
            .bind(provider.id)
            .run();

        discoveredNames = [
            "alpha",
            "beta",
            "delta",
        ];
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
        ).resolves.toEqual({
            total: 3,
            created: 1,
            removed: 1,
            skipped: 2,
        });
        const rows = await env.DATABASE.prepare(
            `SELECT name, upstream_name, enabled, is_manually_edited
            FROM models WHERE provider_id = ? ORDER BY upstream_name`,
        )
            .bind(provider.id)
            .all();
        expect(rows.results).toEqual([
            {
                enabled: 0,
                is_manually_edited: 1,
                name: "custom/renamed-alpha",
                upstream_name: "alpha",
            },
            {
                enabled: 1,
                is_manually_edited: 1,
                name: "custom/beta",
                upstream_name: "beta",
            },
            {
                enabled: 0,
                is_manually_edited: 0,
                name: "custom/delta",
                upstream_name: "delta",
            },
        ]);

        discoveredNames = [
            "delta",
        ];
        await expect(
            discoverProviderModels(env.DATABASE, provider, cipher, fetchUpstream),
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

    it("rejects duplicate upstream IDs before changing the database", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await insertProvider(
            "custom",
            "openai_compatible",
            await cipher.encrypt({
                apiKey: "key",
            }),
        );
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

    it("discovers the authenticated Antigravity runtime catalog", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await insertProvider(
            "google",
            "antigravity_oauth",
            await cipher.encrypt({
                accessToken: "google-access",
                projectId: "google-project",
                refreshToken: "google-refresh",
            }),
        );
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
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
        await insertProvider(
            "google",
            "antigravity_oauth",
            await cipher.encrypt({
                accessToken: "stale-access",
                projectId: "google-project",
                refreshToken: "google-refresh",
            }),
        );
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

    it("uses the pinned OpenAI OAuth catalog without an upstream model-list request", async () => {
        const cipher = await CredentialsCipher.create(CREDENTIAL_KEY);
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
