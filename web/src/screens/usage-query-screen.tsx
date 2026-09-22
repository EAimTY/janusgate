import type { JSX } from "@solidjs/web";
import { createMemo, createSignal, createTrackedEffect, For, Show } from "solid-js";
import { formatTokenCount, parseDateTimeInputToMicros } from "/src/format";
import type { ManagementApi } from "/src/management-api/client";
import type {
    QueryUsageRequest,
    UsageBucket,
    UsageGroupBy,
} from "/src/management-api/generated/v0";
import { performManagementCall } from "/src/management-api/outcome";
import { USAGE_GROUP_BY_OPTIONS } from "/src/management-api/vocabulary";
import { refreshRequest, reportRefresh } from "/src/refresh-state";
import { hrefs } from "/src/router";
import { describeCallFailure } from "/src/screens/failure";
import {
    type UsageEntities,
    usageApiKeyLabel,
    usageModelLabel,
    usageProviderLabel,
} from "/src/screens/usage-labels";
import { Button, SelectField, TextField } from "/src/ui/controls";
import { Notice, Skeleton } from "/src/ui/feedback";
import { Card, CardHeader, PageHeader } from "/src/ui/layout";

/**
 * Request accounting as aggregates: one bucket per day, model, provider, or
 * API key inside the window and filters. The bucketing dimension is the
 * request's typed enum; every other field narrows what the gateway counts,
 * and labels resolve through the entity lists the screen already holds.
 */
export function UsageQueryScreen(props: { readonly api: ManagementApi }): JSX.Element {
    const [entities, setEntities] = createSignal<UsageEntities>({
        providers: [],
        models: [],
        apiKeys: [],
    });
    const [groupBy, setGroupBy] = createSignal<UsageGroupBy>("day");
    const [fromInput, setFromInput] = createSignal("");
    const [toInput, setToInput] = createSignal("");
    const [providerId, setProviderId] = createSignal("");
    const [modelId, setModelId] = createSignal("");
    const [apiKeyId, setApiKeyId] = createSignal("");
    const [buckets, setBuckets] = createSignal<ReadonlyArray<UsageBucket>>([]);
    const [hasRead, setHasRead] = createSignal(false);
    const [isReading, setIsReading] = createSignal(true);
    const [isQuerying, setIsQuerying] = createSignal(false);
    const [failure, setFailure] = createSignal<{
        readonly title: string;
        readonly detail: string;
        readonly fix: string | null;
    } | null>(null);

    let controller: AbortController | null = null;

    const queryRequest = (): QueryUsageRequest => {
        const from = parseDateTimeInputToMicros(fromInput());
        const to = parseDateTimeInputToMicros(toInput());

        return {
            group_by: groupBy(),
            ...(from === null
                ? {}
                : {
                      from,
                  }),
            ...(to === null
                ? {}
                : {
                      to,
                  }),
            ...(providerId() === ""
                ? {}
                : {
                      provider_id: Number(providerId()),
                  }),
            ...(modelId() === ""
                ? {}
                : {
                      model_id: Number(modelId()),
                  }),
            ...(apiKeyId() === ""
                ? {}
                : {
                      api_key_id: Number(apiKeyId()),
                  }),
        };
    };

    const run = async (signal: AbortSignal): Promise<boolean> => {
        const outcome = await performManagementCall(() =>
            props.api.queryUsage(queryRequest(), signal),
        );

        if (outcome.status === "aborted") {
            return false;
        }

        if (outcome.status === "failed") {
            const diagnosis = describeCallFailure(outcome.cause, "query-usage");

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            return false;
        }

        setFailure(null);
        setBuckets(outcome.result.buckets);
        return true;
    };

    const read = async (): Promise<void> => {
        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsReading(true);

        const [providersOutcome, modelsOutcome, apiKeysOutcome] = await Promise.all([
            performManagementCall(() => props.api.listProviders(call.signal)),
            performManagementCall(() => props.api.listModels({}, call.signal)),
            performManagementCall(() => props.api.listApiKeys(call.signal)),
        ]);

        if (
            providersOutcome.status === "aborted" ||
            modelsOutcome.status === "aborted" ||
            apiKeysOutcome.status === "aborted"
        ) {
            return;
        }

        const failed =
            providersOutcome.status === "failed"
                ? providersOutcome
                : modelsOutcome.status === "failed"
                  ? modelsOutcome
                  : apiKeysOutcome.status === "failed"
                    ? apiKeysOutcome
                    : null;

        if (failed !== null) {
            const diagnosis = describeCallFailure(failed.cause);

            setFailure({
                title: diagnosis.headline,
                detail: diagnosis.detail,
                fix: diagnosis.fix,
            });
            setIsReading(false);
            return;
        }

        setEntities({
            providers: providersOutcome.status === "ok" ? providersOutcome.result.providers : [],
            models: modelsOutcome.status === "ok" ? modelsOutcome.result.models : [],
            apiKeys: apiKeysOutcome.status === "ok" ? apiKeysOutcome.result.api_keys : [],
        });

        if (await run(call.signal)) {
            setHasRead(true);
            reportRefresh();
        }

        setIsReading(false);
    };

    const submit = async (event: SubmitEvent): Promise<void> => {
        event.preventDefault();

        if (isQuerying()) {
            return;
        }

        controller?.abort();

        const call = new AbortController();
        controller = call;
        setIsQuerying(true);

        if (await run(call.signal)) {
            setHasRead(true);
            reportRefresh();
        }

        setIsQuerying(false);
    };

    createTrackedEffect(() => {
        refreshRequest();
        void read();

        return () => controller?.abort();
    });

    const groupOptions = createMemo(() =>
        USAGE_GROUP_BY_OPTIONS.map((option) => ({
            value: option.value,
            label: option.label,
        })),
    );

    const providerOptions = createMemo(() => [
        {
            value: "",
            label: "All providers",
        },
        ...entities().providers.map((row) => ({
            value: String(row.id),
            label: row.name,
        })),
    ]);

    const modelOptions = createMemo(() => [
        {
            value: "",
            label: "All models",
        },
        ...entities().models.map((row) => ({
            value: String(row.id),
            label: row.public_id,
        })),
    ]);

    const apiKeyOptions = createMemo(() => [
        {
            value: "",
            label: "All API keys",
        },
        ...entities().apiKeys.map((row) => ({
            value: String(row.id),
            label: row.name,
        })),
    ]);

    return (
        <div class="rise-in flex flex-col gap-5">
            <PageHeader
                title="Usage aggregates"
                description="One bucket per grouping inside the window and filters — counts and token sums, never content."
                backHref={hrefs.usage()}
                backLabel="Usage"
            />

            <Show when={failure()}>
                {(current) => (
                    <Notice
                        tone="bad"
                        title={current().title}
                        detail={current().detail}
                        fix={current().fix}
                    />
                )}
            </Show>

            <Card>
                <CardHeader title="Question" />
                <form
                    novalidate={true}
                    class="mt-4 flex flex-col gap-4"
                    onSubmit={(event) => void submit(event)}
                >
                    <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
                        <SelectField
                            label="Group by"
                            value={groupBy()}
                            options={groupOptions()}
                            onChange={setGroupBy}
                            isDisabled={isQuerying()}
                        />
                        <TextField
                            label="From"
                            type="datetime-local"
                            value={fromInput()}
                            onInput={setFromInput}
                            isDisabled={isQuerying()}
                            hint="Inclusive, in this browser's zone. Blank sets no bound."
                        />
                        <TextField
                            label="To"
                            type="datetime-local"
                            value={toInput()}
                            onInput={setToInput}
                            isDisabled={isQuerying()}
                            hint="Exclusive, in this browser's zone. Blank sets no bound."
                        />
                        <SelectField
                            label="Provider"
                            value={providerId()}
                            options={providerOptions()}
                            onChange={setProviderId}
                            isDisabled={isQuerying()}
                        />
                        <SelectField
                            label="Model"
                            value={modelId()}
                            options={modelOptions()}
                            onChange={setModelId}
                            isDisabled={isQuerying()}
                        />
                        <SelectField
                            label="API key"
                            value={apiKeyId()}
                            options={apiKeyOptions()}
                            onChange={setApiKeyId}
                            isDisabled={isQuerying()}
                        />
                    </div>
                    <div>
                        <Button
                            type="submit"
                            tone="primary"
                            isBusy={isQuerying()}
                        >
                            {isQuerying() ? "Querying…" : "Run query"}
                        </Button>
                    </div>
                </form>
            </Card>

            <Show when={isReading() && !hasRead() && failure() === null}>
                <Skeleton class="h-64 w-full rounded-2xl" />
            </Show>

            <Show when={hasRead()}>
                <Show
                    when={buckets().length > 0}
                    fallback={
                        <Notice
                            tone="info"
                            title="No rows in this window."
                            detail="Nothing recorded matches these filters — widen the window or clear a filter."
                        />
                    }
                >
                    <Card>
                        <div class="overflow-x-auto">
                            <table class="tnum w-full min-w-[640px] text-left text-[13px]">
                                <thead>
                                    <tr class="text-ink-dim">
                                        <BucketHeader label="Bucket" />
                                        <BucketHeader
                                            label="Requests"
                                            isNumeric
                                        />
                                        <BucketHeader
                                            label="Input"
                                            isNumeric
                                        />
                                        <BucketHeader
                                            label="Output"
                                            isNumeric
                                        />
                                        <BucketHeader
                                            label="Cached"
                                            isNumeric
                                        />
                                        <BucketHeader
                                            label="Cache write"
                                            isNumeric
                                        />
                                        <BucketHeader
                                            label="Reasoning"
                                            isNumeric
                                        />
                                    </tr>
                                </thead>
                                <tbody>
                                    <For each={buckets()}>
                                        {(bucket) => (
                                            <tr class="border-t border-rule">
                                                <BucketKeyCell
                                                    bucket={bucket}
                                                    groupBy={groupBy()}
                                                    entities={entities()}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(bucket.requests)}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(bucket.input_tokens)}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(bucket.output_tokens)}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(bucket.cached_tokens)}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(
                                                        bucket.cache_write_tokens,
                                                    )}
                                                />
                                                <BucketCell
                                                    value={formatTokenCount(
                                                        bucket.reasoning_tokens,
                                                    )}
                                                />
                                            </tr>
                                        )}
                                    </For>
                                </tbody>
                            </table>
                        </div>
                        <p class="mt-3 text-[12.5px] text-ink-faint">
                            Token totals skip rows where the upstream reported no usage — an absent
                            breakdown never inflates or deflates a total.
                        </p>
                    </Card>
                </Show>
            </Show>
        </div>
    );
}

function BucketHeader(props: {
    readonly label: string;
    readonly isNumeric?: boolean;
}): JSX.Element {
    return (
        <th
            scope="col"
            class={`px-3 py-2 text-[12.5px] font-medium ${props.isNumeric === true ? "text-right" : ""}`}
        >
            {props.label}
        </th>
    );
}

function BucketCell(props: { readonly value: string }): JSX.Element {
    return <td class="px-3 py-2 text-right">{props.value}</td>;
}

/** A bucket's key in the words the operator knows: the day, or the resolved row. */
function BucketKeyCell(props: {
    readonly bucket: UsageBucket;
    readonly groupBy: UsageGroupBy;
    readonly entities: UsageEntities;
}): JSX.Element {
    const label = (): string => {
        if (props.groupBy === "day" || typeof props.bucket.key === "string") {
            return String(props.bucket.key);
        }

        switch (props.groupBy) {
            case "model":
                return usageModelLabel(props.entities, props.bucket.key).text;

            case "provider":
                return usageProviderLabel(props.entities, props.bucket.key).text;

            case "api_key":
                return usageApiKeyLabel(props.entities, props.bucket.key).text;
        }
    };

    return (
        <th
            scope="row"
            class="max-w-64 truncate px-3 py-2 font-medium"
            title={label()}
        >
            {label()}
        </th>
    );
}
