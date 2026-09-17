/**
 * A `ref` callback for a control that replaces the one the user just
 * activated — a confirmation button standing in for the action button, for
 * instance — so a keyboard user does not lose their place to `<body>`.
 */
export function focusOnMount(element: HTMLElement): void {
    if (element.isConnected) {
        element.focus();
        return;
    }

    // A ref runs while the element is still detached, and focusing a detached
    // element does nothing; the pending insertion is complete by the next
    // microtask.
    queueMicrotask(() => {
        element.focus();
    });
}
