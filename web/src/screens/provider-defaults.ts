/**
 * The first free name for a fresh provider: the given base name, then the
 * base with `-1`, `-2`, and so on while taken names collide. Gaps left by a
 * renamed provider are reused from the lowest suffix up.
 */
export function suggestProviderName(base: string, takenNames: ReadonlySet<string>): string {
    if (!takenNames.has(base)) {
        return base;
    }

    let suffix = 1;

    while (takenNames.has(`${base}-${suffix}`)) {
        suffix += 1;
    }

    return `${base}-${suffix}`;
}
