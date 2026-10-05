import { translation } from "../i18n";
import type { ProviderPreset } from "./types";

/** Complete authored preset for xai. */
export const xaiPreset: ProviderPreset = {
    id: "xai",
    title: "xAI (Grok) Responses API",
    get description() {
        return translation()(
            ($) =>
                $.helpers.presets
                    .theXAIResponsesEndpointWithAStaticKeyNativeCompactionRidesTheStandaloneResponsescompact,
        );
    },
    grant: null,
    suggestedName: "xai",
    provenance:
        "xAI Responses API research (docs.x.ai); compaction facts researched 2026-10-01 — verify against a live compact run before relying on them",
    verification: "unverified",
    payload: {
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
        compaction: {
            strategy: "native_preferred",
            summary_budget: 4096,
            retention_budget: 8192,
            can_export_native_summary: false,
            mechanisms: {
                responses_standalone_compact: {
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
                    },
                },
                responses_explicit_trigger: {
                    default: "unsupported",
                },
                responses_automatic_compaction: {
                    default: "unsupported",
                },
            },
        },
    },
};
