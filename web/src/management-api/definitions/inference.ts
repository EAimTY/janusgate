import { ContractViolationError } from "../errors";
import type { Inference, RenderOption, UpstreamFormat, UpstreamStreaming } from "../generated/v0";
import { parseQuotaExhaustion } from "../quota";
import {
    expectArray,
    expectDeclaredKeys,
    expectNonEmptyString,
    expectOneOf,
    expectRecord,
    isPresentValue,
} from "../validation-primitives";
import {
    countTemplateBodyNodes,
    expectHeaderRows,
    expectStringExpr,
    expectTemplateNode,
} from "./expressions";

/** The upstream streaming mode a definition pins (README §5.2). */
function expectUpstreamStreaming(value: unknown, context: string): UpstreamStreaming {
    return expectOneOf(
        value,
        [
            "always",
            "match_client",
        ],
        context,
        "an upstream streaming mode",
    );
}

/** The closed render-option vocabulary of the format codecs (README §5.2). */
function expectRenderOption(value: unknown, context: string): RenderOption {
    return expectOneOf(
        value,
        [
            "omit_max_output_tokens",
        ],
        context,
        "a render option",
    );
}

/**
 * The per-format closed render-option sets, the wire mirror of the gateway's
 * codec sets (README §5.2): a codec rejects options outside its set at write
 * time. The record's type is exhaustive, so a format added to the generated
 * union fails to compile until its set is stated here.
 */
const ALLOWED_RENDER_OPTIONS: Readonly<Record<UpstreamFormat, ReadonlyArray<RenderOption>>> = {
    openai_response: [
        "omit_max_output_tokens",
    ],
    openai_chat_completion: [],
    gemini_generate_content: [],
    gemini_interaction: [],
    anthropic_messages: [],
};

/**
 * The inference request descriptor (README §5.2), rebuilt with every optional
 * section the operator supplied and `null` normalized to absence. The walk
 * holds the descriptor to the gateway's write boundary: non-empty URLs, the
 * render options of the definition's own format's set, the envelope carrying
 * exactly one body placeholder, and a non-empty response-unwrap member.
 */
export function expectInference(
    value: unknown,
    context: string,
    format: UpstreamFormat,
): Inference {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "url",
            "stream_url",
            "headers",
            "upstream_streaming",
            "render_options",
            "envelope",
            "response_unwrap",
            "quota_exhaustion",
        ],
        context,
    );

    const url = expectStringExpr(Reflect.get(record, "url"), `${context}.url`);

    if (url.length === 0) {
        throw new ContractViolationError(`${context}.url must carry at least one expression part`);
    }

    const inference: Inference = {
        url,
        upstream_streaming: expectUpstreamStreaming(
            Reflect.get(record, "upstream_streaming"),
            `${context}.upstream_streaming`,
        ),
    };

    const streamUrl = Reflect.get(record, "stream_url");

    if (isPresentValue(streamUrl)) {
        const streamUrlExpression = expectStringExpr(streamUrl, `${context}.stream_url`);

        if (streamUrlExpression.length === 0) {
            throw new ContractViolationError(
                `${context}.stream_url must carry at least one expression part`,
            );
        }

        inference.stream_url = streamUrlExpression;
    }

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        inference.headers = expectHeaderRows(headers, `${context}.headers`);
    }

    const renderOptions = Reflect.get(record, "render_options");

    if (renderOptions !== undefined) {
        inference.render_options = expectArray(renderOptions, `${context}.render_options`).map(
            (option, index): RenderOption => {
                const rendered = expectRenderOption(option, `${context}.render_options[${index}]`);

                if (!ALLOWED_RENDER_OPTIONS[format].includes(rendered)) {
                    throw new ContractViolationError(
                        `${context}.render_options[${index}] is not part of the ` +
                            `\`${format}\` format's set: ${rendered}`,
                    );
                }

                return rendered;
            },
        );
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        const node = expectTemplateNode(envelope, `${context}.envelope`);
        const bodyCount = countTemplateBodyNodes(node);

        if (bodyCount !== 1) {
            throw new ContractViolationError(
                `${context}.envelope carries ${bodyCount} body placeholders; exactly one is required`,
            );
        }

        inference.envelope = node;
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        inference.response_unwrap = expectNonEmptyString(
            responseUnwrap,
            `${context}.response_unwrap`,
        );
    }

    const quotaExhaustion = Reflect.get(record, "quota_exhaustion");
    if (isPresentValue(quotaExhaustion)) {
        inference.quota_exhaustion = parseQuotaExhaustion(
            quotaExhaustion,
            `${context}.quota_exhaustion`,
        );
    }

    return inference;
}
