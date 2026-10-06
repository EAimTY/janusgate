import { render } from "@solidjs/web";
import { createRoot } from "solid-js";
import { describe, expect, it } from "vitest";
import { hasGuardedSubjects } from "/src/leave-guard-registry";
import type {
    CompleteProviderOauthResponse,
    ProviderResponse,
    StartProviderOauthResponse,
} from "/src/management-api/generated/v0";
import { ErrorCode } from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import {
    createDisposals,
    drainWrites,
    gatewayFailure,
    goneFailure,
    providerRow,
    settle,
    stubManagementApi,
} from "/src/test-support/management-api";
import { createOauthFlow, type OauthFlow } from "./oauth-flow";
import { AuthorizeStep, DeviceStep } from "./oauth-flow-view";

/**
 * The OAuth flow machine's own suite: the registry seat the live flow keeps
 * in the leave-guard registry (README §6.3), pinned per end-of-flow path —
 * the seat arming on a succeeded begin and disarming on every settle,
 * discard, expiry, gone, and disposal path, where a leak would leave the
 * shell's `beforeunload` guard armed over a dead screen — and the one
 * navigation warning both live steps state beside their guidance, since the
 * in-app move is the one discarding a leave event cannot intercept.
 */

/** An authorization-code flow the gateway has begun and the machine holds. */
const authorizationFlow: Extract<
    StartProviderOauthResponse,
    {
        grant: "authorization_code";
    }
> = {
    grant: "authorization_code",
    authorize_url: "https://claude.ai/oauth/authorize?state=state-1",
    state: "state-1",
    expires_at: 1_700_000_060_000_000,
};

/** A live device flow expiring the given number of microseconds after the epoch. */
function deviceFlow(expiresAtMicros: number): Extract<
    StartProviderOauthResponse,
    {
        grant: "device";
    }
> {
    return {
        grant: "device",
        state: "state-d1",
        expires_at: expiresAtMicros,
        user_code: "WDJB-MJHT",
        verification_uri_complete: "https://auth.kimi.com/activate?user_code=WDJB-MJHT",
    };
}

/** The provider row a succeeded completion answers with. */
const reauthorizedProvider: ProviderResponse = providerRow({
    model_prefix: "",
    type: "oauth",
    expires_at: 1_700_000_100_000_000,
    id: 7,
    name: "kimi",
    provider_definition_name: "kimi-code",
    format: "openai_chat_completion",
    base_url: null,
});

describe("OauthFlow leave-guard registration", () => {
    const disposes = createDisposals();

    /**
     * The flow machine mounted under its own reactive root, the way a screen
     * holds it: the begin answers with the given flow, the completion runs
     * where the test points it, and every other route refuses loudly. The
     * begin is the reauthorization shape's `"called"` arm — the creation
     * page's definition-ensure prelude adds nothing to the seat's behavior.
     */
    function mountedFlow(options: {
        readonly begun: StartProviderOauthResponse;
        readonly complete: () => Promise<CompleteProviderOauthResponse>;
        readonly onSucceeded?: (provider: ProviderResponse) => void;
    }): {
        readonly flow: OauthFlow;
        readonly dispose: () => void;
    } {
        const api = stubManagementApi({
            completeProviderOauth: options.complete,
        });
        const box: {
            flow?: OauthFlow;
        } = {};
        const dispose = createRoot((teardown) => {
            box.flow = createOauthFlow({
                api,
                startAction: "start-oauth",
                completeAction: "complete-oauth",
                begin: async () => ({
                    status: "called" as const,
                    outcome: await performManagementCall(() => Promise.resolve(options.begun)),
                }),
                onSucceeded: (provider) => {
                    options.onSucceeded?.(provider);
                },
            });
            return teardown;
        });

        if (box.flow === undefined) {
            throw new Error("The flow machine did not mount");
        }

        disposes.push(dispose);

        return {
            flow: box.flow,
            dispose,
        };
    }

    it("stands disarmed until a begin succeeds, then arms and disarms on a manual discard", async () => {
        const { flow } = mountedFlow({
            begun: authorizationFlow,
            complete: () =>
                Promise.resolve({
                    status: "pending",
                }),
        });

        // No flow has stood, so the guard is disarmed.
        await settle();
        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);

        await flow.start();
        await settle();

        // The live flow's loss to a tab close or a reload is now the guard's
        // to intercept, exactly as an unsaved edit form's is.
        expect(flow.flow()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(true);

        flow.reset();
        await drainWrites();

        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("unregisters when the completion succeeds and installs the provider", async () => {
        const landed: ProviderResponse[] = [];
        const { flow } = mountedFlow({
            begun: authorizationFlow,
            complete: () =>
                Promise.resolve({
                    status: "succeeded",
                    provider: reauthorizedProvider,
                }),
            onSucceeded: (provider) => {
                landed.push(provider);
            },
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        flow.setCode("the-code");
        await settle();
        await flow.complete();
        await settle();

        // The settle ran — the provider landed — and the flow's end took the
        // seat with it.
        expect(landed).toEqual([
            reauthorizedProvider,
        ]);
        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("unregisters when the exchange fails and burns the flow", async () => {
        const { flow } = mountedFlow({
            begun: authorizationFlow,
            complete: () =>
                Promise.resolve({
                    status: "failed",
                    failure_code: "invalid_grant",
                    failure_message: "The upstream refused the exchange.",
                }),
        });

        await flow.start();
        await settle();
        flow.setCode("the-code");
        await settle();
        await flow.complete();
        await settle();

        // The burned flow is gone and the seat with it — the guard must not
        // stand armed over a flow that can never complete.
        expect(flow.flow()).toBeNull();
        expect(flow.failure()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("unregisters when the authorization-code completion answers gone", async () => {
        const { flow } = mountedFlow({
            begun: authorizationFlow,
            complete: () => Promise.reject(goneFailure("no such flow")),
        });

        await flow.start();
        await settle();
        flow.setCode("the-code");
        await settle();
        await flow.complete();
        await settle();

        // The spent flow reset through the gone handover, and the seat went
        // with it.
        expect(flow.flow()).toBeNull();
        expect(flow.failure()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("unregisters when a device poll answers gone", async () => {
        const { flow } = mountedFlow({
            begun: deviceFlow((Date.now() + 600_000) * 1000),
            complete: () => Promise.reject(goneFailure("no such flow")),
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        expect(await flow.completeDevice()).toBe("done");
        await drainWrites();

        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("unregisters when a device flow expires", async () => {
        const { flow } = mountedFlow({
            begun: deviceFlow((Date.now() - 60_000) * 1000),
            complete: () =>
                Promise.resolve({
                    status: "pending",
                }),
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        // The expiry is past, so the first ask ends the flow without asking.
        expect(await flow.completeDevice()).toBe("done");
        await drainWrites();

        expect(flow.flow()).toBeNull();
        expect(flow.failure()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });

    it("keeps the seat through answers that leave the flow live", async () => {
        let isFailing = false;
        const { flow } = mountedFlow({
            begun: deviceFlow((Date.now() + 600_000) * 1000),
            complete: () => {
                if (isFailing) {
                    return Promise.reject(
                        gatewayFailure("upstream wobbled", ErrorCode.UpstreamError, 502),
                    );
                }

                return Promise.resolve({
                    status: "pending",
                });
            },
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        // The pending answer: the flow stays live on the gateway, the seat
        // holds.
        expect(await flow.completeDevice()).toBe("pending");
        await drainWrites();
        expect(flow.flow()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(true);

        // The failed-but-not-gone answer: the flow still stands, so the seat
        // still holds — only an end-of-flow path may disarm.
        isFailing = true;
        expect(await flow.completeDevice()).toBe("pending");
        await drainWrites();
        expect(flow.flow()).not.toBeNull();
        expect(hasGuardedSubjects()).toBe(true);
    });

    it("gives the seat up when the screen goes away with the flow live", async () => {
        const { flow, dispose } = mountedFlow({
            begun: deviceFlow((Date.now() + 600_000) * 1000),
            complete: () =>
                Promise.resolve({
                    status: "pending",
                }),
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        // The in-app navigation the leave guard cannot see: the screen goes
        // away with its flow live, and the seat's own cleanup is what keeps
        // the guard from standing armed over the dead screen.
        dispose();
        await drainWrites();

        expect(hasGuardedSubjects()).toBe(false);
    });

    it("re-arms nothing when a held completion answers after the screen went away", async () => {
        // A holder rather than a bare `let`: the assignment happens inside
        // the stubbed completion below, and a bare variable would be narrowed
        // to `null` at the release site.
        const release: {
            current: ((response: CompleteProviderOauthResponse) => void) | null;
        } = {
            current: null,
        };
        const { flow, dispose } = mountedFlow({
            begun: deviceFlow((Date.now() + 600_000) * 1000),
            complete: () =>
                new Promise<CompleteProviderOauthResponse>((resolve) => {
                    release.current = resolve;
                }),
        });

        await flow.start();
        await settle();
        expect(hasGuardedSubjects()).toBe(true);

        // The poll is in flight when the screen goes away; its late answer
        // must act on nothing and arm nothing.
        const pendingAsk = flow.completeDevice();
        dispose();
        release.current?.({
            status: "succeeded",
            provider: reauthorizedProvider,
        });
        await settle();
        expect(await pendingAsk).toBe("done");
        await drainWrites();

        expect(hasGuardedSubjects()).toBe(false);
    });

    it("arms nothing when the begin settles after the screen went away", async () => {
        const { flow, dispose } = mountedFlow({
            begun: authorizationFlow,
            complete: () =>
                Promise.resolve({
                    status: "pending",
                }),
        });

        dispose();
        await flow.start();
        await settle();

        // The disposal guard stops the begin ahead of its success tail, so
        // no flow installs and no seat arms.
        expect(flow.flow()).toBeNull();
        expect(hasGuardedSubjects()).toBe(false);
    });
});

describe("OauthFlowView navigation warning", () => {
    const disposes = createDisposals();

    it("states the in-console navigation cost on the authorization-code step", async () => {
        const resets: number[] = [];
        disposes.push(
            render(
                () => (
                    <AuthorizeStep
                        flow={authorizationFlow}
                        onStartOver={() => {
                            resets.push(1);
                        }}
                    >
                        <p>the completion form</p>
                    </AuthorizeStep>
                ),
                document.body,
            ),
        );
        await settle();

        // The approval happens in another tab, so stepping elsewhere in the
        // console while authorizing is the move the visitor is tempted into —
        // and the one move a leave event cannot intercept, so the step states
        // its cost where it stands.
        expect(document.body.textContent).toContain(
            "navigating within the console discards the flow",
        );
    });

    it("states the in-console navigation cost on the device step", async () => {
        const resets: number[] = [];
        disposes.push(
            render(
                () => (
                    <DeviceStep
                        flow={deviceFlow((Date.now() + 600_000) * 1000)}
                        onComplete={() => Promise.resolve("done")}
                        onStartOver={() => {
                            resets.push(1);
                        }}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        expect(document.body.textContent).toContain(
            "navigating within the console discards the flow",
        );
    });
});
