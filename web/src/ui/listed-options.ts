/**
 * The option list one id-bearing select presents: the listing the screen
 * read, plus the current value when no listed row carries it.
 *
 * A filter id the route or an arrival subject carried can name a row deleted
 * since it was written, and a listing-built list alone would match nothing —
 * the select would visibly reset to its "All" option while the filter still
 * sent the id. The trailing option names the id, so the control keeps
 * stating the filter in force and the filter stays inspectable. The usage
 * filter's id fields and the models screen's provider filter share it.
 */
export function listedOptions(
    options: ReadonlyArray<{
        readonly value: string;
        readonly label: string;
    }>,
    value: string,
): ReadonlyArray<{
    readonly value: string;
    readonly label: string;
}> {
    if (value === "" || options.some((option) => option.value === value)) {
        return options;
    }

    return [
        ...options,
        {
            value,
            label: `${value} (no longer listed)`,
        },
    ];
}
