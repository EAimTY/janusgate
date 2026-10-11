import type { JSX } from "@solidjs/web";
import { render } from "@solidjs/web";
import { type Accessor, createMemo, createSignal, flush } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ManagementCallOutcome } from "/src/management-api/outcome";
import type { FailureCall } from "/src/screen-state/failure";
import type { ScreenNotice } from "/src/screen-state/notices";
import { isSkeletonWindowOpen, type ScreenRead } from "/src/screen-state/read";
import { ReadNotice, StatedNotice } from "/src/ui/read-notice";
import { ScreenFrame } from "/src/ui/screen-frame";
import { SkeletonRows } from "/src/ui/skeleton-rows";

let disposeView: (() => void) | null = null;

function mount(view: () => JSX.Element): HTMLElement {
    const host = document.createElement("div");
    document.body.appendChild(host);
    disposeView = render(view, host);
    return host;
}

function requireElement<TElement extends Element>(host: ParentNode, selectors: string): TElement {
    const element = host.querySelector<TElement>(selectors);
    if (element === null) {
        throw new Error(`Missing element for "${selectors}".`);
    }

    return element;
}

function click(element: HTMLElement): void {
    element.click();
    flush();
}

const failureNotice: ScreenNotice = {
    tone: "bad",
    title: "No answer from the gateway.",
    detail: "Either nothing is listening at that address, or the browser blocked the response.",
};

function isOk<TResult>(outcome: ManagementCallOutcome<TResult>): outcome is Extract<
    ManagementCallOutcome<TResult>,
    {
        status: "ok";
    }
> {
    return outcome.status === "ok";
}

/**
 * A read whose signals the test drives directly: the frame's branches are
 * mutually exclusive by their own conditions, so the fixture mirrors the
 * states a read cycle moves through without running one.
 */
function createReadFixture(state: {
    readonly hasRead: boolean;
    readonly isReading: boolean;
    readonly notice: ScreenNotice | null;
}): ScreenRead & {
    readonly setHasRead: (value: boolean) => void;
    readonly setIsReading: (value: boolean) => void;
    readonly setNotice: (value: ScreenNotice | null) => void;
} {
    const [hasRead, setHasRead] = createSignal(state.hasRead);
    const [isReading, setIsReading] = createSignal(state.isReading);
    const [notice, setNotice] = createSignal<ScreenNotice | null>(state.notice);
    const isFirstRead: Accessor<boolean> = createMemo(() =>
        isSkeletonWindowOpen(isReading(), hasRead(), notice()),
    );

    return {
        hasRead,
        isReading,
        isFirstRead,
        isDisposed: () => false,
        notice,
        showNotice: (stated) => setNotice(stated),
        reportAction: <TResult,>(
            outcome: ManagementCallOutcome<TResult>,
            _call: FailureCall,
            _options?: {
                readonly onGone?: (() => void) | undefined;
            },
        ): outcome is Extract<
            ManagementCallOutcome<TResult>,
            {
                status: "ok";
            }
        > => isOk(outcome),
        reload: () => {},
        setHasRead,
        setIsReading,
        setNotice,
    };
}

afterEach(() => {
    disposeView?.();
    disposeView = null;
    document.body.replaceChildren();
});

describe("ScreenFrame", () => {
    it("stands the first read's header and skeleton while the first read is in flight", () => {
        const fixture = createReadFixture({
            hasRead: false,
            isReading: true,
            notice: null,
        });
        const host = mount(() => (
            <ScreenFrame
                read={fixture}
                row={() => null}
                firstReadHeader={<h1>Edit API key</h1>}
                notFound={<div>API key not found</div>}
            >
                {(row) => <p>{`Row ${String(row)}`}</p>}
            </ScreenFrame>
        ));

        expect(host.querySelector("h1")?.textContent).toBe("Edit API key");
        expect(host.querySelector(".wash")).not.toBeNull();
        expect(host.textContent).not.toContain("API key not found");
        expect(host.querySelector("button")).toBeNull();
    });

    it("keeps the header beside a failed first read and reruns the read from there", () => {
        const fixture = createReadFixture({
            hasRead: false,
            isReading: false,
            notice: failureNotice,
        });
        const reload = vi.fn<() => void>();
        const failingRead: ScreenRead = {
            ...fixture,
            reload,
        };
        const host = mount(() => (
            <ScreenFrame
                read={failingRead}
                row={() => null}
                firstReadHeader={<h1>Edit API key</h1>}
                notFound={<div>API key not found</div>}
            >
                {(row) => <p>{`Row ${String(row)}`}</p>}
            </ScreenFrame>
        ));

        // The standing failure holds the skeleton window closed.
        expect(host.querySelector(".wash")).toBeNull();
        expect(host.textContent).toContain("No answer from the gateway.");
        expect(host.querySelector("h1")?.textContent).toBe("Edit API key");

        click(requireElement<HTMLButtonElement>(host, "button"));
        expect(reload).toHaveBeenCalledTimes(1);
    });

    it("replaces a stale editor with the not-found state when no row answers", () => {
        const fixture = createReadFixture({
            hasRead: true,
            isReading: false,
            notice: null,
        });
        const host = mount(() => (
            <ScreenFrame
                read={fixture}
                row={() => null}
                firstReadHeader={<h1>Edit API key</h1>}
                notFound={<div>API key not found</div>}
            >
                {(row) => <p>{`Row ${String(row)}`}</p>}
            </ScreenFrame>
        ));

        expect(host.textContent).toContain("API key not found");
        expect(host.textContent).not.toContain("Row");
    });

    it("lands the skeleton into the keyed row content once the read answers", () => {
        const fixture = createReadFixture({
            hasRead: false,
            isReading: true,
            notice: null,
        });
        const [row, setRow] = createSignal<{
            readonly id: number;
            readonly name: string;
        } | null>(null);
        const host = mount(() => (
            <ScreenFrame
                read={fixture}
                row={row}
                firstReadHeader={<h1>Edit API key</h1>}
                notFound={<div>API key not found</div>}
            >
                {(current) => <p>{`Row: ${current.name}`}</p>}
            </ScreenFrame>
        ));

        expect(host.querySelector(".wash")).not.toBeNull();

        fixture.setIsReading(false);
        fixture.setHasRead(true);
        setRow({
            id: 7,
            name: "staging",
        });
        flush();
        expect(host.querySelector(".wash")).toBeNull();
        expect(host.textContent).toContain("Row: staging");

        // The content is keyed to the row: a fresh row object re-renders it.
        setRow({
            id: 7,
            name: "renamed",
        });
        flush();
        expect(host.textContent).toContain("Row: renamed");
    });
});

describe("ReadNotice", () => {
    it("renders the standing read failure and offers Read again only where nothing else stands", () => {
        const fixture = createReadFixture({
            hasRead: false,
            isReading: false,
            notice: failureNotice,
        });
        const retry = vi.fn<() => void>();
        const host = mount(() => (
            <ReadNotice
                read={fixture}
                retry={retry}
            />
        ));

        expect(host.textContent).toContain("No answer from the gateway.");
        click(requireElement<HTMLButtonElement>(host, "button"));
        expect(retry).toHaveBeenCalledTimes(1);
    });

    it("states a notice beside last-good content without offering a read rerun", () => {
        const fixture = createReadFixture({
            hasRead: true,
            isReading: false,
            notice: failureNotice,
        });
        const retry = vi.fn<() => void>();
        const host = mount(() => (
            <ReadNotice
                read={fixture}
                retry={retry}
            />
        ));

        expect(host.textContent).toContain("No answer from the gateway.");
        expect(host.querySelector("button")).toBeNull();
        expect(retry).not.toHaveBeenCalled();
    });

    it("renders nothing while no notice stands", () => {
        const fixture = createReadFixture({
            hasRead: true,
            isReading: false,
            notice: null,
        });
        const host = mount(() => <ReadNotice read={fixture} />);

        expect(host.textContent).toBe("");
    });
});

describe("StatedNotice", () => {
    it("renders bare while it carries neither class nor children, wrapped otherwise", () => {
        const [notice] = createSignal<ScreenNotice | null>({
            tone: "ok",
            title: "staging saved.",
        });
        const bare = mount(() => <StatedNotice notice={notice} />);
        // Bare means the Notice itself, with no placement wrapper around it.
        expect(bare.children.length).toBe(1);
        expect(bare.firstElementChild?.tagName).toBe("DIV");
        expect(bare.firstElementChild?.className).not.toContain("mt-4");
        expect(bare.textContent).toContain("staging saved.");

        const [wrapped] = createSignal<ScreenNotice | null>({
            tone: "ok",
            title: "staging saved.",
        });
        const wrappedHost = mount(() => (
            <StatedNotice
                notice={wrapped}
                class="mt-4"
            >
                <button type="button">Undo</button>
            </StatedNotice>
        ));
        expect(requireElement(wrappedHost, "div.mt-4 > div")).not.toBeNull();
        expect(requireElement<HTMLButtonElement>(wrappedHost, "button").textContent).toBe("Undo");
    });
});

describe("SkeletonRows", () => {
    it("stands the requested placeholder rows while the first read is in flight, then lands the listing", () => {
        const fixture = createReadFixture({
            hasRead: false,
            isReading: true,
            notice: null,
        });
        const host = mount(() => (
            <SkeletonRows
                read={fixture}
                rowCount={3}
                rowHeight="h-16"
            />
        ));

        let skeletons = Array.from(host.querySelectorAll(".wash"));
        expect(skeletons.length).toBe(3);
        expect(skeletons[0]?.className).toContain("h-16");

        fixture.setIsReading(false);
        fixture.setHasRead(true);
        flush();
        skeletons = Array.from(host.querySelectorAll(".wash"));
        expect(skeletons.length).toBe(0);
    });
});
