import { render } from "@solidjs/web";
import { afterEach, describe, expect, it } from "vitest";
import type { ManagementApi } from "/src/management-api/client";
import type { CreateModelRequest, ProviderResponse } from "/src/management-api/generated/v0";
import { hrefs } from "/src/router";
import { ModelNewScreen } from "/src/screens/model-new-screen";
import {
    bodyText,
    buttonWithText,
    labeledInput,
    labeledSelect,
    linkTo,
    modelRow,
    providerRow,
    settle,
    stubManagementApi,
    typeInto,
} from "./screen-test-support";

/**
 * A management API standing in for the gateway: the providers listing answers
 * from the given override when one is supplied — a rejection factory keeps
 * the rejection from existing before a handler is attached — and from the
 * given rows otherwise; `models:create` records its request for assertions on
 * what the form sent and answers with the given row; every other route
 * refuses, so an unexpected call fails the test loudly instead of hanging it.
 */
function screenApi(
    providers: ReadonlyArray<ProviderResponse>,
    options: {
        readonly listing?: () => Promise<{
            providers: ProviderResponse[];
        }>;
        readonly createdModel?: ReturnType<typeof modelRow>;
        readonly sent?: CreateModelRequest[];
    } = {},
): ManagementApi {
    return stubManagementApi({
        listProviders: () =>
            options.listing?.() ??
            Promise.resolve({
                providers: [
                    ...providers,
                ],
            }),
        createModel: (request) => {
            options.sent?.push(request);
            return Promise.resolve({
                model: options.createdModel ?? modelRow(),
            });
        },
    });
}

/** One option's value, label, and selected state, as the DOM holds them. */
function optionRepr(option: HTMLOptionElement): {
    value: string;
    label: string;
    isSelected: boolean;
} {
    return {
        value: option.value,
        label: option.textContent ?? "",
        isSelected: option.selected,
    };
}

describe("ModelNewScreen creation", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    function mountScreen(
        api: ManagementApi,
        providerFilter: number | null = null,
        upstreamId: string | null = null,
    ): void {
        disposes.push(
            render(
                () => (
                    <ModelNewScreen
                        api={api}
                        providerFilter={providerFilter}
                        upstreamId={upstreamId}
                    />
                ),
                document.body,
            ),
        );
    }

    it("auto-selects the sole provider and opens the gate once a routable upstream ID stands", async () => {
        mountScreen(
            screenApi([
                providerRow({
                    id: 5,
                    name: "solo",
                    model_prefix: "solo-",
                }),
            ]),
        );
        await settle();

        // With one provider there is nothing to pick: the select stands on
        // it, past the placeholder, before any interaction.
        const options = Array.from(labeledSelect("Provider").options).map(optionRepr);

        expect(options).toEqual([
            {
                value: "",
                label: "Choose a provider",
                isSelected: false,
            },
            {
                value: "5",
                label: "solo",
                isSelected: true,
            },
        ]);

        // The route carried no claim link, so the field states its plain
        // verbatim rule, never the claim-back wording.
        expect(bodyText()).toContain(
            "Sent upstream verbatim — letters, numbers, and symbols only.",
        );
        expect(bodyText()).not.toContain(
            "Carried here from a model the upstream no longer reports",
        );

        // The selection alone composes no identity: the gate stays closed
        // until the upstream ID stands, and a non-routing token keeps it
        // closed while naming itself where it was typed.
        expect(buttonWithText("Create model").disabled).toBe(true);

        typeInto(labeledInput("Upstream ID"), "has space");
        await settle();
        expect(bodyText()).toContain(
            'Upstream ID accepts letters, numbers, and symbols only — no spaces, non-ASCII text, "/", or ":".',
        );
        expect(buttonWithText("Create model").disabled).toBe(true);

        typeInto(labeledInput("Upstream ID"), "gpt-4o");
        await settle();
        expect(buttonWithText("Create model").disabled).toBe(false);
    });

    it("leaves the provider unchosen when several stand and keeps submission closed", async () => {
        mountScreen(
            screenApi([
                providerRow(),
                providerRow({
                    id: 2,
                    name: "other",
                    model_prefix: "other-",
                }),
            ]),
        );
        await settle();

        // The auto-select is the single-provider courtesy only: with a real
        // choice open, the select rests on its placeholder and the gate holds.
        expect(labeledSelect("Provider").value).toBe("");
        expect(buttonWithText("Create model").disabled).toBe(true);
    });

    it("renders the route-named provider as fixed, with no choice left to make", async () => {
        mountScreen(
            screenApi([
                providerRow({
                    id: 3,
                    name: "alpha",
                    model_prefix: "alpha-",
                }),
                providerRow({
                    id: 5,
                    name: "beta",
                    model_prefix: "beta-",
                }),
            ]),
            3,
        );
        await settle();

        // A provider named by the route and present in the listing replaces
        // the select outright: its name stands as text and no choice control
        // exists to contradict it.
        expect(() => labeledSelect("Provider")).toThrow("No select labeled Provider");
        expect(bodyText()).toContain("alpha");

        // The page answers to its provider, not the models listing: the way
        // back is the provider's own page.
        expect(linkTo("/providers/3").textContent).toContain("Provider");

        // The fixed selection feeds the gate directly — only the upstream ID
        // is missing, and naming a routable one opens submission.
        expect(buttonWithText("Create model").disabled).toBe(true);
        typeInto(labeledInput("Upstream ID"), "gpt-4o");
        await settle();
        expect(buttonWithText("Create model").disabled).toBe(false);
    });

    it("prefills the claim's upstream ID and states the claim-back wording", async () => {
        mountScreen(
            screenApi([
                providerRow(),
                providerRow({
                    id: 2,
                    name: "other",
                    model_prefix: "other-",
                }),
            ]),
            null,
            "lost-model-7",
        );
        await settle();

        // The vanished model's link prefills its upstream ID — a prefill,
        // never a force: the field stays editable and the choice of provider
        // stays open.
        expect(labeledInput("Upstream ID").value).toBe("lost-model-7");

        // The hint names what submitting will do, in place of the plain
        // verbatim rule the unclaimed field states.
        expect(bodyText()).toContain(
            "Carried here from a model the upstream no longer reports — submitting claims it back under the settings below.",
        );
        expect(bodyText()).not.toContain("Sent upstream verbatim");
    });

    it("sends the composed request and lands on the new model's page", async () => {
        const sent: CreateModelRequest[] = [];
        mountScreen(
            screenApi(
                [
                    providerRow({
                        id: 5,
                        name: "solo",
                        model_prefix: "solo-",
                    }),
                ],
                {
                    createdModel: modelRow({
                        id: 11,
                        provider_id: 5,
                        provider: "solo",
                        upstream_id: "gpt-4o",
                        public_id: "solo-gpt-4o",
                    }),
                    sent,
                },
            ),
        );
        await settle();
        typeInto(labeledInput("Upstream ID"), "gpt-4o");
        await settle();
        buttonWithText("Create model").click();
        await settle();

        // The auto-selected provider is the one the write names; the initial
        // state is enabled with no override, so the composed ID stands.
        expect(sent).toEqual([
            {
                provider_id: 5,
                upstream_id: "gpt-4o",
                id_override: null,
                is_enabled: true,
            },
        ]);
        expect(window.location.pathname).toBe(hrefs.modelDetail(11));
    });
});

describe("ModelNewScreen failed first read", () => {
    const disposes: Array<() => void> = [];

    afterEach(() => {
        for (const dispose of disposes.reverse()) {
            dispose();
        }
        disposes.length = 0;
    });

    it("states the diagnosis and recovers in place through Read again", async () => {
        let isListingFailing = true;
        let listingReads = 0;
        disposes.push(
            render(
                () => (
                    <ModelNewScreen
                        api={screenApi(
                            [
                                providerRow({
                                    id: 5,
                                    name: "solo",
                                    model_prefix: "solo-",
                                }),
                            ],
                            {
                                listing: () => {
                                    listingReads += 1;

                                    return isListingFailing
                                        ? Promise.reject(new Error("the listing refused"))
                                        : Promise.resolve({
                                              providers: [
                                                  providerRow({
                                                      id: 5,
                                                      name: "solo",
                                                      model_prefix: "solo-",
                                                  }),
                                              ],
                                          });
                                },
                            },
                        )}
                        providerFilter={null}
                        upstreamId={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();

        // The failed first read states its diagnosis — and, wired like every
        // sibling harness screen, the Read again action beside it (README §6.3):
        // first contact with a down gateway recovers in place.
        expect(bodyText()).toContain("the listing refused");
        expect(buttonWithText("Read again").disabled).toBe(false);

        // The action re-runs the screen's own read in place: the listing
        // answers this time, the notice is recovered away, and the form
        // stands — a full recovery, no trip to the shell's Refresh.
        isListingFailing = false;
        buttonWithText("Read again").click();
        await settle();

        expect(listingReads).toBe(2);
        expect(bodyText()).not.toContain("the listing refused");
        expect(labeledSelect("Provider").value).toBe("5");
    });
});
