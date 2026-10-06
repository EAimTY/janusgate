import { describe, expect, it } from "vitest";
import { hasBaseReference } from "../management-api/definitions/base-reference";
import { PROVIDER_PRESETS } from "./catalog";
import type { ProviderPreset } from "./types";

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
            "xai",
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

    it("pins every preset's verification status against the researched facts", () => {
        // The status drives the user-facing Verified badge and provenance
        // line, so every catalog entry is pinned against this table — a new
        // preset must state its researched status here, and a flip anywhere
        // fails loudly.
        //
        // Codex rides the unverified list since its compaction section:
        // the login and inference shape was live-verified, but a preset
        // carrying compaction declarations is not live-verified until an
        // authenticated compact-and-continuation run passes (README §5.6).
        const expectedStatuses: Record<string, ProviderPreset["verification"]> = {
            antigravity: "live",
            "generic-openai-chat-completions": "live",
            "generic-openai-responses": "live",
            "generic-gemini-generate-content": "live",
            "generic-gemini-interactions": "live",
            "generic-anthropic-messages": "live",
            codex: "unverified",
            "claude-code": "unverified",
            "kimi-code": "unverified",
            "github-copilot": "unverified",
            devin: "unverified",
            xai: "unverified",
            "zen-openai-chat-completions": "unverified",
            "zen-openai-responses": "unverified",
            "zen-anthropic-messages": "unverified",
            "zen-gemini-generate-content": "unverified",
        };

        expect(new Set(PROVIDER_PRESETS.map((candidate) => candidate.id))).toEqual(
            new Set(Object.keys(expectedStatuses)),
        );

        for (const candidate of PROVIDER_PRESETS) {
            expect(candidate.verification, candidate.id).toBe(expectedStatuses[candidate.id]);
        }
    });

    it("keeps every payload structurally conformant with the descriptor contract", () => {
        for (const candidate of PROVIDER_PRESETS) {
            // The payload's shape is the generated contract's own type, and the
            // authored-payload validator walks it wholesale (validation.test.ts);
            // what stays here is the part no type can carry: non-empty
            // expressions and terminal-code lists, and the catalog's own
            // credential fact.
            const payload = candidate.payload;
            expect(payload.inference.url.length, candidate.id).toBeGreaterThan(0);

            const credential = payload.credential;

            if (credential.type === "oauth") {
                expect(
                    credential.exchange.terminal_codes.length,
                    `${candidate.id} terminal codes`,
                ).toBeGreaterThan(0);
                expect(candidate.grant, candidate.id).toBeDefined();
            } else {
                expect(candidate.grant, candidate.id).toBeNull();
            }
        }
    });
});

/** The OAuth credential of a preset known to carry one, failing the test otherwise. */
function oauthCredentialOf(entry: ProviderPreset): Extract<
    ProviderPreset["payload"]["credential"],
    {
        type: "oauth";
    }
> {
    const { credential } = entry.payload;
    if (credential.type !== "oauth") {
        throw new Error(`preset ${entry.id} carries no OAuth credential`);
    }
    return credential;
}

describe("derived base-URL neediness", () => {
    const claude = findPreset("claude-code");
    const claudeOAuth = oauthCredentialOf(claude);
    const claudeGrant = claudeOAuth.grant;
    const claudeRefresh = claudeOAuth.refresh;
    const claudeDiscovery = claude.payload.discovery;

    if (claudeGrant.type !== "authorization_code") {
        throw new Error("the claude-code preset runs an authorization-code grant");
    }

    if (claudeDiscovery === undefined || claudeDiscovery === null) {
        throw new Error("the claude-code preset discovers its model list");
    }

    if (claudeRefresh.type !== "grant") {
        throw new Error("the claude-code preset renews with a refresh grant");
    }

    const kimi = findPreset("kimi-code");
    const kimiOAuth = oauthCredentialOf(kimi);
    const kimiGrant = kimiOAuth.grant;

    if (kimiGrant.type !== "device") {
        throw new Error("the kimi-code preset runs a device grant");
    }

    it("answers each preset's neediness from its payload's own URL expressions", () => {
        // The generic static-key presets and xAI address any compatible
        // endpoint through the base — the xAI compact operation rides the
        // base the same way its inference does — and GitHub Copilot's and
        // Devin's inference and discovery URLs reference it too — their
        // exchanges and provisioning re-mint the base afterwards, but the
        // gateway still rejects a creation or flow start naming such a
        // definition without a base URL, so neediness follows the
        // expressions, not the renewal story. The remaining presets pin
        // their endpoints.
        for (const id of [
            "generic-openai-responses",
            "generic-openai-chat-completions",
            "generic-gemini-generate-content",
            "generic-gemini-interactions",
            "generic-anthropic-messages",
            "github-copilot",
            "devin",
            "xai",
        ]) {
            expect(hasBaseReference(findPreset(id).payload), id).toBe(true);
        }

        for (const id of [
            "codex",
            "antigravity",
            "claude-code",
            "kimi-code",
            "zen-openai-chat-completions",
            "zen-openai-responses",
            "zen-anthropic-messages",
            "zen-gemini-generate-content",
        ]) {
            expect(hasBaseReference(findPreset(id).payload), id).toBe(false);
        }
    });

    it("counts every expression surface whose evaluation environment carries the base", () => {
        // An inference URL referencing the base — directly or through the
        // coalesce, the one compound part — addresses the instance.
        expect(
            hasBaseReference({
                ...claude.payload,
                inference: {
                    ...claude.payload.inference,
                    url: [
                        {
                            type: "base",
                        },
                    ],
                },
            }),
        ).toBe(true);
        expect(
            hasBaseReference({
                ...claude.payload,
                inference: {
                    ...claude.payload.inference,
                    url: [
                        {
                            type: "coalesce",
                            parts: [
                                {
                                    type: "base",
                                },
                            ],
                        },
                    ],
                },
            }),
        ).toBe(true);

        // The authorize URL is the one excluded member — its stage bars the
        // base reference at write time (gateway README §5.1) — so a base
        // planted there never counts, while an inference header row's value
        // is evaluated against the base like any other expression surface.
        expect(
            hasBaseReference({
                ...claude.payload,
                credential: {
                    ...claudeOAuth,
                    grant: {
                        ...claudeGrant,
                        authorize_url: [
                            {
                                type: "base",
                            },
                        ],
                    },
                },
            }),
        ).toBe(false);
        expect(
            hasBaseReference({
                ...claude.payload,
                inference: {
                    ...claude.payload.inference,
                    headers: [
                        {
                            name: "X-Endpoint",
                            value: [
                                {
                                    type: "base",
                                },
                            ],
                        },
                    ],
                },
            }),
        ).toBe(true);
    });

    it("answers true for a base reference planted in each URL-bearing member the walk reads", () => {
        // The catalog's own neediness can never fail on these arms: every
        // preset that carries one of them pins its endpoints, so a broken arm
        // would stay green behind the inference URL's verdict. Each arm here
        // carries the base itself — the gateway fixture's own shape — and
        // must answer true on its own.
        const plantedUrl = (): Array<{
            type: "base";
        }> => [
            {
                type: "base",
            },
        ];

        // The streaming URL shares the inference URL's evaluation
        // environment.
        expect(
            hasBaseReference({
                ...claude.payload,
                inference: {
                    ...claude.payload.inference,
                    stream_url: plantedUrl(),
                },
            }),
        ).toBe(true);

        // The device-authorization request URL addresses the instance.
        expect(
            hasBaseReference({
                ...kimi.payload,
                credential: {
                    ...kimiOAuth,
                    grant: {
                        ...kimiGrant,
                        authorization: {
                            ...kimiGrant.authorization,
                            request: {
                                ...kimiGrant.authorization.request,
                                url: plantedUrl(),
                            },
                        },
                    },
                },
            }),
        ).toBe(true);

        // The token-exchange request URL addresses the instance.
        expect(
            hasBaseReference({
                ...claude.payload,
                credential: {
                    ...claudeOAuth,
                    exchange: {
                        ...claudeOAuth.exchange,
                        request: {
                            ...claudeOAuth.exchange.request,
                            url: plantedUrl(),
                        },
                    },
                },
            }),
        ).toBe(true);

        // The refresh grant's request URL addresses the instance.
        expect(
            hasBaseReference({
                ...claude.payload,
                credential: {
                    ...claudeOAuth,
                    refresh: {
                        ...claudeRefresh,
                        request: {
                            ...claudeRefresh.request,
                            url: plantedUrl(),
                        },
                    },
                },
            }),
        ).toBe(true);

        // A provisioning step's request URL addresses the instance.
        expect(
            hasBaseReference({
                ...claude.payload,
                provisioning: [
                    {
                        request: {
                            method: "POST",
                            url: plantedUrl(),
                        },
                        response_encoding: "json",
                    },
                ],
            }),
        ).toBe(true);

        // A provisioning poll's request URL addresses the instance too.
        expect(
            hasBaseReference({
                ...claude.payload,
                provisioning: [
                    {
                        request: {
                            method: "GET",
                            url: [
                                {
                                    type: "literal",
                                    value: "https://upstream.example.com/provision",
                                },
                            ],
                        },
                        response_encoding: "json",
                        poll: {
                            request: {
                                method: "GET",
                                url: plantedUrl(),
                            },
                            response_encoding: "json",
                            done_path: [
                                "done",
                            ],
                            error_path: [
                                "error",
                            ],
                            interval_ms: 1_000,
                            budget_ms: 60_000,
                        },
                    },
                ],
            }),
        ).toBe(true);

        // The discovery request URL addresses the instance too. No catalog
        // preset can witness this arm — its discovery URL references the
        // base only where its inference URL does as well — so the plant in
        // the endpoints-pinning claude payload is the arm's only coverage.
        expect(
            hasBaseReference({
                ...claude.payload,
                discovery: {
                    ...claudeDiscovery,
                    request: {
                        ...claudeDiscovery.request,
                        url: plantedUrl(),
                    },
                },
            }),
        ).toBe(true);
    });
});

describe("generic Gemini presets", () => {
    it("addresses the GenerateContent resource path under the discovered bare model id", () => {
        // Discovery strips the `models/` resource prefix off every reported
        // name (public model identifiers exclude `/`), so the inference URLs
        // must spell the resource segment the protocol requires:
        // `{base}/models/{model}:generateContent`.
        const gemini = findPreset("generic-gemini-generate-content");

        expect(
            at(gemini, [
                "inference",
                "url",
            ]),
        ).toEqual([
            {
                type: "base",
            },
            {
                type: "literal",
                value: "/models/",
            },
            {
                type: "model",
            },
            {
                type: "literal",
                value: ":generateContent",
            },
        ]);
        expect(
            at(gemini, [
                "inference",
                "stream_url",
            ]),
        ).toEqual([
            {
                type: "base",
            },
            {
                type: "literal",
                value: "/models/",
            },
            {
                type: "model",
            },
            {
                type: "literal",
                value: ":streamGenerateContent?alt=sse",
            },
        ]);
    });

    for (const id of [
        "generic-gemini-generate-content",
        "generic-gemini-interactions",
    ]) {
        it(`walks the paginated model list by pageToken and nextPageToken (${id})`, () => {
            const gemini = findPreset(id);
            const pagination = at<Record<string, unknown>>(gemini, [
                "discovery",
                "pagination",
            ]);

            expect(pagination["type"]).toBe("token");
            expect(pagination["request_param"]).toBe("pageToken");
            expect(pagination["response_path"]).toEqual([
                "nextPageToken",
            ]);
            expect(pagination["max_pages"]).toBe(10);

            // The page size rides the discovery request at the API's
            // 1000-model maximum, so the budget covers ten thousand models
            // before the loud budget failure can ever fire.
            const query = at<unknown>(gemini, [
                "discovery",
                "request",
                "query",
            ]);
            expect(query).toEqual([
                {
                    name: "pageSize",
                    value: [
                        {
                            type: "literal",
                            value: "1000",
                        },
                    ],
                },
            ]);
        });
    }
});

describe("claude-code preset", () => {
    const claude = findPreset("claude-code");

    it("rides the OAuth beta and version headers on inference and discovery alike", () => {
        const inferenceHeaders = headerNames(
            (claude.payload as Record<string, unknown>)["inference"] as unknown,
        );
        const discoveryHeaders = headerNames(
            ((claude.payload as Record<string, unknown>)["discovery"] as Record<string, unknown>)[
                "request"
            ] as unknown,
        );

        const carriesHeader = (
            rows: ReturnType<typeof headerNames>,
            name: string,
            value: string,
        ): boolean =>
            rows.some((row) => row.name === name && JSON.stringify(row.value).includes(value));

        expect(carriesHeader(inferenceHeaders, "anthropic-beta", "oauth-2025-04-20")).toBe(true);
        expect(carriesHeader(discoveryHeaders, "anthropic-beta", "oauth-2025-04-20")).toBe(true);
        expect(carriesHeader(inferenceHeaders, "anthropic-version", "2023-06-01")).toBe(true);
        expect(carriesHeader(discoveryHeaders, "anthropic-version", "2023-06-01")).toBe(true);
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

        expect(zen.map((candidate) => payloadOf(candidate)["format"]).sort()).toEqual([
            "anthropic_messages",
            "gemini_generate_content",
            "openai_chat_completion",
            "openai_response",
        ]);

        for (const candidate of zen) {
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

describe("preset compaction sections", () => {
    /** The mechanisms object of one preset's compaction section. */
    function mechanismsOf(entry: ProviderPreset): Record<string, Record<string, unknown>> {
        const compaction = payloadOf(entry)["compaction"] as Record<string, unknown> | undefined;

        if (compaction === undefined) {
            throw new Error(`preset ${entry.id} carries no compaction section`);
        }

        return compaction["mechanisms"] as Record<string, Record<string, unknown>>;
    }

    it("leaves the generic presets unextended — no evidence of compaction support", () => {
        for (const candidate of PROVIDER_PRESETS.filter((entry) =>
            entry.id.startsWith("generic-"),
        )) {
            expect(payloadOf(candidate)["compaction"], candidate.id).toBeUndefined();
        }
    });

    it("declares the xAI standalone compact against the base and both other mechanisms unsupported", () => {
        const xai = findPreset("xai");
        const mechanisms = mechanismsOf(xai);

        // The standalone declaration carries its operation, and the
        // operation addresses the instance base exactly like the inference
        // does — never a string-appended variant of it.
        expect(mechanisms["responses_standalone_compact"]).toMatchObject({
            default: "supported",
            operation: {
                url: [
                    {
                        type: "base",
                    },
                    {
                        type: "literal",
                        value: "/responses/compact",
                    },
                ],
            },
        });

        // A standalone compact endpoint is not a trigger or an automatic
        // compaction: both stay positively unsupported, never unknown.
        expect(mechanisms["responses_explicit_trigger"]).toEqual({
            default: "unsupported",
        });
        expect(mechanisms["responses_automatic_compaction"]).toEqual({
            default: "unsupported",
        });

        // The facts are researched, not live-run.
        expect(xai.verification).toBe("unverified");
    });

    it("declares the two Claude betas as the separate facts they are", () => {
        const claude = findPreset("claude-code");
        const mechanisms = mechanismsOf(claude);

        const overlayValue = (mechanism: string): string | undefined => {
            const declaration = mechanisms[mechanism];
            const headers = declaration?.["overlay_headers"] as
                | Array<{
                      name: string;
                      value: unknown;
                  }>
                | undefined;
            const row = headers?.find((candidate) => candidate.name === "anthropic-beta");

            return row === undefined ? undefined : JSON.stringify(row.value);
        };

        expect(mechanisms["messages_on_demand_compaction"]).toMatchObject({
            default: "supported",
        });
        expect(overlayValue("messages_on_demand_compaction") ?? "").toContain("compact-2026-09-04");

        expect(mechanisms["messages_threshold_compaction"]).toMatchObject({
            default: "supported",
        });
        expect(overlayValue("messages_threshold_compaction") ?? "").toContain("compact-2026-01-12");

        // The research status is unchanged: the betas are two different
        // facts, and neither has run live from this codebase.
        expect(claude.verification).toBe("unverified");
    });

    it("declares the Codex trigger and automatic mechanisms the backend accepts", () => {
        const codex = findPreset("codex");
        const mechanisms = mechanismsOf(codex);

        expect(mechanisms["responses_explicit_trigger"]).toMatchObject({
            default: "supported",
        });
        expect(mechanisms["responses_automatic_compaction"]).toMatchObject({
            default: "supported",
        });

        // The login and inference shape was live-verified, but a preset
        // carrying compaction declarations is not live-verified until an
        // authenticated compact-and-continuation run passes.
        expect(codex.verification).toBe("unverified");
    });

    it("counts a base reference planted in each compaction surface the walk reads", () => {
        // Every catalog preset that carries a compaction section pins its
        // endpoints, so a broken arm would stay green behind the verdicts
        // above. Each arm here plants the base itself and must answer true
        // on its own.
        const plantedUrl = (): [
            {
                type: "base";
            },
        ] => [
            {
                type: "base",
            },
        ];

        /** The declared compaction section of a preset known to carry one. */
        function compactionOf(
            entry: ProviderPreset,
        ): NonNullable<ProviderPreset["payload"]["compaction"]> {
            const section = entry.payload.compaction;

            if (section === undefined || section === null) {
                throw new Error(`preset ${entry.id} carries no compaction section`);
            }

            return section;
        }

        const xai = findPreset("xai");
        const xaiStandalone = compactionOf(xai).mechanisms.responses_standalone_compact;

        if (xaiStandalone === undefined || xaiStandalone === null) {
            throw new Error("the xai preset declares the standalone compact");
        }

        const xaiOperation = xaiStandalone.operation ?? {
            url: [
                {
                    type: "literal" as const,
                    value: "https://api.x.ai/v1/responses/compact",
                },
            ],
        };

        // The standalone operation's URL, header rows, and envelope string
        // leaves all evaluate against the base.
        expect(
            hasBaseReference({
                ...xai.payload,
                compaction: {
                    ...compactionOf(xai),
                    mechanisms: {
                        ...compactionOf(xai).mechanisms,
                        responses_standalone_compact: {
                            ...xaiStandalone,
                            operation: {
                                ...xaiOperation,
                                url: plantedUrl(),
                            },
                        },
                    },
                },
            }),
        ).toBe(true);

        expect(
            hasBaseReference({
                ...xai.payload,
                compaction: {
                    ...compactionOf(xai),
                    mechanisms: {
                        ...compactionOf(xai).mechanisms,
                        responses_standalone_compact: {
                            ...xaiStandalone,
                            operation: {
                                ...xaiOperation,
                                headers: [
                                    {
                                        name: "X-Endpoint",
                                        value: plantedUrl(),
                                    },
                                ],
                            },
                        },
                    },
                },
            }),
        ).toBe(true);

        expect(
            hasBaseReference({
                ...xai.payload,
                compaction: {
                    ...compactionOf(xai),
                    mechanisms: {
                        ...compactionOf(xai).mechanisms,
                        responses_standalone_compact: {
                            ...xaiStandalone,
                            operation: {
                                ...xaiOperation,
                                envelope: {
                                    type: "object",
                                    fields: {
                                        wrapped: {
                                            type: "string",
                                            expr: plantedUrl(),
                                        },
                                        body: {
                                            type: "body",
                                        },
                                    },
                                },
                            },
                        },
                    },
                },
            }),
        ).toBe(true);

        // The Messages overlay headers evaluate against the base too.
        const claude = findPreset("claude-code");
        const claudeOnDemand = compactionOf(claude).mechanisms.messages_on_demand_compaction ?? {
            default: "supported" as const,
        };

        expect(
            hasBaseReference({
                ...claude.payload,
                compaction: {
                    ...compactionOf(claude),
                    mechanisms: {
                        ...compactionOf(claude).mechanisms,
                        messages_on_demand_compaction: {
                            ...claudeOnDemand,
                            overlay_headers: [
                                {
                                    name: "X-Endpoint",
                                    value: plantedUrl(),
                                },
                            ],
                        },
                    },
                },
            }),
        ).toBe(true);

        // The token-count operation's URL addresses the instance as well.
        expect(
            hasBaseReference({
                ...claude.payload,
                compaction: {
                    ...compactionOf(claude),
                    count_tokens: {
                        url: plantedUrl(),
                    },
                },
            }),
        ).toBe(true);
    });
});
