import { unixSeconds } from "/common/validation";
import { parseCodeAssistResponse } from "./adapters/antigravity";
import type { ClientProtocol } from "./request-validation";
import {
    parseChatCompletionsResponse,
    renderChatCompletionsResponse,
} from "./translation/chat-completions";
import { TranslationError } from "./translation/errors";
import type { IrEvent } from "./translation/ir";
import { parseResponsesResponse, renderResponsesResponse } from "./translation/responses";
import type { UpstreamProtocol } from "./upstream-request";

function parseUpstreamEvents(protocol: UpstreamProtocol, value: unknown): IrEvent[] {
    if (protocol === "responses") return parseResponsesResponse(value);
    if (protocol === "chat_completions") return parseChatCompletionsResponse(value);
    return parseCodeAssistResponse(value);
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
        clientProtocol === "responses"
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
