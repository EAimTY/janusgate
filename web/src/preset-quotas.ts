import { translation } from "/src/i18n";
import type {
    ControlRequest,
    QuotaDescriptor,
    QuotaEntryExtraction,
    QuotaField,
    QuotaNumberEncoding,
    QuotaNumberField,
    QuotaWindowSource,
} from "/src/management-api/generated/v0";

/** Explicit scalar context; requiredness belongs to the mapping, never a fallback. */
function field(path: string[], isRequired = false, base: QuotaField["base"] = "entry"): QuotaField {
    return {
        base,
        path,
        is_required: isRequired,
    };
}

/** The numeric union is chosen from reviewed wire-shape evidence. */
function numberField(
    path: string[],
    encoding: QuotaNumberEncoding = "number",
    isRequired = false,
): QuotaNumberField {
    return {
        field: field(path, isRequired),
        encoding,
    };
}

/** One optional reported ratio window; an absent entry supplies no observation. */
function ratioEntry(
    key: string,
    label: string,
    at: string[],
    valuePath: string[],
    window: QuotaWindowSource,
    resetPath: string[],
    resetEncoding: "unix_seconds" | "rfc3339",
    encoding: QuotaNumberEncoding = "number",
): QuotaEntryExtraction {
    return {
        key,
        label,
        at,
        is_required: false,
        routing: {
            type: "monitoring_only",
        },
        window,
        measurement: {
            type: "ratio",
            value: numberField(valuePath, encoding),
            scale: "percent",
            direction: "used",
        },
        reset: {
            type: "field",
            field: field(resetPath),
            encoding: resetEncoding,
        },
    };
}

/** Standard current-token bearer request, with headers entirely in preset data. */
function bearer(url: string): ControlRequest {
    return {
        method: "GET",
        url: [
            {
                type: "literal",
                value: url,
            },
        ],
        headers: [
            {
                name: "Authorization",
                value: [
                    {
                        type: "literal",
                        value: "Bearer ",
                    },
                    {
                        type: "secret",
                    },
                ],
            },
        ],
    };
}

/** Codex uses upstream durations; window names do not imply five hours or seven days. */
function codexWindows(at: string[] = []): QuotaEntryExtraction[] {
    return [
        "primary",
        "secondary",
    ].map((key) =>
        ratioEntry(
            key,
            key === "primary"
                ? translation()(($) => $.helpers.presetQuotas.primaryWindow)
                : translation()(($) => $.helpers.presetQuotas.secondaryWindow),
            [
                ...at,
                `${key}_window`,
            ],
            [
                "used_percent",
            ],
            {
                type: "duration_field",
                value: numberField([
                    "limit_window_seconds",
                ]),
                unit: "seconds",
            },
            [
                "reset_at",
            ],
            "unix_seconds",
        ),
    );
}

/** Researched quota capabilities; each remains unverified by a live quota query. */
export const PRESET_QUOTAS: Readonly<
    Record<
        "codex" | "claude-code" | "kimi-code" | "github-copilot" | "antigravity",
        QuotaDescriptor
    >
> = {
    codex: {
        request: {
            ...bearer("https://chatgpt.com/backend-api/wham/usage"),
            headers: [
                ...(bearer("").headers ?? []),
                {
                    name: "ChatGPT-Account-Id",
                    value: [
                        {
                            type: "slot",
                            name: "account_id",
                        },
                    ],
                },
            ],
        },
        sources: [
            {
                type: "object",
                key: "account",
                at: [
                    "rate_limit",
                ],
                is_required: false,
                scope: {
                    type: "literal",
                    value: "Account",
                },
                entries: codexWindows(),
            },
            {
                type: "array",
                key: "additional",
                at: [
                    "additional_rate_limits",
                ],
                is_required: false,
                identity: [
                    {
                        type: "field",
                        field: field(
                            [
                                "metered_feature",
                            ],
                            true,
                            "item",
                        ),
                        encoding: "string",
                    },
                ],
                scope: {
                    type: "field",
                    field: field(
                        [
                            "limit_name",
                        ],
                        false,
                        "item",
                    ),
                    encoding: "string",
                },
                entries: codexWindows([
                    "rate_limit",
                ]),
            },
        ],
    },
    "claude-code": {
        request: {
            ...bearer("https://api.anthropic.com/api/oauth/usage"),
            headers: [
                ...(bearer("").headers ?? []),
                {
                    name: "anthropic-beta",
                    value: [
                        {
                            type: "literal",
                            value: "oauth-2025-04-20",
                        },
                    ],
                },
                {
                    name: "User-Agent",
                    value: [
                        {
                            type: "literal",
                            value: "claude-cli/2.1.0 (external, cli)",
                        },
                    ],
                },
            ],
        },
        sources: [
            {
                type: "object",
                key: "account",
                at: [],
                is_required: true,
                scope: {
                    type: "literal",
                    value: "Account",
                },
                entries: [
                    ratioEntry(
                        "five_hour",
                        "Five-hour allowance",
                        [
                            "five_hour",
                        ],
                        [
                            "utilization",
                        ],
                        {
                            type: "duration",
                            seconds: 18000,
                        },
                        [
                            "resets_at",
                        ],
                        "rfc3339",
                    ),
                    ratioEntry(
                        "seven_day",
                        "Weekly allowance",
                        [
                            "seven_day",
                        ],
                        [
                            "utilization",
                        ],
                        {
                            type: "duration",
                            seconds: 604800,
                        },
                        [
                            "resets_at",
                        ],
                        "rfc3339",
                    ),
                    ratioEntry(
                        "seven_day_sonnet",
                        "Sonnet weekly allowance",
                        [
                            "seven_day_sonnet",
                        ],
                        [
                            "utilization",
                        ],
                        {
                            type: "duration",
                            seconds: 604800,
                        },
                        [
                            "resets_at",
                        ],
                        "rfc3339",
                    ),
                ],
            },
        ],
    },
    "kimi-code": {
        request: {
            ...bearer("https://api.kimi.com/coding/v1/usages"),
            headers: [
                ...(bearer("").headers ?? []),
                {
                    name: "Accept",
                    value: [
                        {
                            type: "literal",
                            value: "application/json",
                        },
                    ],
                },
            ],
        },
        sources: [
            {
                type: "object",
                key: "account",
                at: [
                    "usages",
                ],
                is_required: true,
                scope: {
                    type: "literal",
                    value: "Account",
                },
                entries: [
                    {
                        ...ratioEntry(
                            "limit_5h",
                            "Five-hour allowance",
                            [
                                "limit_5h",
                            ],
                            [
                                "used_ratio",
                            ],
                            {
                                type: "duration",
                                seconds: 18000,
                            },
                            [
                                "reset_time",
                            ],
                            "rfc3339",
                            "number_or_numeric_string",
                        ),
                        measurement: {
                            type: "ratio",
                            value: numberField(
                                [
                                    "used_ratio",
                                ],
                                "number_or_numeric_string",
                            ),
                            scale: "fraction",
                            direction: "used",
                        },
                    },
                    {
                        ...ratioEntry(
                            "limit_7d",
                            "Weekly allowance",
                            [
                                "limit_7d",
                            ],
                            [
                                "used_ratio",
                            ],
                            {
                                type: "duration",
                                seconds: 604800,
                            },
                            [
                                "reset_time",
                            ],
                            "rfc3339",
                            "number_or_numeric_string",
                        ),
                        measurement: {
                            type: "ratio",
                            value: numberField(
                                [
                                    "used_ratio",
                                ],
                                "number_or_numeric_string",
                            ),
                            scale: "fraction",
                            direction: "used",
                        },
                    },
                    {
                        ...ratioEntry(
                            "limit_month_total",
                            "Monthly total",
                            [
                                "limit_month_total",
                            ],
                            [
                                "used_ratio",
                            ],
                            {
                                type: "calendar",
                                unit: "month",
                            },
                            [
                                "reset_time",
                            ],
                            "rfc3339",
                            "number_or_numeric_string",
                        ),
                        measurement: {
                            type: "ratio",
                            value: numberField(
                                [
                                    "used_ratio",
                                ],
                                "number_or_numeric_string",
                            ),
                            scale: "fraction",
                            direction: "used",
                        },
                    },
                    {
                        ...ratioEntry(
                            "limit_month_code",
                            "Monthly coding",
                            [
                                "limit_month_code",
                            ],
                            [
                                "used_ratio",
                            ],
                            {
                                type: "calendar",
                                unit: "month",
                            },
                            [
                                "reset_time",
                            ],
                            "rfc3339",
                            "number_or_numeric_string",
                        ),
                        measurement: {
                            type: "ratio",
                            value: numberField(
                                [
                                    "used_ratio",
                                ],
                                "number_or_numeric_string",
                            ),
                            scale: "fraction",
                            direction: "used",
                        },
                    },
                ],
            },
        ],
    },
    "github-copilot": {
        request: {
            method: "GET",
            url: [
                {
                    type: "literal",
                    value: "https://api.github.com/copilot_internal/user",
                },
            ],
            headers: [
                {
                    name: "Authorization",
                    value: [
                        {
                            type: "literal",
                            value: "token ",
                        },
                        {
                            type: "slot",
                            name: "github_token",
                        },
                    ],
                },
                {
                    name: "X-GitHub-Api-Version",
                    value: [
                        {
                            type: "literal",
                            value: "2025-04-01",
                        },
                    ],
                },
            ],
        },
        sources: [
            {
                type: "map",
                key: "allowances",
                at: [
                    "quota_snapshots",
                ],
                is_required: false,
                scope: {
                    type: "map_key",
                },
                entries: [
                    {
                        key: "allowance",
                        routing: {
                            type: "monitoring_only",
                        },
                        get label() {
                            return translation()(($) => $.helpers.presetQuotas.allowance);
                        },
                        at: [],
                        is_required: true,
                        window: {
                            type: "unknown",
                        },
                        is_unlimited: field(
                            [
                                "unlimited",
                            ],
                            true,
                        ),
                        measurement: {
                            type: "ratio",
                            value: numberField(
                                [
                                    "percent_remaining",
                                ],
                                "number",
                                true,
                            ),
                            scale: "percent",
                            direction: "remaining",
                        },
                        reset: {
                            type: "field",
                            field: field(
                                [
                                    "quota_reset_date",
                                ],
                                false,
                                "root",
                            ),
                            encoding: "date",
                        },
                    },
                ],
            },
        ],
    },
    antigravity: {
        request: {
            method: "POST",
            url: [
                {
                    type: "literal",
                    value: "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
                },
            ],
            headers: [
                ...(bearer("").headers ?? []),
                {
                    name: "User-Agent",
                    value: [
                        {
                            type: "literal",
                            value: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Antigravity/1.18.3 Chrome/138.0.7204.235 Electron/37.3.1 Safari/537.36",
                        },
                    ],
                },
            ],
            body: {
                kind: "json",
                node: {
                    type: "object",
                    fields: {
                        project: {
                            type: "string",
                            expr: [
                                {
                                    type: "slot",
                                    name: "project",
                                },
                            ],
                        },
                    },
                },
            },
        },
        sources: [
            {
                type: "map",
                key: "models",
                at: [
                    "models",
                ],
                is_required: true,
                scope: {
                    type: "map_key",
                },
                entries: [
                    {
                        key: "usage",
                        routing: {
                            type: "monitoring_only",
                        },
                        get label() {
                            return translation()(($) => $.helpers.presetQuotas.modelAllowance);
                        },
                        at: [
                            "quotaInfo",
                        ],
                        is_required: false,
                        window: {
                            type: "unknown",
                        },
                        measurement: {
                            type: "ratio",
                            value: numberField([
                                "remainingFraction",
                            ]),
                            scale: "fraction",
                            direction: "remaining",
                        },
                        reset: {
                            type: "field",
                            field: field([
                                "resetTime",
                            ]),
                            encoding: "rfc3339",
                        },
                    },
                ],
            },
        ],
    },
};
