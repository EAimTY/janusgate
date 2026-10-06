/** Development-only checks. This module is never imported by the application. */
import { type PluralContract, pluralMessages, type RichContract, richMessages } from "./metadata";
import type { LocaleEntry } from "./types";

const pluralSuffix = /_(?:(ordinal)_)?(zero|one|two|few|many|other)$/;
function leaves(
    value: unknown,
    prefix: string,
    errors: string[],
    groups: Set<string>,
    output: Map<string, string>,
): void {
    if (typeof value === "string") {
        if (!value.trim()) errors.push(`${prefix}: empty string`);
        output.set(prefix, value);
        return;
    }
    if (value === null || typeof value !== "object" || Array.isArray(value)) {
        errors.push(`${prefix}: invalid structure`);
        return;
    }
    groups.add(prefix);
    for (const [key, child] of Object.entries(value))
        leaves(child, prefix ? `${prefix}.${key}` : key, errors, groups, output);
}
function parameters(message: string, path: string, errors: string[]): Set<string> {
    const names = [
        ...message.matchAll(/{{([A-Za-z][A-Za-z0-9]*)}}/g),
    ].map((match) => match[1] ?? "");
    if (/[{}]/.test(message.replace(/{{[A-Za-z][A-Za-z0-9]*}}/g, "")))
        errors.push(`${path}: malformed interpolation`);
    return new Set(names);
}
function sameSet(a: ReadonlySet<string>, b: ReadonlySet<string>): boolean {
    return (
        a.size === b.size &&
        [
            ...a,
        ].every((key) => b.has(key))
    );
}
function checkSlots(
    message: string,
    path: string,
    expected: readonly string[],
    errors: string[],
): void {
    const slots = [
        ...message.matchAll(/\[\[([A-Za-z][A-Za-z0-9]*)\]\]/g),
    ].map((match) => match[1] ?? "");
    if (
        /\[\[|\]\]/.test(message.replace(/\[\[[A-Za-z][A-Za-z0-9]*\]\]/g, "")) ||
        slots.length !== new Set(slots).size ||
        !sameSet(new Set(slots), new Set(expected))
    )
        errors.push(`${path}: invalid rich slots`);
}

/** Compares semantic keys after normalizing declared native plural branches. */
export function checkCatalog(
    reference: unknown,
    catalog: unknown,
    locale: string,
    plurals: readonly PluralContract[] = pluralMessages,
    rich: readonly RichContract[] = richMessages,
): string[] {
    const errors: string[] = [];
    const canonical = new Map<string, string>();
    const translated = new Map<string, string>();
    const canonicalGroups = new Set<string>();
    const translatedGroups = new Set<string>();
    leaves(reference, "", errors, canonicalGroups, canonical);
    leaves(catalog, "", errors, translatedGroups, translated);
    if (!sameSet(canonicalGroups, translatedGroups))
        errors.push("Missing/extra groups or scalar/object mismatch");
    function normalize(input: Map<string, string>): Map<string, string[]> {
        const semantic = new Map<string, string[]>();
        for (const [key, message] of input) {
            const match = pluralSuffix.exec(key);
            const path = match ? key.slice(0, match.index) : key;
            const contract = plurals.find((entry) => entry.path === path);
            if (match && (!contract || Boolean(match[1]) !== (contract.kind === "ordinal")))
                errors.push(`${key}: undeclared plural branch`);
            if (!match && contract) errors.push(`${key}: plural requires suffixed branches`);
            semantic.set(path, [
                ...(semantic.get(path) ?? []),
                key,
            ]);
            const slots = rich.find((entry) => entry.path === path)?.slots ?? [];
            checkSlots(message, key, slots, errors);
            const interpolation = parameters(message, key, errors);
            if (
                slots.length > 0 &&
                [
                    ...interpolation,
                ].some((name) => name !== "count")
            )
                errors.push(`${key}: string values require rich slots`);
        }
        return semantic;
    }
    const source = normalize(canonical);
    const target = normalize(translated);
    for (const key of source.keys()) if (!target.has(key)) errors.push(`${key}: missing message`);
    for (const key of target.keys()) if (!source.has(key)) errors.push(`${key}: extra message`);
    for (const [path, branches] of target) {
        const sourceKey =
            source.get(path)?.find((key) => key.endsWith("_other")) ?? source.get(path)?.[0];
        if (sourceKey === undefined) continue;
        const expected = parameters(canonical.get(sourceKey) ?? "", sourceKey, errors);
        const contract = plurals.find((entry) => entry.path === path);
        for (const branch of branches) {
            const actual = parameters(translated.get(branch) ?? "", branch, errors);
            if (!sameSet(new Set(expected), actual))
                errors.push(`${branch}: interpolation parameters differ`);
        }
        if (contract) {
            const categories = new Set<string>(
                new Intl.PluralRules(locale, {
                    type: contract.kind,
                }).resolvedOptions().pluralCategories,
            );
            categories.add("other");
            const prefix = `${path}_${contract.kind === "ordinal" ? "ordinal_" : ""}`;
            for (const category of categories)
                if (!translated.has(`${prefix}${category}`))
                    errors.push(`${path}: missing plural ${category}`);
            for (const branch of branches)
                if (!categories.has(branch.slice(prefix.length)))
                    errors.push(`${branch}: extra plural category`);
        }
    }
    // English branches carry the same interpolation parameters, branch for branch.
    for (const branches of source.values()) {
        const first = branches.find((key) => key.endsWith("_other")) ?? branches[0];
        if (first === undefined) continue;
        const expected = parameters(canonical.get(first) ?? "", first, errors);
        for (const branch of branches) {
            const actual = parameters(canonical.get(branch) ?? "", branch, errors);
            if (!sameSet(new Set(expected), actual))
                errors.push(`${branch}: inconsistent English parameters`);
        }
    }
    for (const contract of plurals)
        if (!source.has(contract.path)) errors.push(`${contract.path}: declared plural is absent`);
    return errors;
}

/** Registry authoring invariants are checked offline, never repaired at runtime. */
export async function checkRegistry(
    registry: Readonly<Record<string, LocaleEntry>>,
    defaults: Readonly<Record<string, string>>,
): Promise<string[]> {
    const errors: string[] = [];
    if (!registry["en"]) errors.push("English is required");
    for (const [tag, entry] of Object.entries(registry)) {
        if (new Intl.Locale(tag).baseName !== tag) errors.push(`${tag}: noncanonical tag`);
        if (!entry.autonym.trim()) errors.push(`${tag}: missing autonym`);
        const language = new Intl.Locale(tag).language;
        if (!defaults[language]) errors.push(`${tag}: missing language default`);
        const catalog = await entry.load();
        if (!catalog || typeof catalog !== "object") errors.push(`${tag}: invalid loader`);
    }
    for (const [language, tag] of Object.entries(defaults))
        if (!registry[tag] || new Intl.Locale(tag).language !== language)
            errors.push(`${language}: invalid default`);
    return errors;
}
