import { env } from "cloudflare:workers";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
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
    type: "openai_oauth" | "openai_compatible",
    credentials: Uint8Array,
): Promise<void> {
    await env.DATABASE.prepare(
        `INSERT INTO providers (
            name, model_prefix, type, protocol, base_url, encrypted_credentials,
            expires_at, refresh_started_at, enabled
        ) VALUES (?, ?, ?, ?, ?, ?, NULL, NULL, 1)`,
    )
        .bind(
            name,
            `${name}/`,
            type,
            type === "openai_compatible" ? "openai_response" : null,
            type === "openai_compatible" ? "https://upstream.test/v1" : null,
            credentials,
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
            skipped: 0,
        });
        expect(fetchUpstream).toHaveBeenCalledWith("https://upstream.test/v1/models", {
            headers: {
                Authorization: "Bearer key",
            },
            method: "GET",
        });

        const rows = await env.DATABASE.prepare(
            "SELECT name, upstream_name, thinking_levels, enabled FROM models ORDER BY id",
        ).all<{
            name: string;
            upstream_name: string;
            thinking_levels: string | null;
            enabled: number;
        }>();
        expect(rows.results).toEqual([
            {
                enabled: 0,
                name: "custom/alpha",
                thinking_levels: null,
                upstream_name: "alpha",
            },
            {
                enabled: 0,
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
            skipped: 2,
        });
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

    it("uses the pinned OAuth catalog without an upstream model-list request", async () => {
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
            skipped: 0,
        });
        expect(fetchUpstream).not.toHaveBeenCalled();
    });
});
