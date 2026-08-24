import type { ClientProtocol, UpstreamProtocol } from "/common/domain";
import { unixSeconds } from "/common/validation";
import { parseGenerateContentResponse } from "./adapters/gemini";
import {
    parseChatCompletionsResponse,
    renderChatCompletionsResponse,
} from "./translation/chat-completions";
import { TranslationError } from "./translation/errors";
import type { IrEvent } from "./translation/ir";
import { parseResponsesResponse, renderResponsesResponse } from "./translation/responses";

function parseUpstreamEvents(protocol: UpstreamProtocol, value: unknown): IrEvent[] {
    if (protocol === "openai_response") return parseResponsesResponse(value);
    if (protocol === "openai_chat_completion") return parseChatCompletionsResponse(value);
    return parseGenerateContentResponse(value);
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
