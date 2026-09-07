import type { ClientProtocol, UpstreamProtocol } from "/common/domain";
import { upstreamFailureError } from "/common/errors";
import { unixSeconds } from "/common/validation";
import {
    type GenerateContentStreamState,
    parseGenerateContentResponse,
    parseGenerateContentStreamEvent,
} from "/providers/gemini";
import {
    createChatCompletionsStreamRenderer,
    parseChatCompletionsResponse,
    parseChatCompletionsStreamEvent,
    renderChatCompletionsResponse,
} from "/translation/chat-completions";
import { TranslationError } from "/translation/errors";
import type { IrEvent } from "/translation/ir";
import {
    createResponsesStreamRenderer,
    parseResponsesResponse,
    parseResponsesStreamEvent,
    renderResponsesResponse,
} from "/translation/responses";
import { SseParser } from "/translation/sse";
import { readBeforeIdleTimeout } from "./upstream-response";

function parseUpstreamEvents(protocol: UpstreamProtocol, value: unknown): IrEvent[] {
    if (protocol === "openai_response") return parseResponsesResponse(value);
    if (protocol === "openai_chat_completion") return parseChatCompletionsResponse(value);
    return parseGenerateContentResponse(value);
}

function parseFrameEvents(
    protocol: UpstreamProtocol,
    data: string,
    geminiState: GenerateContentStreamState,
): IrEvent[] {
    // The CC wire sentinel is intercepted here, before JSON decoding, as the single owner of
    // the "[DONE]" terminal-frame rule.
    if (protocol === "openai_chat_completion" && data === "[DONE]") {
        return [
            {
                type: "done",
            },
        ];
    }
    let value: unknown;
    try {
        value = JSON.parse(data);
    } catch {
        throw new TranslationError("upstream_failure", "The upstream returned malformed SSE data");
    }
    try {
        if (protocol === "openai_response") return parseResponsesStreamEvent(value);
        if (protocol === "openai_chat_completion") return parseChatCompletionsStreamEvent(value);
        return parseGenerateContentStreamEvent(value, geminiState);
    } catch {
        throw new TranslationError(
            "upstream_failure",
            "The upstream returned an invalid stream event",
        );
    }
}

export async function translateStreamingResponse(
    upstreamResponse: Response,
    abortController: AbortController,
    upstreamProtocol: UpstreamProtocol,
    clientProtocol: ClientProtocol,
    publicModel: string,
    isUsageIncluded: boolean,
): Promise<Response> {
    if (upstreamResponse.body === null) throw upstreamFailureError();
    const reader = upstreamResponse.body.getReader();
    const firstRead = await readBeforeIdleTimeout(reader, abortController);
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    const renderer =
        clientProtocol === "openai_response"
            ? createResponsesStreamRenderer({
                  created: unixSeconds(),
                  id: `resp_${crypto.randomUUID().replaceAll("-", "")}`,
                  model: publicModel,
              })
            : createChatCompletionsStreamRenderer({
                  created: unixSeconds(),
                  id: `chatcmpl_${crypto.randomUUID().replaceAll("-", "")}`,
                  isUsageIncluded,
                  model: publicModel,
              });
    const geminiState: GenerateContentStreamState = {
        isStarted: false,
        nextToolIndex: 0,
        openToolCallIndexes: new Set(),
    };
    const pendingOutput: Uint8Array[] = [];
    let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
    const parser = new SseParser((frame) => {
        const events = parseFrameEvents(upstreamProtocol, frame.data, geminiState);
        for (const output of renderer(events)) {
            const bytes = encoder.encode(output);
            if (controller === undefined) pendingOutput.push(bytes);
            else controller.enqueue(bytes);
        }
    });

    let nextRead = firstRead;
    while (pendingOutput.length === 0 && !nextRead.done) {
        parser.push(
            decoder.decode(nextRead.value, {
                stream: true,
            }),
        );
        if (pendingOutput.length === 0) {
            nextRead = await readBeforeIdleTimeout(reader, abortController);
        }
    }
    if (pendingOutput.length === 0) {
        parser.push(decoder.decode());
        parser.finish();
    }
    if (pendingOutput.length === 0) {
        reader.releaseLock();
        throw upstreamFailureError();
    }

    let isCanceled = false;
    const body = new ReadableStream<Uint8Array>({
        async start(streamController): Promise<void> {
            controller = streamController;
            for (const bytes of pendingOutput) streamController.enqueue(bytes);
            pendingOutput.length = 0;
            try {
                for (;;) {
                    const result = await readBeforeIdleTimeout(reader, abortController);
                    if (result.done) break;
                    parser.push(
                        decoder.decode(result.value, {
                            stream: true,
                        }),
                    );
                }
                parser.push(decoder.decode());
                parser.finish();
            } catch {
                // Malformed or interrupted upstream streams terminate without a synthetic frame.
            } finally {
                if (!isCanceled) streamController.close();
                reader.releaseLock();
            }
        },
        async cancel(reason): Promise<void> {
            isCanceled = true;
            abortController.abort(reason);
            await reader.cancel(reason);
        },
    });
    return new Response(body, {
        headers: {
            "Content-Type": "text/event-stream",
        },
        status: upstreamResponse.status,
    });
}

export function renderTranslatedNonStreamingResponse(
    bytes: Uint8Array,
    upstreamProtocol: UpstreamProtocol,
    clientProtocol: ClientProtocol,
    publicModel: string,
): Response {
    let value: unknown;
    try {
        const text = new TextDecoder("utf-8", {
            fatal: true,
            ignoreBOM: false,
        }).decode(bytes);
        value = JSON.parse(text);
    } catch {
        throw new TranslationError(
            "upstream_failure",
            "The upstream returned an invalid JSON response",
        );
    }
    let events: IrEvent[];
    try {
        events = parseUpstreamEvents(upstreamProtocol, value);
    } catch {
        throw new TranslationError(
            "upstream_failure",
            "The upstream returned an invalid response object",
        );
    }
    const identifier = crypto.randomUUID().replaceAll("-", "");
    const created = unixSeconds();
    const body =
        clientProtocol === "openai_response"
            ? renderResponsesResponse(events, {
                  created,
                  id: `resp_${identifier}`,
                  model: publicModel,
              })
            : renderChatCompletionsResponse(events, {
                  created,
                  id: `chatcmpl_${identifier}`,
                  model: publicModel,
              });
    return Response.json(body);
}
