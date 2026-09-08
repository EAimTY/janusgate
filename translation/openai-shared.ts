import { TranslationError } from "./errors";
import type { IrFunctionTool, IrResponseFormat, IrToolChoice, IrUsage } from "./ir";
import {
    expectJsonObject,
    expectNonEmptyString,
    expectRecord,
    optionalBoolean,
    optionalNonNegativeInteger,
    optionalString,
} from "./json";

// Helpers shared by the two OpenAI outward dialects (Chat Completions and Responses),
// parameterized by the wire field names on which the dialects differ.

// Wire field names of a usage object that the dialects spell differently;
// `total_tokens` and the `*_details` inner fields are shared verbatim.
export interface UsageFieldNames {
    readonly inputTokens: string;
    readonly outputTokens: string;
    // Error-message context prefix for the `*_details` records. Chat scopes them under the
    // usage object (`usage.prompt_tokens_details`) while Responses names them bare
    // (`input_tokens_details`); the inner `cached_tokens`/`reasoning_tokens` contexts are
    // unprefixed in both dialects.
    readonly detailsContextPrefix: "" | "usage.";
}

// The usage object shape common to both dialects, keyed by the dialect's field names
// (`prompt_tokens`/`completion_tokens` vs `input_tokens`/`output_tokens`).
export type WireUsage<TFieldNames extends UsageFieldNames> = {
    [TField in TFieldNames["inputTokens"] | TFieldNames["outputTokens"] | "total_tokens"]: number;
} & {
    [TField in `${TFieldNames["inputTokens"]}_details`]?: {
        cached_tokens: number;
    };
} & {
    [TField in `${TFieldNames["outputTokens"]}_details`]?: {
        reasoning_tokens: number;
    };
};

export function parseToolChoice(
    value: unknown,
    readFunctionName: (choice: Record<string, unknown>) => string,
): IrToolChoice | undefined {
    if (value === undefined || value === "auto")
        return value === undefined
            ? undefined
            : {
                  mode: "auto",
              };
    if (value === "none" || value === "required")
        return {
            mode: value,
        };
    const choice = expectRecord(value, "tool_choice");
    if (choice["type"] !== "function") {
        throw new TranslationError("unsupported_parameter", "unsupported tool_choice");
    }
    return {
        mode: "required",
        functionName: readFunctionName(choice),
    };
}

export function validateToolCalling(
    tools: readonly IrFunctionTool[],
    toolChoice: IrToolChoice | undefined,
    parallelToolCalls: unknown,
): void {
    if (
        toolChoice?.functionName !== undefined &&
        !tools.some((tool) => tool.name === toolChoice.functionName)
    ) {
        throw new TranslationError(
            "unsupported_parameter",
            "tool_choice names an undeclared function",
        );
    }
    if (parallelToolCalls !== undefined && typeof parallelToolCalls !== "boolean") {
        throw new TranslationError("invalid_json", "parallel_tool_calls must be a boolean");
    }
    if (parallelToolCalls === false) {
        throw new TranslationError(
            "unsupported_parameter",
            "parallel_tool_calls:false is not supported on translated paths",
        );
    }
}

// Where a `json_schema` payload lives inside a response-format object: Chat Completions
// nests it under a `json_schema` record, Responses carries the fields on the format
// object itself. The reader returns that record together with the error-message context
// prefix its fields are validated under.
interface WireFormatSchema {
    readonly record: Record<string, unknown>;
    readonly context: string;
}

export function parseResponseFormat(
    value: unknown,
    context: string,
    readSchema: (format: Record<string, unknown>) => WireFormatSchema,
): IrResponseFormat {
    const format = expectRecord(value, context);
    if (format["type"] === "json_object") {
        return {
            type: "json_object",
        };
    }
    if (format["type"] !== "json_schema") {
        throw new TranslationError("unsupported_parameter", `unsupported ${context}.type`);
    }
    const schema = readSchema(format);
    const result: IrResponseFormat = {
        type: "json_schema",
        name: expectNonEmptyString(schema.record["name"], `${schema.context}.name`),
        schema: expectJsonObject(schema.record["schema"], `${schema.context}.schema`),
    };
    const description = optionalString(
        schema.record["description"],
        `${schema.context}.description`,
    );
    const isStrict = optionalBoolean(schema.record["strict"], `${schema.context}.strict`);
    if (description !== undefined) result["description"] = description;
    if (isStrict !== undefined) result.isStrict = isStrict;
    return result;
}

export function parseUsage(value: unknown, fieldNames: UsageFieldNames): IrUsage {
    const usage = expectRecord(value, "usage");
    const result: IrUsage = {
        inputTokens:
            optionalNonNegativeInteger(
                usage[fieldNames.inputTokens],
                `usage.${fieldNames.inputTokens}`,
            ) ?? 0,
        outputTokens:
            optionalNonNegativeInteger(
                usage[fieldNames.outputTokens],
                `usage.${fieldNames.outputTokens}`,
            ) ?? 0,
        totalTokens: optionalNonNegativeInteger(usage["total_tokens"], "usage.total_tokens") ?? 0,
    };
    const inputDetailsField = `${fieldNames.inputTokens}_details`;
    if (usage[inputDetailsField] !== undefined) {
        const details = expectRecord(
            usage[inputDetailsField],
            `${fieldNames.detailsContextPrefix}${inputDetailsField}`,
        );
        const cached = optionalNonNegativeInteger(details["cached_tokens"], "cached_tokens");
        if (cached !== undefined) result.cachedInputTokens = cached;
    }
    const outputDetailsField = `${fieldNames.outputTokens}_details`;
    if (usage[outputDetailsField] !== undefined) {
        const details = expectRecord(
            usage[outputDetailsField],
            `${fieldNames.detailsContextPrefix}${outputDetailsField}`,
        );
        const reasoning = optionalNonNegativeInteger(
            details["reasoning_tokens"],
            "reasoning_tokens",
        );
        if (reasoning !== undefined) result.reasoningTokens = reasoning;
    }
    return result;
}

export function renderUsage<const TFieldNames extends UsageFieldNames>(
    usage: IrUsage,
    fieldNames: TFieldNames,
): WireUsage<TFieldNames> {
    const result: Record<string, unknown> = {
        [fieldNames.inputTokens]: usage.inputTokens,
        [fieldNames.outputTokens]: usage.outputTokens,
        total_tokens: usage.totalTokens,
    };
    if (usage.cachedInputTokens !== undefined) {
        result[`${fieldNames.inputTokens}_details`] = {
            cached_tokens: usage.cachedInputTokens,
        };
    }
    if (usage.reasoningTokens !== undefined) {
        result[`${fieldNames.outputTokens}_details`] = {
            reasoning_tokens: usage.reasoningTokens,
        };
    }
    // The keys are built from exactly the field names that parameterize `WireUsage`, so
    // the record is a `WireUsage<TFieldNames>`; the assertion only bridges TypeScript's
    // inability to check computed keys against mapped types.
    return result as WireUsage<TFieldNames>;
}
