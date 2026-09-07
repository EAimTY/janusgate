import { CANONICAL_THINKING_LEVELS, type ThinkingLevel } from "/config";

export const thinkingLevels = CANONICAL_THINKING_LEVELS;
export { isThinkingLevel } from "/common/validation";
export type { ThinkingLevel };

interface ModelNamePreviewInput {
    modelPrefix: string;
    upstreamName: string;
    baseName: string;
}

export function composeModelName(modelPrefix: string, baseName: string): string {
    return `${modelPrefix}${baseName}`;
}

/** The public name a mapping resolves to: the prefix plus the override base, else the upstream name. */
export function previewModelName(input: ModelNamePreviewInput): string {
    return composeModelName(
        input.modelPrefix,
        input.baseName.length > 0 ? input.baseName : input.upstreamName,
    );
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
