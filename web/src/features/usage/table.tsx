import type { JSX } from "@solidjs/web";
import { translation } from "/src/i18n";

/**
 * The usage bucket table's shared grammar, spelled once for every site that
 * renders one — the aggregates screen's answer table and the detail cards'
 * per-day series alike: the scroll wrapper and table skeleton, the header and
 * numeric cells, and the day-boundary note. The row shape stays per-site — a
 * row's key cell labels the site's own dimensions and carries its own
 * truncate and title behavior — so only the cells that truly are the same
 * live here.
 */

/**
 * The wrapper every usage bucket table stands in: a horizontally scrollable
 * `div` around a full-width, start-aligned, tabular-number `table` at the
 * shared cell size. `minWidthClass` is the table's whole min-width class
 * (like `min-w-[480px]`) — passed as one literal string at the call site, so
 * the class stays visible to Tailwind's source scan — and `class` carries the
 * site's own placement classes onto the scroll wrapper.
 */
export function UsageTable(props: {
    readonly minWidthClass: string;
    readonly class?: string | undefined;
    readonly children?: JSX.Element;
}): JSX.Element {
    return (
        <div
            class={props.class === undefined ? "overflow-x-auto" : `${props.class} overflow-x-auto`}
        >
            <table class={`tnum w-full ${props.minWidthClass} text-start text-[13px]`}>
                {props.children}
            </table>
        </div>
    );
}

/**
 * One column header cell: the label, end-aligned when the column holds
 * numbers — the alignment every `BucketCell` beneath it carries.
 */
export function BucketHeader(props: {
    readonly label: string;
    readonly isNumeric?: boolean;
}): JSX.Element {
    return (
        <th
            scope="col"
            class={
                props.isNumeric === true
                    ? "px-3 py-2 text-right text-[12.5px] font-medium"
                    : "px-3 py-2 text-[12.5px] font-medium"
            }
        >
            {props.label}
        </th>
    );
}

/**
 * One numeric cell: a preformatted count or token sum, end-aligned under
 * its numeric `BucketHeader`.
 */
export function BucketCell(props: { readonly value: string }): JSX.Element {
    return (
        <td class="px-3 py-2 text-right">
            <bdi dir="ltr">{props.value}</bdi>
        </td>
    );
}

/**
 * The note under a day-split bucket table: the days the buckets split on are
 * the gateway's own UTC day boundary, stated once in the one sentence.
 * `class` carries the site's spacing onto the note.
 */
export function UsageDaysUtcNote(props: { readonly class: string }): JSX.Element {
    return (
        <p class={`${props.class} text-[12.5px] text-ink-faint`}>
            {translation()(($) => $.helpers.usageTable.daysAreUTCTheGatewaysOwnDayBoundaryMessage)}
        </p>
    );
}
