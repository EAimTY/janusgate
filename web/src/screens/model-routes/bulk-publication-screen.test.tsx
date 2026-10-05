import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { ManagementApiError } from "/src/management-api/errors";
import { type CreateModelRouteRequest, ErrorCode } from "/src/management-api/generated/v0";
import { requestRefresh } from "/src/refresh-state";
import {
    buttonWithText,
    createDisposals,
    labeledCheckbox,
    labeledInput,
    modelRow,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "/src/test-support/management-api";
import { BulkPublicationScreen } from "./bulk-publication-screen";

const disposes = createDisposals();

describe("bulk publication", () => {
    it("reports conflicts per authored name, preserves successes, and retries only the corrected failure", async () => {
        const sent: CreateModelRouteRequest[] = [];
        const api = stubManagementApi({
            ...stubListings({
                models: [
                    modelRow({
                        id: 9,
                        upstream_id: "first",
                    }),
                    modelRow({
                        id: 12,
                        upstream_id: "second",
                    }),
                ],
            }),
            createModelRoute: (request) => {
                sent.push(request);
                return request.public_id === "taken"
                    ? Promise.reject(
                          new ManagementApiError("gateway", "Public ID already exists", {
                              errno: ErrorCode.Conflict,
                              httpStatus: 409,
                          }),
                      )
                    : Promise.resolve({
                          model_route_id: sent.length,
                      });
            },
        });
        disposes.push(
            render(
                () => (
                    <BulkPublicationScreen
                        api={api}
                        providerFilter={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        labeledCheckbox("Add alias for local · first").click();
        labeledCheckbox("Add alias for local · second").click();
        typeInto(labeledInput("Public ID for local · first"), "taken");
        typeInto(labeledInput("Public ID for local · second"), "second-public");
        await settle();
        requestRefresh();
        await settle();
        expect(labeledInput("Public ID for local · first").value).toBe("taken");
        buttonWithText("Create selected aliases").click();
        await settle();
        expect(sent).toEqual([
            {
                public_id: "taken",
                is_enabled: true,
                target: {
                    type: "direct",
                    model_id: 9,
                },
            },
            {
                public_id: "second-public",
                is_enabled: true,
                target: {
                    type: "direct",
                    model_id: 12,
                },
            },
        ]);
        expect(document.body.textContent).toContain("Public ID already exists");
        expect(document.body.textContent).toContain("Alias created: second-public");
        typeInto(labeledInput("Public ID for local · first"), "corrected");
        await settle();
        buttonWithText("Create selected aliases").click();
        await settle();
        expect(sent).toHaveLength(3);
        expect(sent[2]).toEqual({
            public_id: "corrected",
            is_enabled: true,
            target: {
                type: "direct",
                model_id: 9,
            },
        });
        expect(document.body.textContent).toContain("Alias created: corrected");
    });
    it("stops a batch after navigation even if the first creation completes late", async () => {
        const sent: CreateModelRouteRequest[] = [];
        let finish: ((value: { model_route_id: number }) => void) | undefined;
        const api = stubManagementApi({
            ...stubListings({
                models: [
                    modelRow({
                        id: 9,
                        upstream_id: "first",
                    }),
                    modelRow({
                        id: 12,
                        upstream_id: "second",
                    }),
                ],
            }),
            createModelRoute: (request) => {
                sent.push(request);
                return new Promise((resolve) => {
                    finish = resolve;
                });
            },
        });
        const dispose = render(
            () => (
                <BulkPublicationScreen
                    api={api}
                    providerFilter={null}
                />
            ),
            document.body,
        );
        disposes.push(dispose);
        await settle();
        labeledCheckbox("Add alias for local · first").click();
        labeledCheckbox("Add alias for local · second").click();
        await settle();
        buttonWithText("Create selected aliases").click();
        await settle();
        expect(sent).toHaveLength(1);
        dispose();
        if (finish === undefined) throw new Error("The first creation did not start.");
        finish({
            model_route_id: 17,
        });
        await settle();
        expect(sent).toHaveLength(1);
        expect(document.body.textContent).not.toContain("Published as first");
    });
});
