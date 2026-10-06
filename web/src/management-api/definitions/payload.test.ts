import { describe, expect, it } from "vitest";
import { PROVIDER_PRESETS } from "/src/presets/catalog";
import { parseListProviderDefinitionsResponse, validateDefinitionPayload } from "./payload";

/** One minimal stored definition row every field the app reads, plus its payload. */
const storedDefinition = {
    name: "generic-openai-responses",
    version: 1,
    created_at: 1_700_000_000_000_000,
    format: "openai_response",
    inference: {
        url: [
            {
                type: "base",
            },
            {
                type: "literal",
                value: "/responses",
            },
        ],
        upstream_streaming: "match_client",
    },
    credential: {
        type: "api_key",
    },
    discovery: {
        request: {
            method: "GET",
            url: [
                {
                    type: "base",
                },
                {
                    type: "literal",
                    value: "/models",
                },
            ],
        },
        extract: {
            mode: "array",
            array_at: [
                "data",
            ],
            id_field: [
                "id",
            ],
        },
        pagination: {
            type: "none",
        },
    },
};

describe("validateDefinitionPayload", () => {
    it("preserves ordered exact quota rejection rules and normalizes their status sets", () => {
        const rule = {
            name: "weekly",
            statuses: [
                429,
                402,
                429,
            ],
            code: {
                at: [
                    "error",
                    "code",
                ],
                value: "",
            },
            restriction_key: [
                {
                    type: "literal",
                    value: "account/weekly",
                },
                {
                    type: "requested_model",
                },
            ],
            applies_to: {
                type: "requested_model",
            },
        };
        const quota_exhaustion = {
            rules: [
                rule,
                {
                    ...rule,
                    name: "account",
                    statuses: [
                        403,
                    ],
                    applies_to: {
                        type: "all_models",
                    },
                },
            ],
        };
        const definition = {
            ...storedDefinition,
            inference: {
                ...storedDefinition.inference,
                quota_exhaustion,
            },
        };
        const parsed = validateDefinitionPayload({
            format: definition.format,
            credential: definition.credential,
            discovery: definition.discovery,
            inference: definition.inference,
        });
        if ("error" in parsed) throw new Error(parsed.error);
        expect(parsed.payload.inference.quota_exhaustion).toEqual({
            rules: [
                {
                    ...rule,
                    statuses: [
                        402,
                        429,
                    ],
                },
                quota_exhaustion.rules[1],
            ],
        });
        const rows = parseListProviderDefinitionsResponse({
            provider_definitions: [
                definition,
            ],
        });
        expect(rows.provider_definitions[0]?.inference).toEqual(parsed.payload.inference);
    });

    it("rejects ambiguous or incomplete rejection policies before saving any definition", () => {
        const rule = {
            name: "weekly",
            statuses: [
                429,
            ],
            code: {
                at: [
                    "error",
                    "code",
                ],
                value: "quota",
            },
            restriction_key: [
                {
                    type: "literal",
                    value: "account",
                },
            ],
            applies_to: {
                type: "all_models",
            },
        };
        for (const quota_exhaustion of [
            {
                rules: [],
            },
            {
                rules: [
                    rule,
                    rule,
                ],
            },
            ...[
                {
                    ...rule,
                    name: "",
                },
                {
                    ...rule,
                    statuses: [],
                },
                {
                    ...rule,
                    statuses: [
                        200,
                    ],
                },
                {
                    ...rule,
                    statuses: [
                        99,
                    ],
                },
                {
                    ...rule,
                    statuses: [
                        1000,
                    ],
                },
                {
                    ...rule,
                    statuses: [
                        429.5,
                    ],
                },
                {
                    ...rule,
                    code: {
                        at: [],
                        value: "quota",
                    },
                },
                {
                    ...rule,
                    code: {
                        at: [
                            "",
                        ],
                        value: "quota",
                    },
                },
                {
                    ...rule,
                    code: {
                        at: [
                            "error",
                        ],
                        value: 5,
                    },
                },
                {
                    ...rule,
                    restriction_key: [],
                },
                {
                    ...rule,
                    restriction_key: [
                        {
                            type: "literal",
                            value: "",
                        },
                    ],
                },
                {
                    ...rule,
                    restriction_key: [
                        {
                            type: "requested_model",
                            value: "M",
                        },
                    ],
                },
                {
                    ...rule,
                    applies_to: {
                        type: "models",
                        upstream_ids: [
                            "M",
                        ],
                    },
                },
                {
                    ...rule,
                    regex: "quota",
                },
            ].map((rule) => ({
                rules: [
                    rule,
                ],
            })),
        ]) {
            const parsed = validateDefinitionPayload({
                format: storedDefinition.format,
                credential: storedDefinition.credential,
                inference: {
                    ...storedDefinition.inference,
                    quota_exhaustion,
                },
            });
            expect("error" in parsed).toBe(true);
        }
    });

    it("names each quota rejection reason at its JSON path", () => {
        const rule = {
            name: "weekly",
            statuses: [
                429,
            ],
            code: {
                at: [
                    "error",
                    "code",
                ],
                value: "quota",
            },
            restriction_key: [
                {
                    type: "literal",
                    value: "account",
                },
            ],
            applies_to: {
                type: "all_models",
            },
        };
        const probes: ReadonlyArray<
            [
                unknown,
                RegExp,
            ]
        > = [
            [
                {
                    rules: [],
                },
                /quota_exhaustion\.rules must not be empty/,
            ],
            [
                {
                    rules: [
                        rule,
                        rule,
                    ],
                },
                /rules\[1\]\.name must not repeat an earlier rule name/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            name: "",
                        },
                    ],
                },
                /rules\[0\]\.name must not be empty/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            statuses: [
                                200,
                            ],
                        },
                    ],
                },
                /rules\[0\]\.statuses\[0\] is not a rejected status: 200/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            statuses: [
                                429.5,
                            ],
                        },
                    ],
                },
                /rules\[0\]\.statuses\[0\] must be an integer/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            code: {
                                at: [],
                                value: "quota",
                            },
                        },
                    ],
                },
                /rules\[0\]\.code\.at must not be empty/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            restriction_key: [
                                {
                                    type: "literal",
                                    value: "",
                                },
                            ],
                        },
                    ],
                },
                /restriction_key\[0\]\.value must not be empty/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            applies_to: {
                                type: "models",
                            },
                        },
                    ],
                },
                /applies_to\.type is not a rejection applicability: models/,
            ],
            [
                {
                    rules: [
                        {
                            ...rule,
                            bogus: true,
                        },
                    ],
                },
                /rules\[0\]\.bogus is not a declared field/,
            ],
        ];
        for (const [quota_exhaustion, expected] of probes) {
            const parsed = validateDefinitionPayload({
                format: storedDefinition.format,
                credential: storedDefinition.credential,
                inference: {
                    ...storedDefinition.inference,
                    quota_exhaustion,
                },
            });
            if (!("error" in parsed)) {
                throw new Error(
                    `the payload should have been rejected: ${JSON.stringify(quota_exhaustion)}`,
                );
            }
            expect(parsed.error, JSON.stringify(quota_exhaustion)).toMatch(expected);
        }
    });

    it("accepts every catalog payload verbatim", () => {
        for (const preset of PROVIDER_PRESETS) {
            const result = validateDefinitionPayload(preset.payload);

            if ("error" in result) {
                throw new Error(`${preset.id} was rejected: ${result.error}`);
            }

            expect(result.payload.format, preset.id).toBe(preset.payload.format);
        }
    });

    it("rejects a payload missing its format, naming the path", () => {
        const { format: _format, ...withoutFormat } = PROVIDER_PRESETS[0].payload;
        const result = validateDefinitionPayload(withoutFormat);

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.format is not an upstream format/);
    });

    it("rejects an upstream streaming mode outside its two-value union, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            inference: {
                ...PROVIDER_PRESETS[0].payload.inference,
                upstream_streaming: "sometimes",
            },
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(
            /definition\.inference\.upstream_streaming is not an upstream streaming mode/,
        );
    });

    it("rejects a json template leaf without a JSON value, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            inference: {
                ...PROVIDER_PRESETS[0].payload.inference,
                envelope: {
                    type: "json",
                },
            },
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.inference\.envelope\.value is not a JSON value/);
    });

    it("rejects a field the descriptor does not declare, naming the path", () => {
        const result = validateDefinitionPayload({
            ...PROVIDER_PRESETS[0].payload,
            name: "generic",
        });

        if (!("error" in result)) {
            throw new Error("the payload should have been rejected");
        }

        expect(result.error).toMatch(/definition\.name is not a declared field/);
    });
});

describe("definition compaction validation", () => {
    /** The responses-format payload every arm here builds on. */
    const responsesPayload = PROVIDER_PRESETS[0].payload;

    /** The messages-format payload, for the Messages-side arms. */
    const messagesPayload = PROVIDER_PRESETS.find(
        (candidate) => candidate.id === "generic-anthropic-messages",
    )?.payload;

    if (messagesPayload === undefined) {
        throw new Error("the catalog holds no generic-anthropic-messages preset");
    }

    /** A compact URL expression naming the endpoint literally. */
    const compactUrl = (): [
        {
            type: "literal";
            value: string;
        },
    ] => [
        {
            type: "literal",
            value: "https://upstream.example.com/v1/responses/compact",
        },
    ];

    const standaloneOperation = {
        url: compactUrl(),
        headers: [
            {
                name: "Authorization",
                value: [
                    {
                        type: "literal",
                        value: "Bearer ",
                    },
                    {
                        type: "secret",
                    },
                ],
            },
        ],
        envelope: {
            type: "object",
            fields: {
                body: {
                    type: "body",
                },
            },
        },
        response_unwrap: "result",
        unsupported: {
            statuses: [
                404,
                422,
            ],
            codes: [
                "model_compaction_unsupported",
            ],
            parameters: [
                "model",
            ],
        },
    };

    const betaHeader = (
        value: string,
    ): {
        name: string;
        value: Array<{
            type: "literal";
            value: string;
        }>;
    } => ({
        name: "anthropic-beta",
        value: [
            {
                type: "literal",
                value,
            },
        ],
    });

    it("accepts a responses-format section and fills the omitted members with the shipped defaults", () => {
        const result = validateDefinitionPayload({
            ...responsesPayload,
            compaction: {
                mechanisms: {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: standaloneOperation,
                    },
                    responses_explicit_trigger: {
                        default: "unsupported",
                        unsupported: {
                            codes: [
                                "no_trigger",
                            ],
                        },
                    },
                    responses_automatic_compaction: {
                        default: "unknown",
                    },
                },
            },
        });

        if ("error" in result) {
            throw new Error(`the payload was rejected: ${result.error}`);
        }

        // The omitted members land on the shipped defaults the gateway's
        // serde parse fills, so the rebuilt payload restates the section
        // canonically.
        expect(result.payload.compaction).toEqual({
            strategy: "native_preferred",
            summary_budget: 4096,
            retention_budget: 8192,
            can_export_native_summary: false,
            mechanisms: {
                responses_standalone_compact: {
                    default: "supported",
                    operation: standaloneOperation,
                },
                responses_explicit_trigger: {
                    default: "unsupported",
                    unsupported: {
                        codes: [
                            "no_trigger",
                        ],
                    },
                },
                responses_automatic_compaction: {
                    default: "unknown",
                },
            },
        });
    });

    it("accepts a messages-format section with overlay headers and a token-count operation", () => {
        const result = validateDefinitionPayload({
            ...messagesPayload,
            compaction: {
                strategy: "gateway_summary",
                summary_budget: 2048,
                retention_budget: 4096,
                can_export_native_summary: true,
                mechanisms: {
                    messages_on_demand_compaction: {
                        default: "supported",
                        overlay_headers: [
                            betaHeader("compact-2026-09-04"),
                        ],
                    },
                    messages_threshold_compaction: {
                        default: "supported",
                        overlay_headers: [
                            betaHeader("compact-2026-01-12"),
                        ],
                        unsupported: {
                            parameters: [
                                "context_management",
                            ],
                        },
                    },
                },
                count_tokens: {
                    url: [
                        {
                            type: "literal",
                            value: "https://upstream.example.com/v1/messages/count_tokens",
                        },
                    ],
                    headers: [
                        {
                            name: "x-api-key",
                            value: [
                                {
                                    type: "secret",
                                },
                            ],
                        },
                    ],
                    response_unwrap: "usage",
                },
            },
        });

        if ("error" in result) {
            throw new Error(`the payload was rejected: ${result.error}`);
        }

        expect(result.payload.compaction?.strategy).toBe("gateway_summary");
        expect(result.payload.compaction?.count_tokens?.response_unwrap).toBe("usage");
    });

    it("rejects a mechanism declaration its format can never speak", () => {
        for (const [payload, mechanism] of [
            [
                messagesPayload,
                "responses_standalone_compact",
            ],
            [
                messagesPayload,
                "responses_explicit_trigger",
            ],
            [
                responsesPayload,
                "messages_on_demand_compaction",
            ],
            [
                responsesPayload,
                "messages_threshold_compaction",
            ],
        ] as const) {
            const result = validateDefinitionPayload({
                ...payload,
                compaction: {
                    mechanisms: {
                        [mechanism]:
                            mechanism === "responses_standalone_compact"
                                ? {
                                      default: "supported",
                                      operation: standaloneOperation,
                                  }
                                : {
                                      default: "supported",
                                      overlay_headers: [
                                          betaHeader("compact-2026-09-04"),
                                      ],
                                  },
                    },
                },
            });

            if (!("error" in result)) {
                throw new Error(`${mechanism} should have been rejected on the wrong format`);
            }

            expect(result.error, mechanism).toContain(
                `the \`${mechanism}\` compaction declaration requires`,
            );
        }

        // The token-count operation is Messages-only too, and names itself
        // an operation rather than a declaration.
        const result = validateDefinitionPayload({
            ...responsesPayload,
            compaction: {
                mechanisms: {},
                count_tokens: {
                    url: compactUrl(),
                },
            },
        });

        if (!("error" in result)) {
            throw new Error("count_tokens should have been rejected on the wrong format");
        }

        expect(result.error).toMatch(/the `count_tokens` compaction operation requires/);
    });

    it("rejects a supported declaration without the operation data executing it", () => {
        const withoutOperation = validateDefinitionPayload({
            ...responsesPayload,
            compaction: {
                mechanisms: {
                    responses_standalone_compact: {
                        default: "supported",
                    },
                },
            },
        });

        if (!("error" in withoutOperation)) {
            throw new Error("the operation-less standalone should have been rejected");
        }

        expect(withoutOperation.error).toMatch(/declares `supported` without an operation/);

        const nullOperation = validateDefinitionPayload({
            ...responsesPayload,
            compaction: {
                mechanisms: {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: null,
                    },
                },
            },
        });

        if (!("error" in nullOperation)) {
            throw new Error("the null-operation standalone should have been rejected");
        }

        expect(nullOperation.error).toMatch(/declares `supported` without an operation/);

        for (const mechanism of [
            "messages_on_demand_compaction",
            "messages_threshold_compaction",
        ] as const) {
            const withoutHeaders = validateDefinitionPayload({
                ...messagesPayload,
                compaction: {
                    mechanisms: {
                        [mechanism]: {
                            default: "supported",
                        },
                    },
                },
            });

            if (!("error" in withoutHeaders)) {
                throw new Error(`${mechanism} should have been rejected without headers`);
            }

            expect(withoutHeaders.error, mechanism).toContain(
                `the \`${mechanism}\` compaction declaration declares`,
            );

            const emptyHeaders = validateDefinitionPayload({
                ...messagesPayload,
                compaction: {
                    mechanisms: {
                        [mechanism]: {
                            default: "supported",
                            overlay_headers: [],
                        },
                    },
                },
            });

            if (!("error" in emptyHeaders)) {
                throw new Error(`${mechanism} should have been rejected on empty headers`);
            }

            expect(emptyHeaders.error, mechanism).toMatch(/without overlay headers/);
        }
    });

    it("rejects a section member outside its closed vocabulary or positive range", () => {
        // Each probe is one whole compaction-section value, so a probe can
        // name the mechanisms object itself rather than only its siblings.
        const probes: ReadonlyArray<
            [
                unknown,
                RegExp,
            ]
        > = [
            [
                {
                    strategy: "native",
                    mechanisms: {},
                },
                /definition\.compaction\.strategy is not a compaction strategy/,
            ],
            [
                {
                    summary_budget: 0,
                    mechanisms: {},
                },
                /definition\.compaction\.summary_budget must be a positive integer/,
            ],
            [
                {
                    summary_budget: -4096,
                    mechanisms: {},
                },
                /definition\.compaction\.summary_budget must be a positive integer/,
            ],
            [
                {
                    summary_budget: 40.5,
                    mechanisms: {},
                },
                /definition\.compaction\.summary_budget must be an integer/,
            ],
            [
                {
                    retention_budget: "many",
                    mechanisms: {},
                },
                /definition\.compaction\.retention_budget must be an integer/,
            ],
            [
                {
                    can_export_native_summary: "yes",
                    mechanisms: {},
                },
                /definition\.compaction\.can_export_native_summary must be a boolean/,
            ],
            [
                {
                    mechanisms: {},
                    bogus: true,
                },
                /definition\.compaction\.bogus is not a declared field/,
            ],
            [
                {
                    mechanisms: {
                        bogus_mechanism: {},
                    },
                },
                /mechanisms\.bogus_mechanism is not a declared field/,
            ],
        ];

        for (const [compaction, expected] of probes) {
            const result = validateDefinitionPayload({
                ...responsesPayload,
                compaction: compaction as Record<string, unknown>,
            });

            if (!("error" in result)) {
                throw new Error(
                    `the payload should have been rejected: ${JSON.stringify(compaction)}`,
                );
            }

            expect(result.error, JSON.stringify(compaction)).toMatch(expected);
        }
    });

    it("rejects a declaration or operation member outside its shape", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    responses_explicit_trigger: {
                        default: "supportedish",
                    },
                },
                /default is not a mechanism support state/,
            ],
            [
                {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: {
                            url: [],
                        },
                    },
                },
                /operation\.url must carry at least one expression part/,
            ],
            [
                {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: {
                            url: compactUrl(),
                            envelope: {
                                type: "object",
                                fields: {},
                            },
                        },
                    },
                },
                /envelope carries 0 body placeholders; exactly one is required/,
            ],
            [
                {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: {
                            url: compactUrl(),
                            envelope: {
                                type: "object",
                                fields: {
                                    one: {
                                        type: "body",
                                    },
                                    two: {
                                        type: "body",
                                    },
                                },
                            },
                        },
                    },
                },
                /envelope carries 2 body placeholders; exactly one is required/,
            ],
            [
                {
                    responses_standalone_compact: {
                        default: "supported",
                        operation: {
                            url: compactUrl(),
                            response_unwrap: "",
                        },
                    },
                },
                /operation\.response_unwrap must not be empty/,
            ],
            [
                {
                    responses_standalone_compact: {
                        default: "unknown",
                        unsupported: {
                            codes: [
                                "",
                            ],
                        },
                    },
                },
                /must not be an empty code/,
            ],
            [
                {
                    messages_on_demand_compaction: {
                        default: "unknown",
                        unsupported: {
                            parameters: [
                                "",
                            ],
                        },
                    },
                },
                /must not be an empty parameter/,
            ],
            [
                {
                    messages_threshold_compaction: {
                        default: "unknown",
                        unsupported: {
                            statuses: [
                                404.5,
                            ],
                        },
                    },
                },
                /statuses\[0\] must be an integer/,
            ],
            [
                {
                    responses_automatic_compaction: {
                        default: "unknown",
                        unsupported: {
                            statuses: [
                                65_536,
                            ],
                        },
                    },
                },
                /statuses\[0\] is not an HTTP status/,
            ],
            [
                {
                    messages_on_demand_compaction: {
                        default: "unknown",
                        unsupported: {
                            bogus: true,
                        },
                    },
                },
                /unsupported\.bogus is not a declared field/,
            ],
        ];

        for (const [mechanisms, expected] of probes) {
            // Every probe rides whichever format its mechanism is legal on:
            // the responses mechanisms sit on the responses payload and the
            // Messages mechanisms on the messages payload.
            const isMessages = JSON.stringify(mechanisms).includes("messages_");
            const result = validateDefinitionPayload({
                ...(isMessages ? messagesPayload : responsesPayload),
                compaction: {
                    mechanisms: mechanisms as Record<string, unknown>,
                },
            });

            if (!("error" in result)) {
                throw new Error(
                    `the payload should have been rejected: ${JSON.stringify(mechanisms)}`,
                );
            }

            expect(result.error, JSON.stringify(mechanisms)).toMatch(expected);
        }
    });

    it("carries the compaction section of a stored row through the response parse", () => {
        const rows = parseListProviderDefinitionsResponse({
            provider_definitions: [
                {
                    ...storedDefinition,
                    compaction: {
                        mechanisms: {
                            responses_standalone_compact: {
                                default: "supported",
                                operation: {
                                    url: compactUrl(),
                                },
                            },
                        },
                    },
                },
            ],
        });

        expect(rows.provider_definitions[0]?.compaction).toEqual({
            strategy: "native_preferred",
            summary_budget: 4096,
            retention_budget: 8192,
            can_export_native_summary: false,
            mechanisms: {
                responses_standalone_compact: {
                    default: "supported",
                    operation: {
                        url: compactUrl(),
                    },
                },
            },
        });
    });
});

describe("definition response validation", () => {
    it("parses the stored listing and the envelope, building every row", () => {
        expect(
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    storedDefinition,
                ],
            }),
        ).toEqual({
            provider_definitions: [
                storedDefinition,
            ],
        });
    });

    it("rejects a stored row missing a field the app reads", () => {
        const { version: _version, ...withoutVersion } = storedDefinition;

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    withoutVersion,
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.version must be an integer/);

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        version: 0,
                    },
                ],
            }),
        ).toThrow(
            /response\.provider_definitions\[0\]\.version must be a safe integer of at least 1/,
        );

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        created_at: 2 ** 53,
                    },
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.created_at must be a safe integer/);
    });

    it("rejects a stored row carrying a member the row does not declare", () => {
        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        future_row_field: true,
                    },
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.future_row_field is not a declared field/);
    });

    it("rejects a stored payload that breaks the descriptor contract", () => {
        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        inference: {
                            ...storedDefinition.inference,
                            upstream_streaming: "sometimes",
                        },
                    },
                ],
            }),
        ).toThrow(
            /response\.provider_definitions\[0\]\.inference\.upstream_streaming is not an upstream streaming mode/,
        );

        expect(() =>
            parseListProviderDefinitionsResponse({
                provider_definitions: [
                    {
                        ...storedDefinition,
                        credential: {
                            type: "oauth",
                        },
                    },
                ],
            }),
        ).toThrow(/response\.provider_definitions\[0\]\.credential\.client_id must be a string/);
    });
});

describe("definition write-boundary mirror", () => {
    /** The responses-format `api_key` payload the structural probes graft onto. */
    const responsesPayload = structuredClone(PROVIDER_PRESETS[0].payload);

    /** The devin payload: the OAuth recipe the credential probes graft onto. */
    const devinPreset = PROVIDER_PRESETS.find((candidate) => candidate.id === "devin")?.payload;

    if (devinPreset === undefined) {
        throw new Error("the catalog holds no devin preset");
    }

    const oauthPayload = structuredClone(devinPreset);
    const oauthCredential = structuredClone(devinPreset.credential);

    if (oauthCredential.type !== "oauth") {
        throw new Error("the devin preset carries no OAuth recipe");
    }

    const oauthGrant = oauthCredential.grant;

    if (oauthGrant.type !== "authorization_code") {
        throw new Error("the devin preset carries no authorization-code grant");
    }

    const responsesDiscovery = responsesPayload.discovery;

    if (responsesDiscovery === undefined || responsesDiscovery === null) {
        throw new Error("the responses preset carries no discovery");
    }

    /** The chat-completion payload, for the per-format render-option probes. */
    const chatCompletionPreset = PROVIDER_PRESETS.find(
        (candidate) => candidate.id === "generic-openai-chat-completions",
    )?.payload;

    if (chatCompletionPreset === undefined) {
        throw new Error("the catalog holds no generic-openai-chat-completions preset");
    }

    const chatCompletionPayload = structuredClone(chatCompletionPreset);

    /** One literal expression part. */
    const literal = (
        value: string,
    ): {
        type: "literal";
        value: string;
    } => ({
        type: "literal",
        value,
    });

    /** Validates one grafted payload, returning the rejection message it names. */
    const reject = (payload: unknown): string => {
        const result = validateDefinitionPayload(payload);

        if (!("error" in result)) {
            throw new Error(`the payload should have been rejected: ${JSON.stringify(payload)}`);
        }

        return result.error;
    };

    it("names the expression walk's structural rules at their JSON paths", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        url: [],
                    },
                },
                /definition\.inference\.url must carry at least one expression part/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        url: [
                            {
                                type: "coalesce",
                                parts: [],
                            },
                        ],
                    },
                },
                /definition\.inference\.url\[0\] carries an empty coalesce/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        url: [
                            {
                                type: "coalesce",
                                parts: [
                                    {
                                        type: "coalesce",
                                        parts: [
                                            literal("fallback"),
                                        ],
                                    },
                                ],
                            },
                        ],
                    },
                },
                /definition\.inference\.url\[0\]\.parts\[0\] nests a coalesce inside a coalesce/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        headers: [
                            {
                                name: "",
                                value: [
                                    literal("x"),
                                ],
                            },
                        ],
                    },
                },
                /definition\.inference\.headers\[0\]\.name must not be empty/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        headers: [
                            {
                                name: "bad header",
                                value: [
                                    literal("x"),
                                ],
                            },
                        ],
                    },
                },
                /definition\.inference\.headers\[0\]\.name is not a valid HTTP header name/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        envelope: {
                            type: "object",
                            fields: {
                                "": {
                                    type: "body",
                                },
                            },
                        },
                    },
                },
                /definition\.inference\.envelope\.fields carries an empty object member name/,
            ],
        ];

        for (const [payload, expected] of probes) {
            expect(reject(payload), String(expected)).toMatch(expected);
        }
    });

    it("holds the inference section to the gateway's write boundary", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        stream_url: [],
                    },
                },
                /definition\.inference\.stream_url must carry at least one expression part/,
            ],
            [
                {
                    ...responsesPayload,
                    inference: {
                        ...responsesPayload.inference,
                        envelope: {
                            type: "object",
                            fields: {},
                        },
                    },
                },
                /definition\.inference\.envelope carries 0 body placeholders; exactly one is required/,
            ],
            [
                {
                    ...chatCompletionPayload,
                    inference: {
                        ...chatCompletionPayload.inference,
                        render_options: [
                            "omit_max_output_tokens",
                        ],
                    },
                },
                /definition\.inference\.render_options\[0\] is not part of the `openai_chat_completion` format's set/,
            ],
        ];

        for (const [payload, expected] of probes) {
            expect(reject(payload), String(expected)).toMatch(expected);
        }

        // The one render option sits inside its own format's set.
        const accepted = validateDefinitionPayload({
            ...responsesPayload,
            inference: {
                ...responsesPayload.inference,
                render_options: [
                    "omit_max_output_tokens",
                ],
            },
        });

        expect("error" in accepted).toBe(false);
    });

    it("holds every stage's control request to the shared shape rules", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesDiscovery,
                        request: {
                            ...responsesDiscovery.request,
                            url: [],
                        },
                    },
                },
                /definition\.discovery\.request\.url must carry at least one expression part/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesDiscovery,
                        request: {
                            method: "GET",
                            url: [
                                {
                                    type: "base",
                                },
                            ],
                            body: {
                                kind: "json",
                                node: {
                                    type: "json",
                                    value: 1,
                                },
                            },
                        },
                    },
                },
                /definition\.discovery\.request must not carry a body on a GET request/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesDiscovery,
                        request: {
                            method: "POST",
                            url: [
                                {
                                    type: "base",
                                },
                            ],
                            body: {
                                kind: "json",
                                node: {
                                    type: "body",
                                },
                            },
                        },
                    },
                },
                /definition\.discovery\.request\.body must not carry the inference body placeholder/,
            ],
        ];

        for (const [payload, expected] of probes) {
            expect(reject(payload), String(expected)).toMatch(expected);
        }
    });

    it("holds the discovery extraction and pagination to their shapes", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesDiscovery,
                        extract: {
                            ...responsesDiscovery.extract,
                            id_field: [],
                        },
                    },
                },
                /definition\.discovery\.extract\.id_field must not be empty/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesDiscovery,
                        extract: {
                            ...responsesDiscovery.extract,
                            strip_prefix: "",
                        },
                    },
                },
                /definition\.discovery\.extract\.strip_prefix must not be empty/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesPayload.discovery,
                        pagination: {
                            type: "token",
                            request_param: "",
                            response_path: [
                                "data",
                            ],
                            max_pages: 5,
                        },
                    },
                },
                /definition\.discovery\.pagination\.request_param must not be empty/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesPayload.discovery,
                        pagination: {
                            type: "token",
                            request_param: "after",
                            response_path: [],
                            max_pages: 5,
                        },
                    },
                },
                /definition\.discovery\.pagination\.response_path must not be empty/,
            ],
            [
                {
                    ...responsesPayload,
                    discovery: {
                        ...responsesPayload.discovery,
                        pagination: {
                            type: "token",
                            request_param: "after",
                            response_path: [
                                "data",
                            ],
                            max_pages: 0,
                        },
                    },
                },
                /definition\.discovery\.pagination\.max_pages must be a positive integer/,
            ],
        ];

        for (const [payload, expected] of probes) {
            expect(reject(payload), String(expected)).toMatch(expected);
        }
    });

    it("holds the OAuth recipe to the credential write rules", () => {
        const probes: ReadonlyArray<
            [
                object,
                RegExp,
            ]
        > = [
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        client_id: "",
                    },
                },
                /definition\.credential\.client_id must not be empty/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        grant: {
                            ...oauthGrant,
                            authorize_url: [],
                        },
                    },
                },
                /definition\.credential\.grant\.authorize_url must carry at least one expression part/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        grant: {
                            ...oauthGrant,
                            redirect_uri: "",
                        },
                    },
                },
                /definition\.credential\.grant\.redirect_uri must not be empty/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        exchange: {
                            ...oauthCredential.exchange,
                            terminal_codes: [],
                        },
                    },
                },
                /definition\.credential\.exchange\.terminal_codes must not be empty/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        exchange: {
                            ...oauthCredential.exchange,
                            terminal_codes: [
                                "",
                            ],
                        },
                    },
                },
                /definition\.credential\.exchange\.terminal_codes\[0\] must not be an empty code/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        access_token_lifetime_seconds: 0,
                    },
                },
                /definition\.credential\.access_token_lifetime_seconds must be a positive integer/,
            ],
            [
                {
                    ...oauthPayload,
                    credential: {
                        ...oauthCredential,
                        exchange: {
                            ...oauthCredential.exchange,
                            response: {
                                encoding: "form",
                                fields: oauthCredential.exchange.response.fields.map(
                                    (field, index) =>
                                        index === 0
                                            ? {
                                                  ...field,
                                                  path: [
                                                      "api_key",
                                                      "nested",
                                                  ],
                                              }
                                            : field,
                                ),
                            },
                        },
                    },
                },
                /definition\.credential\.exchange\.response paths address flat form fields and carry exactly one member/,
            ],
        ];

        for (const [payload, expected] of probes) {
            expect(reject(payload), String(expected)).toMatch(expected);
        }
    });

    it("requires provisioning to be a non-empty OAuth-only sequence", () => {
        const step = {
            request: {
                method: "POST",
                url: [
                    literal("https://upstream.example.com/provision"),
                ],
            },
            response_encoding: "json",
        };

        expect(
            reject({
                ...responsesPayload,
                provisioning: [
                    step,
                ],
            }),
        ).toMatch(
            /definition\.provisioning requires an OAuth credential: it runs after the token exchange/,
        );

        expect(
            reject({
                ...oauthPayload,
                provisioning: [],
            }),
        ).toMatch(/definition\.provisioning must carry at least one step when present/);
    });
});
