import type { ProviderType, UpstreamProtocol } from "/common/domain";

export function providerTypeLabel(type: ProviderType): string {
    if (type === "openai_oauth") return "OpenAI OAuth";
    if (type === "antigravity_oauth") return "Antigravity OAuth";
    return "OpenAI-compatible";
}

export function protocolLabel(protocol: UpstreamProtocol | null): string {
    if (protocol === "openai_chat_completion") return "Chat Completions";
    if (protocol === "openai_response") return "Responses";
    if (protocol === "gemini_generate_content") return "Gemini GenerateContent";
    return "Managed OAuth protocol";
}
