import type { JSX } from "@solidjs/web";
import { createMemo } from "solid-js";
import type {
    ApiKeyResponse,
    ModelResponse,
    ProviderResponse,
} from "/src/management-api/generated/v0";
import { API_LOG_ENDPOINT_TYPE, API_LOG_OUTCOME } from "/src/management-api/vocabulary";
import { entitySelectOptions, noLongerListedLabel, SelectField, TextField } from "/src/ui/controls";
import type { UsageFilterFields, UsageOutcomeChoice, UsageStreamChoice } from "/src/usage-filter";

/**
 * The outcome control's choices: everything, the two failure outcomes under
 * the one name the app calls them by, then each outcome on its own. The
 * grouped choice exists so a figure stated as "errors" can be opened as a
 * filter without the control having to show a list it cannot present.
 */
const OUTCOME_CHOICE_OPTIONS: ReadonlyArray<{
    readonly value: UsageOutcomeChoice;
    readonly label: string;
}> = [
    {
        value: "",
        label: "All outcomes",
    },
    {
        value: "failures",
        label: "Failures (upstream or internal)",
    },
    ...API_LOG_OUTCOME.options,
];

const STREAM_CHOICE_OPTIONS: ReadonlyArray<{
    readonly value: UsageStreamChoice;
    readonly label: string;
}> = [
    {
        value: "",
        label: "Any",
    },
    {
        value: "streamed",
        label: "Streamed",
    },
    {
        value: "non-streamed",
        label: "Non-streamed",
    },
];

/**
 * The shared usage filter's controls, as a fragment the caller lays out in its
 * own grid: both usage screens narrow the same filter type, so both present
 * these fields — the same labels, the same options, the same vocabulary —
 * while each screen owns the bounds it resolves (a rolling window on the
 * listing, explicit instants on the aggregates page). Every option list tracks
 * only the entity listings the screen read; an id no option carries is stated
 * by the select's own trailing option, worded by `noLongerListedLabel` once
 * the listings stand read (`hasEntityListings`) — the point from which the
 * absence is a fact a read listing states, not an unread one it merely fails
 * to contradict.
 *
 * The control reports the one field the operator changed and the screen merges
 * it into the filter it holds, so two edits delivered in one task — a change
 * event following an input event — never overwrite each other with a field set
 * read before the first landed.
 */
export function UsageFilterControls(props: {
    readonly fields: UsageFilterFields;
    readonly providers: ReadonlyArray<ProviderResponse>;
    readonly models: ReadonlyArray<ModelResponse>;
    readonly apiKeys: ReadonlyArray<ApiKeyResponse>;
    /** Whether the screen has read the entity listings at least once — the point from which an unlisted id is a stated absence. */
    readonly hasEntityListings: boolean;
    /** The inline error for the hand-typed status list, computed and gated on by the screen. */
    readonly statusError: string | null | undefined;
    readonly isDisabled: boolean;
    readonly onEdit: (edits: Partial<UsageFilterFields>) => void;
}): JSX.Element {
    const providerOptions = createMemo(() =>
        entitySelectOptions(props.providers, (row) => row.name, "All providers"),
    );

    const modelOptions = createMemo(() =>
        entitySelectOptions(props.models, (row) => row.public_id, "All models"),
    );

    const apiKeyOptions = createMemo(() =>
        entitySelectOptions(props.apiKeys, (row) => row.name, "All API keys"),
    );

    return (
        <>
            <SelectField
                label="Provider"
                value={props.fields.providerId}
                options={providerOptions()}
                absentLabel={props.hasEntityListings ? noLongerListedLabel : undefined}
                onChange={(providerId) =>
                    props.onEdit({
                        providerId,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <SelectField
                label="Model"
                value={props.fields.modelId}
                options={modelOptions()}
                absentLabel={props.hasEntityListings ? noLongerListedLabel : undefined}
                onChange={(modelId) =>
                    props.onEdit({
                        modelId,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <SelectField
                label="API key"
                value={props.fields.apiKeyId}
                options={apiKeyOptions()}
                absentLabel={props.hasEntityListings ? noLongerListedLabel : undefined}
                onChange={(apiKeyId) =>
                    props.onEdit({
                        apiKeyId,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <SelectField
                label="Endpoint"
                value={props.fields.endpointType}
                options={[
                    {
                        value: "",
                        label: "All endpoints",
                    },
                    ...API_LOG_ENDPOINT_TYPE.options,
                ]}
                onChange={(endpointType) =>
                    props.onEdit({
                        endpointType,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <SelectField
                label="Outcome"
                value={props.fields.outcome}
                options={OUTCOME_CHOICE_OPTIONS}
                onChange={(outcome) =>
                    props.onEdit({
                        outcome,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <SelectField
                label="Stream mode"
                value={props.fields.streamChoice}
                options={STREAM_CHOICE_OPTIONS}
                onChange={(streamChoice) =>
                    props.onEdit({
                        streamChoice,
                    })
                }
                isDisabled={props.isDisabled}
            />
            <TextField
                label="HTTP statuses"
                value={props.fields.statusInput}
                onInput={(statusInput) =>
                    props.onEdit({
                        statusInput,
                    })
                }
                isDisabled={props.isDisabled}
                error={props.statusError}
                hint="Comma-separated, e.g. 200, 429. Never-answered requests match no status."
            />
        </>
    );
}
