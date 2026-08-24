import type { ClientProtocol, UpstreamProtocol } from "/common/domain";
import { upstreamFailureError } from "/common/errors";
import { unixSeconds } from "/common/validation";
import {
    type GenerateContentStreamState,
    parseGenerateContentStreamEvent,
} from "./adapters/gemini";
import {
    parseChatCompletionsStreamEvent,
    renderChatCompletionsStream,
} from "./translation/chat-completions";
import { TranslationError } from "./translation/errors";
import type { IrEvent, IrUsage } from "./translation/ir";
import { createResponsesStreamRenderer, parseResponsesStreamEvent } from "./translation/responses";
import { SseParser } from "./translation/sse";
import { readBeforeIdleTimeout } from "./upstream-response";

interface EventRenderer {
    render(events: readonly IrEvent[]): string[];
}

class ChatCompletionsEventRenderer implements EventRenderer {
    readonly #metadata;
    #hasStarted = false;
    #usage: IrUsage | undefined;

    public constructor(model: string, isUsageIncluded: boolean) {
        this.#metadata = {
            created: unixSeconds(),
            id: `chatcmpl_${crypto.randomUUID().replaceAll("-", "")}`,
            isUsageIncluded,
            model,
        };
    }

    public render(events: readonly IrEvent[]): string[] {
        const immediate: IrEvent[] = [];
        for (const event of events) {
            if (event.type === "message_start") {
                if (!this.#hasStarted) {
                    this.#hasStarted = true;
                    immediate.push(event);
                }
            } else if (event.type === "usage") this.#usage = event.usage;
            else if (event.type === "done") {
                if (this.#usage !== undefined)
                    immediate.push({
                        type: "usage",
                        usage: this.#usage,
                    });
                immediate.push(event);
            } else immediate.push(event);
        }
        return renderChatCompletionsStream(immediate, this.#metadata);
    }
}

function parseFrameEvents(
    protocol: UpstreamProtocol,
    data: string,
    geminiState: GenerateContentStreamState,
): IrEvent[] {
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
    if (upstreamResponse.body === null) throw new Error("Streaming upstream returned no body");
    const reader = upstreamResponse.body.getReader();
    const firstRead = await readBeforeIdleTimeout(reader, abortController);
    const decoder = new TextDecoder();
    const encoder = new TextEncoder();
    const renderer: EventRenderer =
        clientProtocol === "openai_response"
            ? {
                  render: createResponsesStreamRenderer({
                      created: unixSeconds(),
                      id: `resp_${crypto.randomUUID().replaceAll("-", "")}`,
                      model: publicModel,
                  }),
              }
            : new ChatCompletionsEventRenderer(publicModel, isUsageIncluded);
    const geminiState: GenerateContentStreamState = {
        isStarted: false,
        nextToolIndex: 0,
        openToolIndexes: new Map(),
    };
    const pendingOutput: Uint8Array[] = [];
    let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
    const parser = new SseParser((frame) => {
        const events = parseFrameEvents(upstreamProtocol, frame.data, geminiState);
        for (const output of renderer.render(events)) {
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
