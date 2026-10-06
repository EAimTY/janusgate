/**
 * Deadline plumbing for the connection probe and the diagnosis's
 * reachability refinement: a pure transport utility with no wiring of
 * its own.
 */

/**
 * A fresh `AbortController` paired with a deadline of `timeoutMs`. One
 * signal reaches the transport: the deadline forwards its abort — reason
 * included — into the controller, and a direct abort of the controller
 * (an operator cancel, a screen going away) ends the request the same
 * way — whichever fires first. The forwarding is what makes an expiry
 * distinguishable at the transport, the fact the diagnosis's "a deadline
 * is never an answer" rule rests on.
 */
export function deadlineController(timeoutMs: number): AbortController {
    const controller = new AbortController();
    const deadline = AbortSignal.timeout(timeoutMs);
    deadline.addEventListener("abort", () => {
        controller.abort(deadline.reason);
    });
    return controller;
}
