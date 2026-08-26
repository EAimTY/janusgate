import type { JsonObject, JsonValue } from "./json";

export type MessageRole = "system" | "user" | "assistant" | "tool";
export type FinishReason = "stop" | "length" | "tool_calls" | "content_filter";

export interface TextPart {
    type: "text";
    text: string;
    thoughtSignature?: string;
}

export interface ImagePart {
    type: "image";
    url: string;
    detail?: "auto" | "low" | "high";
}

export interface ToolCallPart {
    type: "tool_call";
    id: string;
    name: string;
    arguments: string;
    thoughtSignature?: string;
}

export interface ToolOutputPart {
    type: "tool_output";
    callId: string;
    output: JsonValue;
    name?: string;
    thoughtSignature?: string;
}

export type MessagePart = TextPart | ImagePart | ToolCallPart | ToolOutputPart;

export interface IrMessage {
    role: MessageRole;
    parts: MessagePart[];
    reasoning?: string;
    reasoningThoughtSignature?: string;
}

export interface IrFunctionTool {
    name: string;
    description?: string;
    parameters: JsonObject;
    isStrict?: boolean;
}

export interface IrResponseFormatJsonObject {
    type: "json_object";
}

export interface IrResponseFormatJsonSchema {
    type: "json_schema";
    name: string;
    schema: JsonObject;
    description?: string;
    isStrict?: boolean;
}

export type IrResponseFormat = IrResponseFormatJsonObject | IrResponseFormatJsonSchema;

export interface IrSampling {
    temperature?: number;
    topP?: number;
    maxOutputTokens?: number;
    stop?: string[];
    responseFormat?: IrResponseFormat;
}

export interface IrToolChoice {
    mode: "auto" | "none" | "required";
    functionName?: string;
}

export interface IrRequest {
    messages: IrMessage[];
    tools: IrFunctionTool[];
    toolChoice?: IrToolChoice;
    sampling: IrSampling;
    reasoningEffort?: string;
    isStreaming: boolean;
}

export interface IrUsage {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
    cachedInputTokens?: number;
    reasoningTokens?: number;
}

export type IrEvent =
    | {
          type: "message_start";
          id?: string;
      }
    | {
          type: "text_delta";
          delta: string;
          thoughtSignature?: string;
      }
    | {
          type: "tool_call_start";
          index: number;
          id: string;
          name: string;
          thoughtSignature?: string;
      }
    | {
          type: "tool_call_delta";
          index: number;
          delta: string;
      }
    | {
          type: "tool_call_done";
          index: number;
      }
    | {
          type: "reasoning_delta";
          delta: string;
          thoughtSignature?: string;
      }
    | {
          type: "finish";
          reason: FinishReason;
      }
    | {
          type: "usage";
          usage: IrUsage;
      }
    | {
          type: "done";
      };
