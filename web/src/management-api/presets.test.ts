import { describe, expect, it } from "vitest";
import { PROVIDER_PRESETS, type ProviderPreset } from "./presets";

const FORMATS = new Set([
    "openai_response",
    "openai_chat_completion",
    "gemini_generate_content",
    "gemini_interaction",
    "anthropic_messages",
]);

/** Finds one catalog preset by id, failing the test when absent. */
function findPreset(id: string): ProviderPreset {
    const found = PROVIDER_PRESETS.find((candidate) => candidate.id === id);
    expect(found, `preset ${id} must be in the catalog`).toBeDefined();
    return found as ProviderPreset;
}

/** Reads one value out of the preset's payload document. */
function at<T = unknown>(entry: ProviderPreset, path: ReadonlyArray<string | number>): T {
    let current: unknown = entry.payload;

    for (const key of path) {
        expect(current, `payload path ${path.join(".")} at ${String(key)}`).toBeTypeOf("object");
        current = (current as Record<string | number, unknown>)[key];
    }

    return current as T;
}

/** Every header row of one control request. */
function headerNames(request: unknown): ReadonlyArray<{
    name: string;
    value: unknown[];
}> {
    return ((
        request as {
            headers?: ReadonlyArray<{
                name: string;
                value: unknown[];
            }>;
        }
    ).headers ?? []) as ReadonlyArray<{
        name: string;
        value: unknown[];
    }>;
}

/** The payload document of one preset, as an indexable record. */
function payloadOf(entry: ProviderPreset): Record<string, unknown> {
    return entry.payload as Record<string, unknown>;
}

/** The provisioning steps of one preset's payload, when it carries any. */
function payloadSteps(entry: ProviderPreset): ReadonlyArray<{
    extract: ReadonlyArray<{
        target: {
            type: string;
        };
    }>;
}> {
    return (
        ((entry.payload as Record<string, unknown>)["provisioning"] as ReadonlyArray<{
            extract: ReadonlyArray<{
                target: {
                    type: string;
                };
            }>;
        }>) ?? []
    );
}

describe("preset catalog", () => {
    it("covers every researched product plus the generic per-format presets", () => {
        const ids = PROVIDER_PRESETS.map((candidate) => candidate.id);

        for (const id of [
            "codex",
            "antigravity",
            "claude-code",
            "kimi-code",
            "github-copilot",
            "devin",
        ]) {
            expect(ids).toContain(id);
        }

        for (const id of [
            "generic-openai-responses",
            "generic-openai-chat-completions",
            "generic-gemini-generate-content",
            "generic-gemini-interactions",
            "generic-anthropic-messages",
        ]) {
            expect(ids).toContain(id);
        }

        for (const id of [
            "zen-openai-chat-completions",
            "zen-openai-responses",
            "zen-anthropic-messages",
            "zen-gemini-generate-content",
        ]) {
            expect(ids).toContain(id);
        }
    });

    it("marks the researched-but-unconfirmed products unverified and the rest live", () => {
        for (const id of [
            "codex",
            "antigravity",
            "generic-openai-chat-completions",
        ]) {
            expect(findPreset(id).verification).toBe("live");
        }

        for (const id of [
            "claude-code",
            "kimi-code",
            "github-copilot",
            "devin",
            "zen-openai-chat-completions",
        ]) {
            expect(findPreset(id).verification).toBe("unverified");
        }
    });

    it("keeps every payload structurally conformant with the descriptor contract", () => {
        for (const candidate of PROVIDER_PRESETS) {
            const payload = candidate.payload as Record<string, unknown>;

            expect(FORMATS.has(payload["format"] as string), candidate.id).toBe(true);

            const inference = payload["inference"] as {
                url: unknown[];
                upstream_streaming: string;
            };
            expect(inference.url.length, candidate.id).toBeGreaterThan(0);
            expect([
                "always",
                "match_client",
            ]).toContain(inference.upstream_streaming);

            const credential = payload["credential"] as Record<string, unknown>;

            if (candidate.credentialKind === "oauth") {
                expect(credential["type"], candidate.id).toBe("oauth");
                expect(credential["client_id"], candidate.id).toBeTypeOf("string");

                const exchange = credential["exchange"] as {
                    terminal_codes: string[];
                };
                expect(
                    exchange.terminal_codes.length,
                    `${candidate.id} terminal codes`,
                ).toBeGreaterThan(0);
            } else {
                expect(credential, candidate.id).toEqual({
                    type: "api_key",
                });
                expect(candidate.grant, candidate.id).toBeNull();
            }

            // A definition whose URLs reference the base either needs the
            // operator's base URL at creation, or mints the base itself from
            // an exchange/provisioning extraction (the Copilot account
            // endpoint). A definition with pinned endpoints needs neither.
            const urlText = JSON.stringify(inference.url);
            const referencesBase = urlText.includes('"base"');
            const mintsBase = JSON.stringify(payload).includes('"base_url"');

            if (referencesBase) {
                expect(
                    candidate.needsBaseUrl || mintsBase,
                    `${candidate.id} references the base but neither asks for it nor mints it`,
                ).toBe(true);
            } else {
                expect(candidate.needsBaseUrl, `${candidate.id}`).toBe(false);
            }
        }
    });
});

describe("claude-code preset", () => {
    const claude = findPreset("claude-code");

    it("rides the OAuth beta header on inference and discovery alike", () => {
        const inferenceHeaders = headerNames(
            (claude.payload as Record<string, unknown>)["inference"] as unknown,
        );
        const discoveryHeaders = headerNames(
            ((claude.payload as Record<string, unknown>)["discovery"] as Record<string, unknown>)[
                "request"
            ] as unknown,
        );

        const beta = (rows: ReturnType<typeof headerNames>): boolean =>
            rows.some(
                (row) =>
                    row.name === "anthropic-beta" &&
                    JSON.stringify(row.value).includes("oauth-2025-04-20"),
            );

        expect(beta(inferenceHeaders)).toBe(true);
        expect(beta(discoveryHeaders)).toBe(true);
    });

    it("uses PKCE with the hosted paste-back callback", () => {
        const grant = at<Record<string, unknown>>(claude, [
            "credential",
            "grant",
        ]);

        expect(grant["pkce"]).toBe("s256");
        expect(grant["redirect_uri"]).toBe("https://console.anthropic.com/oauth/code/callback");

        const extra = JSON.stringify(grant["extra_params"]);
        expect(extra).toContain('"code"');
        expect(extra).toContain("true");
    });

    it("walks the paginated model list by has_more and last_id", () => {
        const pagination = at<Record<string, unknown>>(claude, [
            "discovery",
            "pagination",
        ]);

        expect(pagination["type"]).toBe("cursor");
        expect(pagination["has_more_path"]).toEqual([
            "has_more",
        ]);
        expect(pagination["last_id_path"]).toEqual([
            "last_id",
        ]);
        expect((pagination["request_param"] as string).length).toBeGreaterThan(0);
    });
});

describe("kimi-code preset", () => {
    const kimi = findPreset("kimi-code");

    it("carries the stable device slot across authorization, polling, and refresh", () => {
        const credential = payloadOf(kimi)["credential"] as Record<string, unknown>;

        // The slot is initialized once per flow.
        const initial = (
            credential["initial_slots"] as ReadonlyArray<{
                name: string;
            }>
        ).map((initializer) => initializer.name);
        expect(initial).toContain("device_id");

        // The device authorization request sends it.
        const grant = credential["grant"] as Record<string, unknown>;
        const authorizationRequest = (grant["authorization"] as Record<string, unknown>)["request"];
        expect(JSON.stringify(headerNames(authorizationRequest))).toContain("device_id");

        // The polling exchange and the refresh grant send the same slot.
        const exchange = credential["exchange"] as {
            request: unknown;
        };
        expect(JSON.stringify(headerNames(exchange.request))).toContain("device_id");

        const refresh = credential["refresh"] as {
            request: unknown;
        };
        expect(JSON.stringify(headerNames(refresh.request))).toContain("device_id");
    });

    it("maps the device response including the complete verification URI", () => {
        const grant = (payloadOf(kimi)["credential"] as Record<string, unknown>)["grant"] as Record<
            string,
            unknown
        >;
        const authorization = grant["authorization"] as {
            response: {
                fields: ReadonlyArray<{
                    target: {
                        type: string;
                    };
                }>;
            };
        };

        const targets = authorization.response.fields.map((field) => field.target.type);

        expect(targets).toContain("device_code");
        expect(targets).toContain("user_code");
        expect(targets).toContain("verification_uri_complete");
    });
});

describe("github-copilot preset", () => {
    const copilot = findPreset("github-copilot");

    it("banks the durable token in a slot and reprovisions the session", () => {
        const credential = payloadOf(copilot)["credential"] as Record<string, unknown>;

        // The exchange's product is the durable slot, not the access token.
        const exchange = credential["exchange"] as {
            response: {
                fields: ReadonlyArray<{
                    target: Record<string, string>;
                }>;
            };
        };
        expect(exchange.response.fields.map((field) => field.target)).toContainEqual({
            type: "slot",
            name: "github_token",
        });

        // The renewal is reprovisioning, so the durable token re-mints the
        // short-lived session token, its expiry, and the account API endpoint.
        expect(credential["refresh"]).toEqual({
            type: "reprovision",
        });

        const extracted = payloadSteps(copilot).flatMap((step) => step.extract);

        expect(extracted).toContainEqual(
            expect.objectContaining({
                target: {
                    type: "access_token",
                },
            }),
        );
        expect(extracted).toContainEqual(
            expect.objectContaining({
                target: {
                    type: "expires_at_unix_seconds",
                },
            }),
        );
        expect(extracted).toContainEqual(
            expect.objectContaining({
                target: {
                    type: "base_url",
                },
            }),
        );
    });

    it("points inference and discovery at the account's returned endpoint", () => {
        const payload = payloadOf(copilot);

        const inference = payload["inference"] as {
            url: unknown[];
        };
        expect(JSON.stringify(inference.url)).toContain('"base"');

        const discovery = payload["discovery"] as {
            request: {
                url: unknown[];
            };
        };
        expect(JSON.stringify(discovery.request.url)).toContain('"base"');
    });
});

describe("devin preset", () => {
    const devin = findPreset("devin");

    it("exchanges to a static key and server URL and never renews", () => {
        const credential = payloadOf(devin)["credential"] as Record<string, unknown>;
        const exchange = credential["exchange"] as {
            request: unknown;
            response: {
                fields: ReadonlyArray<{
                    path: string[];
                    target: {
                        type: string;
                    };
                }>;
            };
        };

        // The ConnectRPC exchange carries its protocol header.
        expect(JSON.stringify(exchange.request)).toContain("connect-protocol-version");

        const fields = exchange.response.fields;
        expect(fields.length).toBe(2);

        const [apiKey, serverUrl] = fields;

        expect(apiKey?.path).toEqual([
            "api_key",
        ]);
        expect(apiKey?.target).toEqual({
            type: "access_token",
        });
        expect(serverUrl?.path).toEqual([
            "api_server_url",
        ]);
        expect(serverUrl?.target).toEqual({
            type: "base_url",
        });

        expect(credential["refresh"]).toEqual({
            type: "none",
        });
    });

    it("ships manual models only — no discovery descriptor", () => {
        expect(payloadOf(devin)["discovery"]).toBeUndefined();
    });
});

describe("opencode zen presets", () => {
    it("offers one static-key definition per supported format", () => {
        const zen = PROVIDER_PRESETS.filter((candidate) => candidate.id.startsWith("zen-"));

        expect(zen.map((candidate) => candidate.format).sort()).toEqual([
            "anthropic_messages",
            "gemini_generate_content",
            "openai_chat_completion",
            "openai_response",
        ]);

        for (const candidate of zen) {
            expect(candidate.credentialKind).toBe("api_key");
            expect(payloadOf(candidate)["credential"]).toEqual({
                type: "api_key",
            });
        }
    });

    it("discovers the plain ID list on every variant", () => {
        for (const candidate of PROVIDER_PRESETS.filter((entry) => entry.id.startsWith("zen-"))) {
            const discovery = payloadOf(candidate)["discovery"] as Record<string, unknown>;

            expect(discovery["extract"]).toEqual({
                mode: "array",
                array_at: [
                    "data",
                ],
                id_field: [
                    "id",
                ],
            });
        }
    });
});
