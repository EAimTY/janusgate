import type { JSX } from "@solidjs/web";
import { createEffect, createSignal, For, onCleanup, Show } from "solid-js";
import { registerDirtyForm } from "/src/dirty-form-registry";
import type { ManagementApi } from "/src/management-api/client";
import type { ModelResponse, ModelRouteResponse } from "/src/management-api/generated/v0";
import { routingTokenError } from "/src/management-api/identifiers";
import { performManagementCall } from "/src/management-api/outcome";
import { hrefs } from "/src/router";
import { Button, ButtonLink, TextField } from "/src/ui/controls";
import { EmptyState, Notice } from "/src/ui/feedback";
import { PageHeader } from "/src/ui/layout";
import { describeCallFailure } from "./failure";
import {
    createDisposalGuard,
    createScreenRead,
    isSameModelRow,
    noticeOf,
    readAll,
    type ScreenNotice,
} from "./read";
import { ReadNotice } from "./read-notice";
import { SkeletonRows } from "./skeleton-rows";

type PublicationResult =
    | {
          readonly type: "published";
          readonly publicId: string;
          readonly routeId: number;
      }
    | {
          readonly type: "failed";
          readonly notice: ScreenNotice;
      };
type PublicationDraft = {
    readonly publicId: string;
    readonly isSelected: boolean;
    readonly result: PublicationResult | null;
};

/** Explicit independent route creations with per-row outcomes and no automatic renaming. */
export function BulkPublicationScreen(props: {
    readonly api: ManagementApi;
    readonly providerFilter: number | null;
}): JSX.Element {
    const [models, setModels] = createSignal<ReadonlyArray<ModelResponse>>([]);
    const [routes, setRoutes] = createSignal<ReadonlyArray<ModelRouteResponse>>([]);
    const [drafts, setDrafts] = createSignal<ReadonlyMap<number, PublicationDraft>>(new Map());
    const [isPublishing, setIsPublishing] = createSignal(false);
    const isDisposed = createDisposalGuard();
    let controller: AbortController | null = null;
    onCleanup(() => controller?.abort());
    const read = createScreenRead({
        load: async (signal) => {
            const [inventory, published] = await readAll(
                signal,
                {
                    run: (signal) =>
                        props.api.listModels(
                            props.providerFilter === null
                                ? {}
                                : {
                                      provider_id: props.providerFilter,
                                  },
                            signal,
                        ),
                },
                {
                    run: (signal) => props.api.listModelRoutes(signal),
                },
            );
            // Refresh keeps authored drafts and successful outcomes. Stable model identities
            // also keep focused inputs mounted when the same inventory answers again.
            const previous = new Map(
                models().map((model) => [
                    model.id,
                    model,
                ]),
            );
            setModels(
                inventory.models.map((model) => {
                    const old = previous.get(model.id);
                    return old !== undefined && isSameModelRow(old, model) ? old : model;
                }),
            );
            setRoutes(published.model_routes);
            setDrafts(
                (current) =>
                    new Map(
                        inventory.models.map((model) => [
                            model.id,
                            current.get(model.id) ?? {
                                publicId: model.upstream_id,
                                isSelected: false,
                                result: null,
                            },
                        ]),
                    ),
            );
        },
    });
    const draft = (id: number): PublicationDraft => {
        const value = drafts().get(id);
        if (value === undefined) throw new Error("Publication row has no draft.");
        return value;
    };
    const edit = (id: number, updates: Partial<PublicationDraft>): void => {
        setDrafts((current) => {
            const value = current.get(id);
            if (value === undefined) return current;
            const changed = new Map(current);
            changed.set(id, {
                ...value,
                ...updates,
            });
            return changed;
        });
    };
    const fieldError = (id: number): string | null => {
        const value = draft(id);
        if (!value.isSelected) return null;
        if (value.publicId === "") return "Public ID is required.";
        const tokenError = routingTokenError(value.publicId, "Public ID");
        if (tokenError !== null) return tokenError;
        return models().some(
            (other) =>
                other.id !== id &&
                draft(other.id).isSelected &&
                draft(other.id).publicId === value.publicId,
        )
            ? "Another selected row uses this public ID. Choose distinct explicit names."
            : null;
    };
    const registration = registerDirtyForm();
    createEffect(
        () =>
            models().some((model) => {
                const state = draft(model.id);
                return (
                    state.result?.type !== "published" &&
                    (state.isSelected || state.publicId !== model.upstream_id)
                );
            }),
        (isDirty) => registration.reportDirty(isDirty),
    );
    onCleanup(() => registration.unregister());
    const selectedCount = (): number =>
        models().filter((model) => draft(model.id).isSelected).length;
    const canPublish = (): boolean =>
        !isPublishing() &&
        selectedCount() > 0 &&
        models().every((model) => fieldError(model.id) === null);
    const publish = async (): Promise<void> => {
        if (!canPublish()) return;
        const selected = models()
            .filter((model) => draft(model.id).isSelected)
            .map((model) => ({
                modelId: model.id,
                publicId: draft(model.id).publicId,
            }));
        controller = new AbortController();
        setIsPublishing(true);
        try {
            for (const row of selected) {
                const request = {
                    public_id: row.publicId,
                    is_enabled: true,
                    target: {
                        type: "direct" as const,
                        model_id: row.modelId,
                    },
                };
                const outcome = await performManagementCall(() =>
                    props.api.createModelRoute(request, controller?.signal),
                );
                if (isDisposed() || outcome.status === "aborted") return;
                if (outcome.status === "failed") {
                    edit(row.modelId, {
                        result: {
                            type: "failed",
                            notice: noticeOf(
                                describeCallFailure(outcome.cause, {
                                    action: "create-route",
                                }),
                            ),
                        },
                    });
                } else {
                    edit(row.modelId, {
                        isSelected: false,
                        result: {
                            type: "published",
                            publicId: row.publicId,
                            routeId: outcome.result.model_route_id,
                        },
                    });
                    setRoutes((current) => [
                        ...current,
                        {
                            id: outcome.result.model_route_id,
                            is_automatic: false,
                            ...request,
                        },
                    ]);
                }
            }
        } finally {
            if (!isDisposed()) setIsPublishing(false);
            controller = null;
        }
    };
    const directCount = (modelId: number): number =>
        routes().filter(
            (route) => route.target.type === "direct" && route.target.model_id === modelId,
        ).length;
    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Create aliases"
                description="Choose physical models and author their public IDs. Each selection creates one enabled direct route; name conflicts are reported without renaming or overwriting."
                backHref={hrefs.models(props.providerFilter)}
                backLabel="Inventory"
            />
            <ReadNotice
                read={read}
                retry={read.reload}
            />
            <SkeletonRows
                read={read}
                rowCount={3}
                rowHeight="h-24"
            />
            <Show when={read.hasRead()}>
                <Show
                    when={models().length > 0}
                    fallback={
                        <EmptyState
                            glyph="stack"
                            title="No physical models to alias"
                            description="Discover or add inventory first."
                        />
                    }
                >
                    <ul class="flex flex-col gap-4">
                        <For each={models()}>
                            {(model) => {
                                const subject = `${model.provider} · ${model.upstream_id}`;
                                const result = (): PublicationResult | null =>
                                    draft(model.id).result;
                                const hasPublished = (): boolean => result()?.type === "published";
                                return (
                                    <li class="flex flex-col gap-3 rounded-xl border border-rule bg-surface p-4">
                                        <label class="flex items-center gap-3 text-[13.5px] font-medium">
                                            <input
                                                type="checkbox"
                                                class="size-4 accent-accent"
                                                checked={draft(model.id).isSelected}
                                                disabled={isPublishing() || hasPublished()}
                                                onChange={(event) =>
                                                    edit(model.id, {
                                                        isSelected: event.currentTarget.checked,
                                                    })
                                                }
                                            />
                                            {`Add alias for ${subject}`}
                                        </label>
                                        <p class="text-[12.5px] text-ink-faint">
                                            {directCount(model.id) === 0
                                                ? "No direct routes"
                                                : `${directCount(model.id)} direct routes`}{" "}
                                            ·{" "}
                                            {model.discovery_state === "vanished"
                                                ? "Model vanished; its new route will be suspended"
                                                : model.is_enabled
                                                  ? "Model enabled"
                                                  : "Model disabled"}
                                        </p>
                                        <TextField
                                            label={`Public ID for ${subject}`}
                                            value={draft(model.id).publicId}
                                            onInput={(publicId) =>
                                                edit(model.id, {
                                                    publicId,
                                                    result: null,
                                                })
                                            }
                                            error={fieldError(model.id)}
                                            isDisabled={isPublishing() || hasPublished()}
                                        />
                                        <Show
                                            when={result()}
                                            keyed
                                        >
                                            {(outcome) =>
                                                outcome.type === "failed" ? (
                                                    <Notice
                                                        tone={outcome.notice.tone}
                                                        title={outcome.notice.title}
                                                        detail={outcome.notice.detail}
                                                        fix={outcome.notice.fix}
                                                    />
                                                ) : (
                                                    <p class="text-[13px] text-ok">
                                                        {`Alias created: ${outcome.publicId}`} ·{" "}
                                                        <ButtonLink
                                                            href={hrefs.modelRouteEdit(
                                                                outcome.routeId,
                                                            )}
                                                        >
                                                            Open route
                                                        </ButtonLink>
                                                    </p>
                                                )
                                            }
                                        </Show>
                                    </li>
                                );
                            }}
                        </For>
                    </ul>
                    <div class="flex flex-wrap items-center gap-3">
                        <Button
                            tone="primary"
                            isBusy={isPublishing()}
                            isDisabled={!canPublish()}
                            onClick={() => void publish()}
                        >
                            Create selected aliases
                        </Button>
                        <p class="text-[12.5px] text-ink-faint">
                            {selectedCount()} selected. Successful rows will not be submitted again.
                        </p>
                    </div>
                </Show>
            </Show>
        </div>
    );
}
