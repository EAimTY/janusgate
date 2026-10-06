import type { Inference, RenderOption, UpstreamStreaming } from "../generated/v0";
import { parseQuotaExhaustion } from "../quota";
import {
    expectArray,
    expectDeclaredKeys,
    expectOneOf,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import { expectParamRows, expectStringExpr, expectTemplateNode } from "./expressions";

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
 * The inference request descriptor (README §5.2), rebuilt with every optional
 * section the operator supplied and `null` normalized to absence.
 */
export function expectInference(value: unknown, context: string): Inference {
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

    const inference: Inference = {
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
        upstream_streaming: expectUpstreamStreaming(
            Reflect.get(record, "upstream_streaming"),
            `${context}.upstream_streaming`,
        ),
    };

    const streamUrl = Reflect.get(record, "stream_url");

    if (isPresentValue(streamUrl)) {
        inference.stream_url = expectStringExpr(streamUrl, `${context}.stream_url`);
    }

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        inference.headers = expectParamRows(headers, `${context}.headers`);
    }

    const renderOptions = Reflect.get(record, "render_options");

    if (renderOptions !== undefined) {
        inference.render_options = expectArray(renderOptions, `${context}.render_options`).map(
            (option, index) => expectRenderOption(option, `${context}.render_options[${index}]`),
        );
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        inference.envelope = expectTemplateNode(envelope, `${context}.envelope`);
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        inference.response_unwrap = expectString(responseUnwrap, `${context}.response_unwrap`);
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
