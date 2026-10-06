import type { JSX } from "@solidjs/web";
import type { Accessor } from "solid-js";
import { createSignal } from "solid-js";
import type { Diagnosis } from "../management-api/diagnosis";
import type { FailureCall } from "./failure";
import { describeCallFailure } from "./failure";

/**
 * The tone a stated notice carries — `"ok"`, `"warn"`, `"bad"`, or `"info"`
 * — the notice's state semantics, chosen where the notice is stated: the
 * diagnosis of a failure, the verdict of an action's outcome. Presentation
 * renders it, deriving the notice's classes, glyph, and ARIA role from it.
 */
export type NoticeTone = "ok" | "warn" | "bad" | "info";

/** What a screen states about its last read or action, rendered as a `Notice`. */
export type ScreenNotice = {
    readonly tone: NoticeTone;
    readonly title: JSX.Element;
    readonly detail?: JSX.Element;
    readonly fix?: JSX.Element;
    readonly rawDetail?: string | undefined;
};

/** The bad notice a diagnosis states verbatim: its title, detail, and fix carried across unchanged. */
export function noticeOf(diagnosis: Diagnosis): ScreenNotice {
    return {
        tone: "bad",
        ...(diagnosis.rawDetail === undefined
            ? {}
            : {
                  rawDetail: diagnosis.rawDetail,
              }),
        get title() {
            return diagnosis.title;
        },
        get detail() {
            return diagnosis.detail;
        },
        get fix() {
            return diagnosis.fix;
        },
    };
}

/** The bad notice a failed call renders: the app's own diagnosis of the failure. */
export function screenFailureNotice(cause: unknown, call?: FailureCall): ScreenNotice {
    return noticeOf(describeCallFailure(cause, call));
}

/**
 * Where a standing notice came from: a failed read cycle's diagnosis
 * (`"read"`), or an action's reported outcome (`"action"`). The origin
 * decides what may clear the notice — the two-origin rule (README §6.3).
 */
type NoticeOrigin = "read" | "action";

/** A standing notice, paired with the origin its clearing reads. */
type StandingNotice = {
    readonly notice: ScreenNotice;
    readonly origin: NoticeOrigin;
};

/** The origin-paired standing notice one screen states its failures and outcomes through. */
export type StandingNoticeController = {
    /** The standing notice, or `null` while nothing stands. */
    readonly notice: Accessor<ScreenNotice | null>;
    /** The origin of the notice that stands, or `null` while nothing stands. */
    readonly origin: Accessor<NoticeOrigin | null>;
    /** States a notice from the given origin, replacing whatever stood. */
    readonly state: (origin: NoticeOrigin, notice: ScreenNotice) => void;
    /** Lifts a notice the given origin stated, keeping any other origin's standing notice. */
    readonly clearOfOrigin: (origin: NoticeOrigin) => void;
    /** Lifts whatever stands, whatever its origin — an explicit dismissal. */
    readonly clear: () => void;
};

/**
 * One screen's standing notice under the two-origin rule (README §6.3): one
 * signal holds the notice and its origin together, so every write pairs them
 * and every clearing reads the origin of the notice that actually stands —
 * one notice owner, with no second state to drift. The rule is
 * set-by-origin, clear-by-origin: `state` stamps the notice with the origin
 * that stated it, and only that origin's own success lifts it again
 * (`clearOfOrigin`) — a read cycle's answer clears a read's failure, an
 * action's own clear lifts an action's diagnosis, and neither reaches
 * across. `clear` is the one origin-unchecked lift, for an explicit
 * dismissal handing the standing slot back.
 */
export function createStandingNotice(): StandingNoticeController {
    const [standing, setStanding] = createSignal<StandingNotice | null>(null);

    return {
        notice: (): ScreenNotice | null => standing()?.notice ?? null,
        origin: (): NoticeOrigin | null => standing()?.origin ?? null,
        state: (origin: NoticeOrigin, notice: ScreenNotice): void => {
            setStanding({
                notice,
                origin,
            });
        },
        clearOfOrigin: (origin: NoticeOrigin): void => {
            setStanding((current) => (current?.origin === origin ? null : current));
        },
        clear: (): void => {
            setStanding(null);
        },
    };
}
