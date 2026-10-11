import { createRoot } from "solid-js";
import { describe, expect, test, vi } from "vitest";
import { hasGuardedSubjects, registerLeaveGuardSubject } from "/src/leave-guard-registry";
import type { ManagementApi } from "/src/management-api/client";
import { ManagementApiError } from "/src/management-api/errors";
import type {
    CompleteProviderOauthRequest,
    CompleteProviderOauthResponse,
    ProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import type { ActionBegin } from "/src/screen-state/action-begin";
import { createOauthFlow, type OauthFlow } from "./oauth-flow";

/**
 * The OAuth flow state machine shared by the provider screens: the begin
 * installing the live flow and arming the leave-guard seat, the
 * authorization-code completion reading the pasted callback (the unreadable
 * paste stating its inline diagnosis and surviving, the pending answer being
 * fatal), the shared settle arms (aborted states nothing, gone resets with
 * the flow's own verdict, a failed exchange burns the flow, succeeded
 * installs the provider), the device poll's retry budget and expiry stop,
 * and every settle path disarming the seat. Solid 2 schedules signal
 * writes, so every mutation settles through a tick before the machine is
 * read again — the same settlement the browser's flush gives it.
 */

/** Advances past owned scopes and lets answered promises and staged writes settle. */
async function tick(): Promise<void> {
    for (let round = 0; round < 4; round += 1) {
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
}

/** One OAuth provider row a succeeded completion installs. */
const PROVIDER: ProviderResponse = {
    type: "oauth",
    id: 4,
    name: "acme",
    model_prefix: "acme",
    is_enabled: true,
    provider_definition_name: "claude-code",
    provider_definition_version: 1,
    format: "anthropic_messages",
    base_url: null,
    expires_at: null,
};

const AUTH_CODE_FLOW: StartProviderOauthResponse = {
    grant: "authorization_code",
    state: "s-code",
    expires_at: Date.now() * 1000 + 600_000_000,
    authorize_url: "https://auth.example.test/authorize",
};

const DEVICE_FLOW: StartProviderOauthResponse = {
    grant: "device",
    state: "s-device",
    expires_at: Date.now() * 1000 + 600_000_000,
    user_code: "ABCD-EFGH",
};

/** The completion calls the flow made, with the answers the test scripts. */
type CompletionHarness = {
    readonly calls: CompleteProviderOauthRequest[];
    answer: (
        response:
            | {
                  readonly kind: "response";
                  readonly value: CompleteProviderOauthResponse;
              }
            | {
                  readonly kind: "error";
                  readonly value: unknown;
              },
    ) => void;
    readonly api: ManagementApi;
};

/** Builds an api stub whose completion calls wait for the test's scripted answer. */
function completionHarness(): CompletionHarness {
    const calls: CompleteProviderOauthRequest[] = [];
    let settle: ((response: CompleteProviderOauthResponse) => void) | null = null;
    let fail: ((error: unknown) => void) | null = null;

    const api = {
        completeProviderOauth: (request: CompleteProviderOauthRequest) => {
            calls.push(request);
            return new Promise<CompleteProviderOauthResponse>((resolve, reject) => {
                settle = resolve;
                fail = reject;
            });
        },
    } as unknown as ManagementApi;

    return {
        calls,
        answer: (answer) => {
            if (answer.kind === "response") {
                settle?.(answer.value);
            } else {
                fail?.(answer.value);
            }
        },
        api,
    };
}

/** Mounts one flow machine under a fresh root, returning it and the root's dispose. */
function mountFlow(options: {
    readonly api: ManagementApi;
    readonly begin?: () => Promise<ActionBegin<StartProviderOauthResponse>>;
    readonly onSucceeded?: (provider: ProviderResponse) => void;
    readonly onGone?: () => void;
}): {
    flow: OauthFlow;
    dispose: () => void;
} {
    let disposeRoot: (() => void) | null = null;
    const machine = createRoot((dispose) => {
        disposeRoot = dispose;
        return createOauthFlow({
            api: options.api,
            startAction: "start-oauth",
            completeAction: "complete-oauth",
            begin:
                options.begin ??
                (async () => ({
                    status: "called",
                    outcome: {
                        status: "ok",
                        result: AUTH_CODE_FLOW,
                    },
                })),
            onSucceeded: options.onSucceeded ?? (() => {}),
            onGone: options.onGone,
        });
    });

    return {
        flow: machine,
        dispose: () => disposeRoot?.(),
    };
}

/** Begins the authorization-code flow of the default prelude and settles the machine. */
async function startAuthCodeFlow(flow: OauthFlow): Promise<void> {
    await flow.start();
    await tick();
    flow.setCode("https://auth.example.test/callback?state=x&code=pasted-code");
    await tick();
}

describe("flow begin", () => {
    test("installs the answered flow and arms the leave-guard seat", async () => {
        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
        });
        await tick();

        await flow.start();
        await tick();

        expect(flow.flow()).toEqual(AUTH_CODE_FLOW);
        expect(flow.isStarting()).toBe(false);
        // The live flow holds a guarded seat — the beforeunload guard's truth.
        expect(hasGuardedSubjects()).toBe(true);
        dispose();
        await tick();
        // The disposal released the seat.
        expect(hasGuardedSubjects()).toBe(false);
    });

    test("states a blocked prelude on the flow card's channel and starts nothing", async () => {
        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
            begin: async () => ({
                status: "blocked",
                notice: {
                    tone: "bad",
                    title: "The definition is missing.",
                },
            }),
        });
        await tick();

        await flow.start();
        await tick();

        expect(flow.flow()).toBeNull();
        expect(flow.failure()).toEqual({
            tone: "bad",
            title: "The definition is missing.",
        });
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });

    test("states a refused begin under the start action and leaves no seat armed", async () => {
        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
            begin: async () => ({
                status: "called",
                outcome: {
                    status: "failed",
                    cause: new ManagementApiError("gateway", "the version moved", {
                        errno: ErrorCode.Conflict,
                        httpStatus: 409,
                    }),
                },
            }),
        });
        await tick();

        await flow.start();
        await tick();

        expect(flow.flow()).toBeNull();
        expect(flow.failure()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });

    test("clears the channel when a fresh begin takes off", async () => {
        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
        });
        await tick();

        await startAuthCodeFlow(flow);
        flow.reset();
        await tick();

        await flow.start();
        await tick();
        // The begin cleared whatever the previous flow had stated.
        expect(flow.failure()).toBeNull();
        dispose();
    });
});

describe("authorization-code completion", () => {
    test("sends the extracted code from the pasted callback", async () => {
        const harness = completionHarness();
        const onSucceeded = vi.fn();
        const { flow, dispose } = mountFlow({
            api: harness.api,
            onSucceeded,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "response",
            value: {
                status: "succeeded",
                provider: PROVIDER,
            },
        });
        await completed;
        await tick();

        expect(harness.calls).toEqual([
            {
                grant: "authorization_code",
                state: "s-code",
                code: "pasted-code",
            },
        ]);
        expect(onSucceeded).toHaveBeenCalledWith(PROVIDER);
        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });

    test("survives an unreadable paste, stating the inline diagnosis until the paste reads", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await flow.start();
        await tick();
        flow.setCode("https://auth.example.test/callback?state=x");
        await tick();

        await flow.complete();
        await tick();

        expect(harness.calls).toHaveLength(0);
        expect(flow.pasteError()).not.toBeNull();

        // A paste that reads retracts the diagnosis without another ask.
        flow.setCode("https://auth.example.test/callback?code=now-readable");
        await tick();
        expect(flow.pasteError()).toBeNull();
        dispose();
    });

    test("burns the flow on a failed exchange and states the fresh-flow recovery", async () => {
        const harness = completionHarness();
        const onSucceeded = vi.fn();
        const { flow, dispose } = mountFlow({
            api: harness.api,
            onSucceeded,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "response",
            value: {
                status: "failed",
                failure_code: "bad_verification_code",
                failure_message: "the code was burned",
            },
        });
        await completed;
        await tick();

        expect(flow.flow()).toBeNull();
        expect(onSucceeded).not.toHaveBeenCalled();
        expect(flow.failure()?.tone).toBe("bad");
        expect(flow.failure()?.detail).toBe("the code was burned");
        dispose();
    });

    test("resets and hands the gone verdict over when the flow state proved spent", async () => {
        const harness = completionHarness();
        const onGone = vi.fn();
        const { flow, dispose } = mountFlow({
            api: harness.api,
            onGone,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "error",
            value: new ManagementApiError("gateway", "no such flow", {
                errno: ErrorCode.NotFound,
                httpStatus: 404,
            }),
        });
        await completed;
        await tick();

        expect(flow.flow()).toBeNull();
        expect(onGone).toHaveBeenCalledTimes(1);
        expect(flow.failure()?.detail).toBe("no such flow");
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });

    test("states a failed-but-not-gone completion verbatim and keeps the flow live", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "error",
            value: new ManagementApiError("transport", "Network request failed"),
        });
        await completed;
        await tick();

        expect(flow.flow()).toEqual(AUTH_CODE_FLOW);
        expect(flow.failure()).not.toBeNull();
        // The live flow's seat stays armed: the flow stands on the gateway.
        expect(hasGuardedSubjects()).toBe(true);
        dispose();
    });

    test("answers an aborted completion with nothing stated and the flow intact", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "error",
            value: new DOMException("aborted", "AbortError"),
        });
        await completed;
        await tick();

        expect(flow.flow()).toEqual(AUTH_CODE_FLOW);
        expect(flow.failure()).toBeNull();
        dispose();
    });

    test("treats a pending answer to a synchronous grant as the contract break it is", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        harness.answer({
            kind: "response",
            value: {
                status: "pending",
            },
        });

        await expect(completed).rejects.toThrowError(
            "The authorization-code completion answered pending.",
        );
        dispose();
    });
});

describe("device completion polling", () => {
    /** Mounts a live device flow under its own root. */
    async function mountDeviceFlow(
        harness: CompletionHarness,
        onSucceeded: (provider: ProviderResponse) => void = () => {},
    ): Promise<{
        flow: OauthFlow;
        dispose: () => void;
    }> {
        let disposeRoot: (() => void) | null = null;
        const flow = createRoot((dispose) => {
            disposeRoot = dispose;
            return createOauthFlow({
                api: harness.api,
                startAction: "start-oauth",
                completeAction: "complete-oauth",
                begin: async () => ({
                    status: "called",
                    outcome: {
                        status: "ok",
                        result: DEVICE_FLOW,
                    },
                }),
                onSucceeded,
            });
        });
        await tick();
        await flow.start();
        await tick();

        return {
            flow,
            dispose: () => disposeRoot?.(),
        };
    }

    /** One scripted poll answering with the given outcome. */
    async function pollWith(
        flow: OauthFlow,
        harness: CompletionHarness,
        answer: Parameters<CompletionHarness["answer"]>[0],
    ): Promise<"pending" | "done"> {
        const poll = flow.completeDevice();
        harness.answer(answer);

        return poll;
    }

    test("ends the loop at once for a flow that is not a device flow", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await flow.start();
        await tick();

        await expect(flow.completeDevice()).resolves.toBe("done");
        expect(harness.calls).toHaveLength(0);
        dispose();
    });

    test("stops with the expiry verdict once the flow's lifetime has passed", async () => {
        const harness = completionHarness();
        let disposeRoot: (() => void) | null = null;
        // The assignment happens inside the root's callback, which the read
        // site's control-flow analysis cannot see; dispose through a function,
        // where the variable reads at its declared type.
        const disposeFlow = () => disposeRoot?.();
        const flow = createRoot((dispose) => {
            disposeRoot = dispose;
            return createOauthFlow({
                api: harness.api,
                startAction: "start-oauth",
                completeAction: "complete-oauth",
                begin: async () => ({
                    status: "called",
                    outcome: {
                        status: "ok",
                        result: {
                            ...DEVICE_FLOW,
                            expires_at: Date.now() * 1000 - 1,
                        },
                    },
                }),
                onSucceeded: () => {},
            });
        });
        await tick();

        await flow.start();
        await tick();

        await expect(flow.completeDevice()).resolves.toBe("done");
        await tick();
        expect(harness.calls).toHaveLength(0);
        expect(flow.flow()).toBeNull();
        expect(flow.failure()?.tone).toBe("bad");
        disposeFlow();
    });

    test("keeps polling through a pending answer, clearing any stale failure", async () => {
        const harness = completionHarness();
        const { flow, dispose } = await mountDeviceFlow(harness);

        const outcome = await pollWith(flow, harness, {
            kind: "response",
            value: {
                status: "pending",
            },
        });
        await tick();

        expect(outcome).toBe("pending");
        expect(harness.calls).toEqual([
            {
                grant: "device",
                state: "s-device",
            },
        ]);
        expect(flow.failure()).toBeNull();
        dispose();
    });

    test("rides out one failed poll with the live-flow recovery, then re-arms", async () => {
        const harness = completionHarness();
        const { flow, dispose } = await mountDeviceFlow(harness);

        const outcome = await pollWith(flow, harness, {
            kind: "error",
            value: new ManagementApiError("transport", "Network request failed"),
        });
        await tick();

        expect(outcome).toBe("pending");
        expect(flow.flow()).toEqual(DEVICE_FLOW);
        expect(flow.failure()).not.toBeNull();
        dispose();
    });

    test("stops asking after three consecutive failed polls, stating the one exit", async () => {
        const harness = completionHarness();
        const { flow, dispose } = await mountDeviceFlow(harness);
        const refusal = {
            kind: "error" as const,
            value: new ManagementApiError("transport", "Network request failed"),
        };

        for (let attempt = 0; attempt < 2; attempt += 1) {
            const outcome = await pollWith(flow, harness, refusal);
            expect(outcome).toBe("pending");
        }

        const outcome = await pollWith(flow, harness, refusal);
        await tick();

        expect(outcome).toBe("done");
        expect(flow.failure()?.tone).toBe("bad");
        dispose();
    });

    test("installs the provider and stops the loop when the device flow succeeds", async () => {
        const harness = completionHarness();
        const onSucceeded = vi.fn();
        const { flow, dispose } = await mountDeviceFlow(harness, onSucceeded);

        const outcome = await pollWith(flow, harness, {
            kind: "response",
            value: {
                status: "succeeded",
                provider: PROVIDER,
            },
        });
        await tick();

        expect(outcome).toBe("done");
        expect(onSucceeded).toHaveBeenCalledWith(PROVIDER);
        expect(flow.flow()).toBeNull();
        dispose();
    });
});

describe("flow discard and disposal", () => {
    test("reset ends the live flow and disarms the seat", async () => {
        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
        });
        await tick();

        await startAuthCodeFlow(flow);
        flow.setCode("some paste");
        await tick();

        flow.reset();
        await tick();

        expect(flow.flow()).toBeNull();
        expect(flow.code()).toBe("");
        expect(flow.failure()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });

    test("an answer after the screen went away states nothing and re-arms nothing", async () => {
        const harness = completionHarness();
        const { flow, dispose } = mountFlow({
            api: harness.api,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        dispose();
        await tick();

        harness.answer({
            kind: "response",
            value: {
                status: "succeeded",
                provider: PROVIDER,
            },
        });
        await completed;
        await tick();

        expect(hasGuardedSubjects()).toBe(false);
    });

    test("a completion answering for a replaced flow acts on nothing", async () => {
        const harness = completionHarness();
        const onSucceeded = vi.fn();
        const { flow, dispose } = mountFlow({
            api: harness.api,
            onSucceeded,
        });
        await tick();

        await startAuthCodeFlow(flow);

        const completed = flow.complete();
        // The operator discards the flow before the answer lands: the answer
        // names a flow that is no longer the live one.
        flow.reset();
        await tick();

        harness.answer({
            kind: "response",
            value: {
                status: "succeeded",
                provider: PROVIDER,
            },
        });
        await completed;
        await tick();

        // The stale answer installed nothing and armed nothing.
        expect(onSucceeded).not.toHaveBeenCalled();
        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });
});

describe("the registry seat the flow holds", () => {
    test("a foreign seat's guardedness never leaks into the flow's report", async () => {
        const foreign = registerLeaveGuardSubject();
        foreign.reportGuarded(true);
        await tick();

        const { flow, dispose } = mountFlow({
            api: completionHarness().api,
        });
        await tick();

        await flow.start();
        await tick();
        flow.reset();
        await tick();

        // The foreign seat still stands guarded; the flow's own disarm cannot clear it.
        expect(hasGuardedSubjects()).toBe(true);
        foreign.release();
        await tick();
        expect(hasGuardedSubjects()).toBe(false);
        dispose();
    });
});
