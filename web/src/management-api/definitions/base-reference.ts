import type { ControlRequest, ParamRow, StringExpr, TemplateNode } from "../generated/v0";
import type { DefinitionPayload } from "./types";

/**
 * Whether one string expression carries the `base` value part, directly or
 * through a `coalesce` — the one compound part, whose arguments the walk
 * descends into (`docs/design/providers.md`).
 */
function expressionReferencesBase(expression: StringExpr): boolean {
    return expression.some(
        (part) =>
            part.type === "base" ||
            (part.type === "coalesce" && expressionReferencesBase(part.parts)),
    );
}

/** Whether one template tree carries the `base` value part in any string leaf. */
function templateNodeReferencesBase(node: TemplateNode): boolean {
    switch (node.type) {
        case "json":
        case "body":
            return false;
        case "string":
            return expressionReferencesBase(node.expr);
        case "object":
            return Object.values(node.fields).some(templateNodeReferencesBase);
        case "array":
            return node.items.some(templateNodeReferencesBase);
    }
}

/** Whether one param-row list carries the `base` value part in any row value. */
function rowsReferenceBase(rows: ReadonlyArray<ParamRow>): boolean {
    return rows.some((row) => expressionReferencesBase(row.value));
}

/** Whether an optional-and-nullable URL expression carries the `base` value part. */
function optionalUrlReferencesBase(url: StringExpr | null | undefined): boolean {
    return url !== undefined && url !== null && expressionReferencesBase(url);
}

/** Whether an optional-and-nullable template tree carries the `base` value part. */
function optionalNodeReferencesBase(node: TemplateNode | null | undefined): boolean {
    return node !== undefined && node !== null && templateNodeReferencesBase(node);
}

/** Whether an optional-and-nullable param-row list carries the `base` value part. */
function optionalRowsReferenceBase(rows: ReadonlyArray<ParamRow> | null | undefined): boolean {
    return rows !== undefined && rows !== null && rowsReferenceBase(rows);
}

/** Whether one compaction operation's surface carries the `base` value part. */
function operationReferencesBase(operation: {
    readonly url: StringExpr;
    readonly headers?: Array<ParamRow>;
    readonly envelope?: TemplateNode | null;
}): boolean {
    return (
        expressionReferencesBase(operation.url) ||
        rowsReferenceBase(operation.headers ?? []) ||
        optionalNodeReferencesBase(operation.envelope)
    );
}

/**
 * Whether one control request's full expression surface — URL, header-row
 * values, query-row values, and the body's form-field values or JSON string
 * leaves — carries the `base` value part.
 */
function requestReferencesBase(request: ControlRequest): boolean {
    return (
        expressionReferencesBase(request.url) ||
        rowsReferenceBase(request.headers ?? []) ||
        rowsReferenceBase(request.query ?? []) ||
        (request.body?.kind === "form" && rowsReferenceBase(request.body.fields)) ||
        (request.body?.kind === "json" && templateNodeReferencesBase(request.body.node))
    );
}

/**
 * Whether any URL-bearing member of this payload references the instance
 * base URL — the `base` value part (`docs/design/storage.md`). The browser-side
 * mirror of the gateway's `DefinitionPayload::references_base`, so a page
 * asks for a base URL exactly when the gateway would reject the write or
 * flow start naming this definition without one. The walk covers exactly
 * the members whose evaluation environment carries the base URL: every
 * expression surface of the inference descriptor (the URLs, the header-row
 * values, and the envelope's string leaves), every control request's full
 * expression surface (URL, header-row values, query-row values, form-field
 * values, and JSON string leaves) across the credential section's device-
 * authorization, token-exchange, and refresh-grant requests, the
 * provisioning steps and their polls, the discovery request, and the
 * compaction section's standalone operation, token-count operation, and
 * Messages overlay headers — each directly or through a `coalesce`. The
 * authorize URL and its extra parameters are excluded: the write-time
 * validation bars the base reference at the authorize-URL stage (gateway
 * `docs/design/providers.md`), so those expressions can never evaluate against a base URL.
 */
export function hasBaseReference(payload: DefinitionPayload): boolean {
    if (expressionReferencesBase(payload.inference.url)) {
        return true;
    }

    if (optionalUrlReferencesBase(payload.inference.stream_url)) {
        return true;
    }

    if (rowsReferenceBase(payload.inference.headers ?? [])) {
        return true;
    }

    if (optionalNodeReferencesBase(payload.inference.envelope)) {
        return true;
    }

    if (payload.credential.type === "oauth") {
        const { grant, exchange, refresh } = payload.credential;

        if (grant.type === "device" && requestReferencesBase(grant.authorization.request)) {
            return true;
        }

        if (requestReferencesBase(exchange.request)) {
            return true;
        }

        if (refresh.type === "grant" && requestReferencesBase(refresh.request)) {
            return true;
        }
    }

    if (
        (payload.provisioning ?? []).some(
            (step) =>
                requestReferencesBase(step.request) ||
                (step.poll != null && requestReferencesBase(step.poll.request)),
        )
    ) {
        return true;
    }

    if (payload.discovery !== undefined && payload.discovery !== null) {
        if (requestReferencesBase(payload.discovery.request)) {
            return true;
        }
    }

    if (payload.quota != null && requestReferencesBase(payload.quota.request)) return true;

    if (payload.compaction !== undefined && payload.compaction !== null) {
        const { mechanisms, count_tokens: countTokens } = payload.compaction;

        const standaloneOperation = mechanisms.responses_standalone_compact?.operation;

        if (
            standaloneOperation !== undefined &&
            standaloneOperation !== null &&
            operationReferencesBase(standaloneOperation)
        ) {
            return true;
        }

        if (
            optionalRowsReferenceBase(mechanisms.messages_on_demand_compaction?.overlay_headers) ||
            optionalRowsReferenceBase(mechanisms.messages_threshold_compaction?.overlay_headers)
        ) {
            return true;
        }

        if (
            countTokens !== undefined &&
            countTokens !== null &&
            operationReferencesBase(countTokens)
        ) {
            return true;
        }
    }

    return false;
}
