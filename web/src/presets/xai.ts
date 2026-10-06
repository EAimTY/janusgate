import { translation } from "../i18n";
import { base, bearerAuthorization, literal, openAiModelsDiscovery } from "./expressions";
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
                base(),
                literal("/responses"),
            ],
            headers: [
                bearerAuthorization(),
            ],
            upstream_streaming: "match_client",
        },
        credential: {
            type: "api_key",
        },
        discovery: openAiModelsDiscovery([
            base(),
            literal("/models"),
        ]),
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
                            base(),
                            literal("/responses/compact"),
                        ],
                        headers: [
                            bearerAuthorization(),
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
