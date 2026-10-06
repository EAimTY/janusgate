import { render } from "@solidjs/web";
import { createRoot } from "solid-js";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
    CompactionSupportOverrideFields,
    compactionOverrideSelection,
    normalizedCompactionOverride,
} from "../features/models/model-form";
import { createOauthFlow } from "../features/providers/oauth-flow";
import { DeviceStep } from "../features/providers/oauth-flow-view";
import { hasGuardedSubjects } from "../leave-guard-registry";
import { ErrorCode } from "../management-api/generated/v0";
import { performManagementCall } from "../management-api/outcome";
import { ApiKeyEditScreen } from "../screens/api-keys/api-key-edit-screen";
import {
    apiKeyRow,
    buttonWithText,
    gatewayFailure,
    labeledInput,
    settle,
    stubListings,
    stubManagementApi,
    typeInto,
} from "../test-support/management-api";
import { setLanguagePreference } from "./index";

const disposals: Array<() => void> = [];
afterEach(async () => {
    for (const dispose of disposals.splice(0)) dispose();
    vi.useRealTimers();
    await setLanguagePreference("en");
    document.body.replaceChildren();
});
describe("language switching inside an existing administration form", () => {
    it("retains the form, draft, armed confirmation and completed read without new management calls", async () => {
        const reads = vi.fn(async () => ({
            api_keys: [
                apiKeyRow(),
            ],
        }));
        const api = stubManagementApi({
            ...stubListings({}),
            listApiKeys: reads,
        });
        disposals.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        const name = labeledInput("Name");
        typeInto(name, "混合 العربية draft");
        buttonWithText("Delete…").click();
        await settle();
        const focused = document.activeElement;
        const count = reads.mock.calls.length;
        await setLanguagePreference("zh-CN");
        await settle();
        expect(labeledInput("名称")).toBe(name);
        expect(name.value).toBe("混合 العربية draft");
        expect(document.activeElement).toBe(focused);
        expect(reads).toHaveBeenCalledTimes(count);
        expect(document.body.textContent).toContain("删除此密钥");
    });
    it("rephrases an existing diagnosis in place and preserves its raw gateway detail", async () => {
        const detail = "upstream said <script> [[providerLink]] العربية";
        const reads = vi.fn(async () => ({
            api_keys: [
                apiKeyRow(),
            ],
        }));
        const api = stubManagementApi({
            ...stubListings({}),
            listApiKeys: reads,
            updateApiKey: async () => {
                throw gatewayFailure(detail, ErrorCode.Conflict, 409);
            },
        });
        disposals.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        typeInto(labeledInput("Name"), "edited");
        await settle();
        buttonWithText("Save").click();
        await settle();
        expect(document.body.textContent).toContain("The gateway refused");
        const count = reads.mock.calls.length;
        await setLanguagePreference("zh-CN");
        await settle();
        expect(document.body.textContent).toContain("标识已被占用");
        expect(document.body.textContent).toContain(detail);
        expect(document.querySelector("script")).toBeNull();
        expect(reads).toHaveBeenCalledTimes(count);
    });
    it("rephrases validation and a completed save without replacing the form or repeating work", async () => {
        let row = apiKeyRow();
        const reads = vi.fn(async () => ({
            api_keys: [
                row,
            ],
        }));
        const writes = vi.fn(async (request: { name: string }) => {
            row = {
                ...row,
                name: request.name,
            };
            return;
        });
        const api = stubManagementApi({
            ...stubListings({}),
            listApiKeys: reads,
            updateApiKey: writes,
        });
        disposals.push(
            render(
                () => (
                    <ApiKeyEditScreen
                        api={api}
                        apiKeyId={7}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        const name = labeledInput("Name");
        typeInto(name, "");
        await settle();
        expect(document.body.textContent).toContain("Name cannot be empty");
        const initialReads = reads.mock.calls.length;
        await setLanguagePreference("zh-CN");
        await settle();
        expect(labeledInput("名称")).toBe(name);
        expect(document.body.textContent).toContain("名称不能为空");
        expect(reads).toHaveBeenCalledTimes(initialReads);
        typeInto(name, "العربية 中文 <script> [[name]]");
        await settle();
        buttonWithText("保存").click();
        await settle();
        expect(writes).toHaveBeenCalledTimes(1);
        const completedReads = reads.mock.calls.length;
        await setLanguagePreference("en");
        await settle();
        expect(document.body.textContent).toContain("saved.");
        expect(document.body.textContent).toContain(row.name);
        expect(document.querySelector("script")).toBeNull();
        expect(reads).toHaveBeenCalledTimes(completedReads);
        expect(writes).toHaveBeenCalledTimes(1);
    });
    it("keeps selected wire options while their labels change", async () => {
        const selected = compactionOverrideSelection({
            responses_standalone_compact: "supported",
        });
        const change = vi.fn();
        disposals.push(
            render(
                () => (
                    <CompactionSupportOverrideFields
                        hasOverride={true}
                        onHasOverrideToggle={change}
                        selected={selected}
                        onSelectedChange={change}
                        discovered={null}
                    />
                ),
                document.body,
            ),
        );
        await settle();
        const select = [
            ...document.querySelectorAll("select"),
        ].find((element) => element.value === "supported");
        expect(select).toBeDefined();
        const label = select?.selectedOptions[0]?.textContent;
        const normalized = normalizedCompactionOverride(true, selected);
        await setLanguagePreference("zh-CN");
        await settle();
        expect([
            ...document.querySelectorAll("select"),
        ]).toContain(select);
        expect(select?.value).toBe("supported");
        expect(select?.selectedOptions[0]?.textContent).not.toBe(label);
        expect(normalizedCompactionOverride(true, selected)).toEqual(normalized);
        expect(change).not.toHaveBeenCalled();
    });
    it("retains OAuth state and the polling schedule through a language switch", async () => {
        vi.useFakeTimers();
        const facts = {
            grant: "device" as const,
            state: "state-العربية",
            expires_at: (Date.now() + 600_000) * 1000,
            user_code: "WDJB-MJHT",
            verification_uri_complete: "https://auth.example.test/activate?user_code=WDJB-MJHT",
        };
        const poll = vi.fn(async () => ({
            status: "pending" as const,
        }));
        const flow = createRoot((dispose) => {
            disposals.push(dispose);
            return createOauthFlow({
                api: stubManagementApi({
                    completeProviderOauth: poll,
                }),
                startAction: "start-oauth",
                completeAction: "complete-oauth",
                begin: async () => ({
                    status: "called",
                    outcome: await performManagementCall(() => Promise.resolve(facts)),
                }),
                onSucceeded: () => {},
            });
        });
        await flow.start();
        disposals.push(
            render(
                () => (
                    <DeviceStep
                        flow={facts}
                        onComplete={flow.completeDevice}
                        onStartOver={flow.reset}
                    />
                ),
                document.body,
            ),
        );
        await vi.advanceTimersByTimeAsync(0);
        const live = flow.flow();
        const code = [
            ...document.querySelectorAll("bdi"),
        ].find((element) => element.textContent === facts.user_code);
        expect(poll).toHaveBeenCalledTimes(1);
        expect(hasGuardedSubjects()).toBe(true);
        await setLanguagePreference("zh-CN");
        await vi.advanceTimersByTimeAsync(0);
        expect(flow.flow()).toBe(live);
        expect(code?.isConnected).toBe(true);
        expect(document.body.textContent).toContain(facts.user_code);
        expect(
            document.querySelector(`a[href="${facts.verification_uri_complete}"]`),
        ).not.toBeNull();
        expect(poll).toHaveBeenCalledTimes(1);
        expect(hasGuardedSubjects()).toBe(true);
        await vi.advanceTimersByTimeAsync(3000);
        await vi.advanceTimersByTimeAsync(0);
        expect(poll).toHaveBeenCalledTimes(2);
    });
});
