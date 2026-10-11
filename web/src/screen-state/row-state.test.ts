import { createRoot, createSignal } from "solid-js";
import { describe, expect, test } from "vitest";
import { hasGuardedSubjects } from "/src/leave-guard-registry";
import type { ModelResponse } from "/src/management-api/generated/v0";
import { createEditRow, createPinnedRow } from "./row-state";

/**
 * The edit screens' held row: a dirty or save-settling form keeps the row's
 * identity whatever a read answers, a clean form adopts a genuinely changed
 * answer while equal content keeps the identity, a read answering null ends
 * the hold, and the save latch covers the window between a save and its own
 * reload. The identity-pinning memo beside it holds the same rule without
 * the hold condition, and the dirt report mirrors into the leave-guard
 * registry.
 */

/** Advances staged writes. */
async function tick(): Promise<void> {
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function model(overrides: Partial<ModelResponse> = {}): ModelResponse {
    return {
        id: 10,
        provider_id: 1,
        provider: "acme",
        upstream_id: "gpt-x",
        is_enabled: true,
        discovery_state: "discovered",
        thinking_levels: null,
        max_input_tokens: null,
        max_output_tokens: null,
        thinking_levels_override: null,
        max_input_tokens_override: null,
        max_output_tokens_override: null,
        compaction_support: null,
        compaction_support_override: null,
        ...overrides,
    };
}

describe("createEditRow", () => {
    test("keeps a content-equal re-read's identity, so a refresh never remounts", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const { row } = createEditRow(latest, (previous, fresh) =>
                isSameModel(previous, fresh),
            );
            await tick();

            const before = row();
            setLatest(model());
            await tick();

            expect(row()).toBe(before);
            dispose();
        });
    });

    test("adopts a genuinely changed answer once the form is clean", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const { row } = createEditRow(latest, (previous, fresh) =>
                isSameModel(previous, fresh),
            );
            await tick();

            setLatest(
                model({
                    is_enabled: false,
                }),
            );
            await tick();

            expect(row()?.is_enabled).toBe(false);
            dispose();
        });
    });

    test("holds the row back while the form stands dirty, and releases the hold on clean", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const edit = createEditRow(latest, (previous, fresh) => isSameModel(previous, fresh));
            await tick();

            edit.reportFormDirty(true);
            await tick();

            const held = edit.row();
            setLatest(
                model({
                    is_enabled: false,
                }),
            );
            await tick();

            expect(edit.row()).toBe(held);
            expect(edit.isRowHeld()).toBe(true);

            // Reverting — a clean report — adopts the changed answer.
            edit.reportFormDirty(false);
            await tick();

            expect(edit.row()?.is_enabled).toBe(false);
            expect(edit.isRowHeld()).toBe(false);
            dispose();
        });
    });

    test("holds the row from a succeeded save until its own reload lands", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const edit = createEditRow(latest, (previous, fresh) => isSameModel(previous, fresh));
            await tick();

            edit.reportSaved();
            await tick();

            const held = edit.row();
            setLatest(
                model({
                    is_enabled: false,
                }),
            );
            await tick();

            // The listing is stale by construction: the save overwrote exactly what it shows.
            expect(edit.row()).toBe(held);

            edit.reportReloaded();
            await tick();

            expect(edit.row()?.is_enabled).toBe(false);
            dispose();
        });
    });

    test("a read answering null ends the hold, deletion ending the form", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const edit = createEditRow(latest, (previous, fresh) => isSameModel(previous, fresh));
            await tick();

            edit.reportFormDirty(true);
            await tick();

            setLatest(null);
            await tick();

            expect(edit.row()).toBeNull();
            expect(edit.isRowHeld()).toBe(false);
            dispose();
        });
    });

    test("mirrors the form's dirt into the leave-guard registry and dies clean", async () => {
        await createRoot(async (dispose) => {
            const [latest] = createSignal<ModelResponse | null>(model());
            const edit = createEditRow(latest, (previous, fresh) => isSameModel(previous, fresh));
            await tick();

            expect(hasGuardedSubjects()).toBe(false);
            edit.reportFormDirty(true);
            await tick();
            expect(hasGuardedSubjects()).toBe(true);

            dispose();
            await tick();
            // A mount that dies dirty leaves the guard never armed behind it.
            expect(hasGuardedSubjects()).toBe(false);
        });
    });
});

describe("createPinnedRow", () => {
    test("pins identity across equal content and adopts a changed answer, no hold", async () => {
        await createRoot(async (dispose) => {
            const [latest, setLatest] = createSignal<ModelResponse | null>(model());
            const pinned = createPinnedRow(latest, (previous, fresh) =>
                isSameModel(previous, fresh),
            );
            await tick();

            const before = pinned();
            setLatest(model());
            await tick();
            expect(pinned()).toBe(before);

            setLatest(
                model({
                    is_enabled: false,
                }),
            );
            await tick();
            expect(pinned()?.is_enabled).toBe(false);

            setLatest(null);
            await tick();
            expect(pinned()).toBeNull();
            dispose();
        });
    });
});

/** The model row equality the screens use, spelled locally over the columns. */
function isSameModel(previous: ModelResponse, fresh: ModelResponse): boolean {
    return JSON.stringify(previous) === JSON.stringify(fresh);
}
