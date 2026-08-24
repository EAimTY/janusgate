import { CANONICAL_THINKING_LEVELS, type ThinkingLevel } from "/config";

export const thinkingLevels = CANONICAL_THINKING_LEVELS;
export type { ThinkingLevel };

export interface ModelNamePreviewInput {
    modelPrefix: string;
    upstreamName: string;
    baseName: string;
    existingName: string | null;
}

export function composeModelName(modelPrefix: string, baseName: string): string {
    return `${modelPrefix}${baseName}`;
}

export function previewModelName(input: ModelNamePreviewInput): string {
    if (input.baseName.length > 0) return composeModelName(input.modelPrefix, input.baseName);
    if (input.existingName !== null) return input.existingName;
    return composeModelName(input.modelPrefix, input.upstreamName);
}

export function isThinkingLevel(value: unknown): value is ThinkingLevel {
    return typeof value === "string" && thinkingLevels.some((level) => level === value);
}

export function toggleThinkingLevel(
    selectedLevels: readonly ThinkingLevel[],
    level: ThinkingLevel,
    isSelected: boolean,
): ThinkingLevel[] {
    const nextLevels = new Set(selectedLevels);
    if (isSelected) nextLevels.add(level);
    else nextLevels.delete(level);
    return thinkingLevels.filter((candidate) => nextLevels.has(candidate));
}
