import { ContractViolationError } from "/src/management-api/errors";
import type {
    CompactionSection,
    CompactionStrategy,
    ControlBody,
    ControlRequest,
    CountTokensOperation,
    CreateProviderDefinitionRequest,
    CredentialAcquisition,
    DefinitionEnvelope,
    DefinitionResponse,
    Discovery,
    DiscoveryExtract,
    Exchange,
    ExtractionShape,
    Grant,
    Inference,
    JsonValue,
    ListProviderDefinitionsResponse,
    MechanismDeclarations,
    MechanismSupport,
    Pagination,
    ParamRow,
    Poll,
    ProvisionExtraction,
    ProvisioningStep,
    RenderOption,
    ResponseDescriptor,
    ResponseEncoding,
    ResponsePath,
    ResponseTarget,
    StandaloneCompactDeclaration,
    StandaloneCompactOperation,
    StringExpr,
    TemplateNode,
    TokenRefresh,
    TriggerDeclaration,
    UnsupportedClassifier,
    UpstreamFormat,
    UpstreamStreaming,
    ValueExpr,
} from "/src/management-api/generated/v0";
import { parseQuotaDescriptor, validateQuotaRequest } from "/src/management-api/quota";

import {
    expectArray,
    expectBoolean,
    expectInteger,
    expectRecord,
    expectString,
    parseEnvelopeResponse,
    parseListResponse,
    parseUpstreamFormat,
} from "/src/management-api/validation";

/**
 * The authored definition payload's validator (README §5): the browser-side
 * mirror of the gateway's descriptor contract. A definition's descriptor is
 * authored JSON — a preset's emission or an operator's edit — and one
 * structural walk over it serves both directions: the creation page validates
 * the edited text before it is sent, and the stored-definition responses
 * build their values through the same walk.
 */

/**
 * The authored definition payload (README §5): a definition's descriptor as
 * `provider-definitions:create` and `provider-definitions:update` carry it,
 * everything beside the `name`.
 */
export type DefinitionPayload = Omit<CreateProviderDefinitionRequest, "name">;

/**
 * What validating an authored payload produced: the typed payload, or the
 * offending JSON path — the message the creation page renders inline.
 */
export type DefinitionPayloadResult =
    | {
          readonly payload: DefinitionPayload;
      }
    | {
          readonly error: string;
      };

/**
 * Whether one string expression carries the `base` value part, directly or
 * through a `coalesce` — the one compound part, whose arguments the walk
 * descends into (README §5.1).
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
 * base URL — the `base` value part (gateway README §3). The browser-side
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
 * README §5.1), so those expressions can never evaluate against a base URL.
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
                optionalUrlReferencesBase(step.poll?.request.url) ||
                rowsReferenceBase(step.poll?.request.headers ?? []) ||
                rowsReferenceBase(step.poll?.request.query ?? []),
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

/**
 * Validates one authored definition payload (README §5): the JSON a preset
 * emitted or an operator edited. The gateway parses descriptors with
 * `deny_unknown_fields` and answers a broken payload with errno 1, so this
 * walk applies the same strictness before the round trip — every closed
 * vocabulary is checked against the generated types, every required key's
 * presence, and every undeclared key is named instead of silently dropped.
 */
export function validateDefinitionPayload(value: unknown): DefinitionPayloadResult {
    try {
        return {
            payload: expectDefinitionPayload(value, "definition"),
        };
    } catch (error) {
        if (error instanceof ContractViolationError) {
            return {
                error: error.message,
            };
        }

        throw error;
    }
}

/**
 * Rejects record keys outside the set the walk consumed: a descriptor field
 * the bindings do not declare would be dropped by the rebuild below, and a
 * dropped field is a mistake to name inline — never one to send a truncated
 * payload about (the response parsers in `validation.ts` tolerate extras
 * instead, because those bodies are never rebuilt).
 */
function expectDeclaredKeys(
    record: object,
    declared: ReadonlyArray<string>,
    context: string,
): void {
    for (const key of Object.keys(record)) {
        if (!declared.includes(key)) {
            throw new ContractViolationError(`${context}.${key} is not a declared field`);
        }
    }
}

/** Whether an optional-and-nullable descriptor value carries content. */
function isPresentValue(value: unknown): boolean {
    return value !== undefined && value !== null;
}

/**
 * Narrows one value to a closed string vocabulary: the member itself when
 * the value is one of `members`, else a contract violation naming the path,
 * the vocabulary's noun, and the offending value.
 */
function expectOneOf<TValue extends string>(
    value: unknown,
    members: ReadonlyArray<TValue>,
    context: string,
    noun: string,
): TValue {
    const member = members.find((candidate) => candidate === value);

    if (member !== undefined) {
        return member;
    }

    throw new ContractViolationError(`${context} is not ${noun}: ${String(value)}`);
}

/**
 * One authored definition payload, rebuilt field by field. The context is the
 * JSON path the messages name, `"definition"` at the top of an authored
 * payload and the response path inside a stored-definition parse.
 */
function expectDefinitionPayload(value: unknown, context: string): DefinitionPayload {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "format",
            "inference",
            "credential",
            "provisioning",
            "discovery",
            "compaction",
            "quota",
        ],
        context,
    );

    const payload: DefinitionPayload = {
        format: parseUpstreamFormat(Reflect.get(record, "format"), `${context}.format`),
        inference: expectInference(Reflect.get(record, "inference"), `${context}.inference`),
        credential: expectCredential(Reflect.get(record, "credential"), `${context}.credential`),
    };

    const provisioning = Reflect.get(record, "provisioning");

    if (isPresentValue(provisioning)) {
        payload.provisioning = expectProvisioning(provisioning, `${context}.provisioning`);
    }

    const discovery = Reflect.get(record, "discovery");

    if (isPresentValue(discovery)) {
        payload.discovery = expectDiscovery(discovery, `${context}.discovery`);
    }

    const compaction = Reflect.get(record, "compaction");

    if (isPresentValue(compaction)) {
        payload.compaction = expectCompactionSection(
            compaction,
            `${context}.compaction`,
            payload.format,
        );
    }

    const quota = Reflect.get(record, "quota");
    if (isPresentValue(quota)) {
        payload.quota = parseQuotaDescriptor(quota, `${context}.quota`, expectControlRequest);
        validateQuotaRequest(
            payload.quota,
            payload.credential,
            payload.provisioning,
            `${context}.quota.request`,
        );
    }
    return payload;
}

/** The upstream streaming mode a definition pins (README §5.2). */
function expectUpstreamStreaming(value: unknown, context: string): UpstreamStreaming {
    return expectOneOf(
        value,
        [
            "always",
            "match_client",
        ],
        context,
        "an upstream streaming mode",
    );
}

/** The closed render-option vocabulary of the format codecs (README §5.2). */
function expectRenderOption(value: unknown, context: string): RenderOption {
    return expectOneOf(
        value,
        [
            "omit_max_output_tokens",
        ],
        context,
        "a render option",
    );
}

/** The wire encoding a control response arrives in (README §5.1). */
function expectResponseEncoding(value: unknown, context: string): ResponseEncoding {
    return expectOneOf(
        value,
        [
            "json",
            "form",
        ],
        context,
        "a response encoding",
    );
}

/**
 * The inference request descriptor (README §5.2), rebuilt with every optional
 * section the operator supplied and `null` normalized to absence.
 */
function expectInference(value: unknown, context: string): Inference {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "url",
            "stream_url",
            "headers",
            "upstream_streaming",
            "render_options",
            "envelope",
            "response_unwrap",
        ],
        context,
    );

    const inference: Inference = {
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
        upstream_streaming: expectUpstreamStreaming(
            Reflect.get(record, "upstream_streaming"),
            `${context}.upstream_streaming`,
        ),
    };

    const streamUrl = Reflect.get(record, "stream_url");

    if (isPresentValue(streamUrl)) {
        inference.stream_url = expectStringExpr(streamUrl, `${context}.stream_url`);
    }

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        inference.headers = expectParamRows(headers, `${context}.headers`);
    }

    const renderOptions = Reflect.get(record, "render_options");

    if (renderOptions !== undefined) {
        inference.render_options = expectArray(renderOptions, `${context}.render_options`).map(
            (option, index) => expectRenderOption(option, `${context}.render_options[${index}]`),
        );
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        inference.envelope = expectTemplateNode(envelope, `${context}.envelope`);
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        inference.response_unwrap = expectString(responseUnwrap, `${context}.response_unwrap`);
    }

    return inference;
}

/** One value expression part, rebuilt from its validated kind (README §5.1). */
function expectExpressionPart(part: unknown, context: string): ValueExpr {
    const record = expectRecord(part, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "literal":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "literal",
                value: expectString(Reflect.get(record, "value"), `${context}.value`),
            };
        case "secret":
        case "client_id":
        case "client_secret":
        case "authorization_code":
        case "code_verifier":
        case "redirect_uri":
        case "oauth_state":
        case "device_code":
        case "refresh_token":
        case "request_uuid":
        case "model":
        case "base":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: kind,
            };
        case "slot":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "name",
                ],
                context,
            );
            return {
                type: "slot",
                name: expectString(Reflect.get(record, "name"), `${context}.name`),
            };
        case "coalesce":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "parts",
                ],
                context,
            );
            return {
                type: "coalesce",
                parts: expectStringExpr(Reflect.get(record, "parts"), `${context}.parts`),
            };
        default:
            throw new ContractViolationError(`${context}.type is not an expression kind: ${kind}`);
    }
}

/** Validates one string expression: an ordered array of expression parts. */
function expectStringExpr(value: unknown, context: string): StringExpr {
    return expectArray(value, context).map((part, index) =>
        expectExpressionPart(part, `${context}[${index}]`),
    );
}

/** One param row: a name and its expression, rebuilt (README §5.1). */
function expectParamRows(value: unknown, context: string): Array<ParamRow> {
    return expectArray(value, context).map((row, index) => {
        const record = expectRecord(row, `${context}[${index}]`);
        expectDeclaredKeys(
            record,
            [
                "name",
                "value",
            ],
            `${context}[${index}]`,
        );

        return {
            name: expectString(Reflect.get(record, "name"), `${context}[${index}].name`),
            value: expectStringExpr(Reflect.get(record, "value"), `${context}[${index}].value`),
        };
    });
}

/** One JSON value, walked so a non-JSON leaf (a function, a bare `undefined`) is named. */
function expectJsonValue(value: unknown, context: string): JsonValue {
    if (value === null) {
        return null;
    }

    switch (typeof value) {
        case "string":
            return value;
        case "number":
            if (Number.isFinite(value)) {
                return value;
            }

            throw new ContractViolationError(`${context} is not a JSON value`);
        case "boolean":
            return value;
        case "object":
            if (Array.isArray(value)) {
                return value.map((entry, index) => expectJsonValue(entry, `${context}[${index}]`));
            }

            return Object.fromEntries(
                Object.keys(value).map((key) => [
                    key,
                    expectJsonValue(Reflect.get(value, key), `${context}.${key}`),
                ]),
            );
        default:
            throw new ContractViolationError(`${context} is not a JSON value`);
    }
}

/** One template tree node, rebuilt from its node kind (README §5.1). */
function expectTemplateNode(value: unknown, context: string): TemplateNode {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    switch (kind) {
        case "json":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "value",
                ],
                context,
            );
            return {
                type: "json",
                value: expectJsonValue(Reflect.get(record, "value"), `${context}.value`),
            };

        case "string":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "expr",
                ],
                context,
            );
            return {
                type: "string",
                expr: expectStringExpr(Reflect.get(record, "expr"), `${context}.expr`),
            };

        case "body":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type: "body",
            };

        case "object": {
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "fields",
                ],
                context,
            );
            const fields = expectRecord(Reflect.get(record, "fields"), `${context}.fields`);

            return {
                type: "object",
                fields: Object.fromEntries(
                    Object.keys(fields).map((name) => [
                        name,
                        expectTemplateNode(Reflect.get(fields, name), `${context}.fields.${name}`),
                    ]),
                ),
            };
        }

        case "array":
            expectDeclaredKeys(
                record,
                [
                    "type",
                    "items",
                ],
                context,
            );
            return {
                type: "array",
                items: expectArray(Reflect.get(record, "items"), `${context}.items`).map(
                    (item, index) => expectTemplateNode(item, `${context}.items[${index}]`),
                ),
            };

        default:
            throw new ContractViolationError(`${context}.type is not a template node: ${kind}`);
    }
}

/** One control body: a form-encoded field set or a typed JSON tree (README §5.1). */
function expectControlBody(value: unknown, context: string): ControlBody {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "kind"), `${context}.kind`);

    if (kind === "form") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "fields",
            ],
            context,
        );

        return {
            kind: "form",
            fields: expectParamRows(Reflect.get(record, "fields"), `${context}.fields`),
        };
    }

    if (kind === "json") {
        expectDeclaredKeys(
            record,
            [
                "kind",
                "node",
            ],
            context,
        );

        return {
            kind: "json",
            node: expectTemplateNode(Reflect.get(record, "node"), `${context}.node`),
        };
    }

    throw new ContractViolationError(`${context}.kind is not a control body: ${kind}`);
}

/** One control request: method, URL, headers, query, and body (README §5.1). */
function expectControlRequest(value: unknown, context: string): ControlRequest {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "method",
            "url",
            "headers",
            "query",
            "body",
        ],
        context,
    );

    const method = expectOneOf(
        expectString(Reflect.get(record, "method"), `${context}.method`),
        [
            "GET",
            "POST",
        ],
        `${context}.method`,
        "a control method",
    );

    const request: ControlRequest = {
        method,
        url: expectStringExpr(Reflect.get(record, "url"), `${context}.url`),
    };

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        request.headers = expectParamRows(headers, `${context}.headers`);
    }

    const query = Reflect.get(record, "query");

    if (query !== undefined) {
        request.query = expectParamRows(query, `${context}.query`);
    }

    const body = Reflect.get(record, "body");

    if (isPresentValue(body)) {
        request.body = expectControlBody(body, `${context}.body`);
    }

    return request;
}

/** One response path: the ordered member chain (README §5.1). */
function expectResponsePath(value: unknown, context: string): ResponsePath {
    return expectArray(value, context).map((member, index) =>
        expectString(member, `${context}[${index}]`),
    );
}

/** One merged working-state target, rebuilt from its tag (README §5.3, §5.4). */
function expectResponseTarget(value: unknown, context: string): ResponseTarget {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "slot") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "name",
            ],
            context,
        );

        return {
            type: "slot",
            name: expectString(Reflect.get(record, "name"), `${context}.name`),
        };
    }

    switch (type) {
        case "access_token":
        case "refresh_token":
        case "expires_in_seconds":
        case "id_token":
        case "base_url":
        case "device_code":
        case "user_code":
        case "verification_uri":
        case "verification_uri_complete":
        case "interval_seconds":
        case "expires_at_unix_seconds":
            expectDeclaredKeys(
                record,
                [
                    "type",
                ],
                context,
            );
            return {
                type,
            };
        default:
            throw new ContractViolationError(`${context}.type is not a response target: ${type}`);
    }
}

/** One response mapping field: the path, its target, and the required flag. */
function expectResponseField(
    value: unknown,
    context: string,
): {
    path: ResponsePath;
    target: ResponseTarget;
    required: boolean;
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** One response descriptor: the encoding and its field mappings (README §5.1). */
function expectResponseDescriptor(value: unknown, context: string): ResponseDescriptor {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "encoding",
            "fields",
        ],
        context,
    );

    return {
        encoding: expectResponseEncoding(Reflect.get(record, "encoding"), `${context}.encoding`),
        fields: expectArray(Reflect.get(record, "fields"), `${context}.fields`).map(
            (field, index) => expectResponseField(field, `${context}.fields[${index}]`),
        ),
    };
}

/** The declared shape of one extracted value (README §5.4). */
function expectExtractionShape(value: unknown, context: string): ExtractionShape {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "string" || type === "integer") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }

    if (type === "string_or_field") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "field",
            ],
            context,
        );

        return {
            type: "string_or_field",
            field: expectString(Reflect.get(record, "field"), `${context}.field`),
        };
    }

    throw new ContractViolationError(`${context}.type is not an extraction shape: ${type}`);
}

/** One provisioning extraction: the path, its target, the shape, and the flag. */
function expectProvisionExtraction(value: unknown, context: string): ProvisionExtraction {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "path",
            "target",
            "shape",
            "required",
        ],
        context,
    );

    return {
        path: expectResponsePath(Reflect.get(record, "path"), `${context}.path`),
        target: expectResponseTarget(Reflect.get(record, "target"), `${context}.target`),
        shape: expectExtractionShape(Reflect.get(record, "shape"), `${context}.shape`),
        required: expectBoolean(Reflect.get(record, "required"), `${context}.required`),
    };
}

/** The extractions of one provisioning step or poll, when the key is present. */
function expectProvisionExtractions(value: unknown, context: string): Array<ProvisionExtraction> {
    return expectArray(value, context).map((entry, index) =>
        expectProvisionExtraction(entry, `${context}[${index}]`),
    );
}

/** The terminal status codes of an exchange or a grant-mode token refresh. */
function expectTerminalCodes(value: unknown, context: string): Array<string> {
    return expectArray(value, context).map((code, index) =>
        expectString(code, `${context}[${index}]`),
    );
}

/** The long-running operation poll of a provisioning step (README §5.4). */
function expectPoll(value: unknown, context: string): Poll {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "done_path",
            "error_path",
            "result_extract",
            "interval_ms",
            "budget_ms",
        ],
        context,
    );

    const poll: Poll = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
        done_path: expectResponsePath(Reflect.get(record, "done_path"), `${context}.done_path`),
        error_path: expectResponsePath(Reflect.get(record, "error_path"), `${context}.error_path`),
        interval_ms: expectInteger(Reflect.get(record, "interval_ms"), `${context}.interval_ms`),
        budget_ms: expectInteger(Reflect.get(record, "budget_ms"), `${context}.budget_ms`),
    };

    const resultExtract = Reflect.get(record, "result_extract");

    if (resultExtract !== undefined) {
        poll.result_extract = expectProvisionExtractions(
            resultExtract,
            `${context}.result_extract`,
        );
    }

    return poll;
}

/** One provisioning step, rebuilt with its guard, extractions, and poll (README §5.4). */
function expectProvisioningStep(value: unknown, context: string): ProvisioningStep {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response_encoding",
            "extract",
            "skip_if",
            "poll",
        ],
        context,
    );

    const step: ProvisioningStep = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response_encoding: expectResponseEncoding(
            Reflect.get(record, "response_encoding"),
            `${context}.response_encoding`,
        ),
    };

    const extract = Reflect.get(record, "extract");

    if (extract !== undefined) {
        step.extract = expectProvisionExtractions(extract, `${context}.extract`);
    }

    const skipIf = Reflect.get(record, "skip_if");

    if (isPresentValue(skipIf)) {
        step.skip_if = expectString(skipIf, `${context}.skip_if`);
    }

    const poll = Reflect.get(record, "poll");

    if (isPresentValue(poll)) {
        step.poll = expectPoll(poll, `${context}.poll`);
    }

    return step;
}

/** Validates one provisioning sequence with its polls (README §5.4). */
function expectProvisioning(value: unknown, context: string): Array<ProvisioningStep> {
    return expectArray(value, context).map((step, index) =>
        expectProvisioningStep(step, `${context}[${index}]`),
    );
}

/** The page-list extraction of a discovery, rebuilt from its mode (README §5.5). */
function expectDiscoveryExtract(value: unknown, context: string): DiscoveryExtract {
    const record = expectRecord(value, context);
    const mode = expectString(Reflect.get(record, "mode"), `${context}.mode`);

    if (mode === "array") {
        expectDeclaredKeys(
            record,
            [
                "mode",
                "array_at",
                "id_field",
                "strip_prefix",
            ],
            context,
        );

        const extract: DiscoveryExtract = {
            mode: "array",
            array_at: expectResponsePath(Reflect.get(record, "array_at"), `${context}.array_at`),
            id_field: expectResponsePath(Reflect.get(record, "id_field"), `${context}.id_field`),
        };

        const stripPrefix = Reflect.get(record, "strip_prefix");

        if (isPresentValue(stripPrefix)) {
            extract.strip_prefix = expectString(stripPrefix, `${context}.strip_prefix`);
        }

        return extract;
    }

    if (mode === "map_keys") {
        expectDeclaredKeys(
            record,
            [
                "mode",
                "at",
            ],
            context,
        );

        return {
            mode: "map_keys",
            at: expectResponsePath(Reflect.get(record, "at"), `${context}.at`),
        };
    }

    throw new ContractViolationError(`${context}.mode is not a discovery extraction: ${mode}`);
}

/** The pagination loop of a discovery, rebuilt from its mode (README §5.5). */
function expectPagination(value: unknown, context: string): Pagination {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "none") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type: "none",
        };
    }

    if (type === "token") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "request_param",
                "response_path",
                "max_pages",
            ],
            context,
        );

        return {
            type: "token",
            request_param: expectString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            response_path: expectResponsePath(
                Reflect.get(record, "response_path"),
                `${context}.response_path`,
            ),
            max_pages: expectInteger(Reflect.get(record, "max_pages"), `${context}.max_pages`),
        };
    }

    if (type === "cursor") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "has_more_path",
                "last_id_path",
                "request_param",
                "max_pages",
            ],
            context,
        );

        return {
            type: "cursor",
            has_more_path: expectResponsePath(
                Reflect.get(record, "has_more_path"),
                `${context}.has_more_path`,
            ),
            last_id_path: expectResponsePath(
                Reflect.get(record, "last_id_path"),
                `${context}.last_id_path`,
            ),
            request_param: expectString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            max_pages: expectInteger(Reflect.get(record, "max_pages"), `${context}.max_pages`),
        };
    }

    throw new ContractViolationError(`${context}.type is not a pagination mode: ${type}`);
}

/** The upstream model-list discovery of a definition (README §5.5). */
function expectDiscovery(value: unknown, context: string): Discovery {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "extract",
            "pagination",
        ],
        context,
    );

    const discovery: Discovery = {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        extract: expectDiscoveryExtract(Reflect.get(record, "extract"), `${context}.extract`),
    };

    const pagination = Reflect.get(record, "pagination");

    if (isPresentValue(pagination)) {
        discovery.pagination = expectPagination(pagination, `${context}.pagination`);
    }

    return discovery;
}

/** The tagged compaction strategy a definition follows (README §5). */
function expectCompactionStrategy(value: unknown, context: string): CompactionStrategy {
    return expectOneOf(
        value,
        [
            "native_preferred",
            "gateway_summary",
        ],
        context,
        "a compaction strategy",
    );
}

/**
 * The default support a mechanism declaration declares: absent is the
 * unknown state, exactly as the gateway's serde default parses it.
 */
function expectDefaultSupport(value: unknown, context: string): MechanismSupport {
    if (value === undefined) {
        return "unknown";
    }

    return expectOneOf(
        value,
        [
            "supported",
            "unsupported",
            "unknown",
        ],
        context,
        "a mechanism support state",
    );
}

/** One gateway budget: a positive integer, the wire mirror of a `NonZero` count. */
function expectBudget(value: unknown, context: string): number {
    const budget = expectInteger(value, context);

    if (budget <= 0) {
        throw new ContractViolationError(`${context} must be a positive integer`);
    }

    return budget;
}

/**
 * One typed positive unsupported-response classifier (README §5): the
 * statuses stay bare integers — the upstream HTTP statuses — and the code
 * and parameter rows must be non-empty strings; an empty row never
 * positively classifies, it is a typo the write names instead of storing.
 */
function expectUnsupportedClassifier(value: unknown, context: string): UnsupportedClassifier {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "statuses",
            "codes",
            "parameters",
        ],
        context,
    );

    const classifier: UnsupportedClassifier = {};

    const statuses = Reflect.get(record, "statuses");

    if (statuses !== undefined) {
        classifier.statuses = expectArray(statuses, `${context}.statuses`).map((status, index) => {
            const parsed = expectInteger(status, `${context}.statuses[${index}]`);

            if (parsed < 0 || parsed > 65_535) {
                throw new ContractViolationError(
                    `${context}.statuses[${index}] is not an HTTP status: ${String(status)}`,
                );
            }

            return parsed;
        });
    }

    const codes = Reflect.get(record, "codes");

    if (codes !== undefined) {
        classifier.codes = expectArray(codes, `${context}.codes`).map((code, index) => {
            const parsed = expectString(code, `${context}.codes[${index}]`);

            if (parsed === "") {
                throw new ContractViolationError(
                    `${context}.codes[${index}] must not be an empty code`,
                );
            }

            return parsed;
        });
    }

    const parameters = Reflect.get(record, "parameters");

    if (parameters !== undefined) {
        classifier.parameters = expectArray(parameters, `${context}.parameters`).map(
            (parameter, index) => {
                const parsed = expectString(parameter, `${context}.parameters[${index}]`);

                if (parsed === "") {
                    throw new ContractViolationError(
                        `${context}.parameters[${index}] must not be an empty parameter`,
                    );
                }

                return parsed;
            },
        );
    }

    return classifier;
}

/**
 * Counts the body placeholders of one template tree, naming an empty
 * object-member name on the way — the same walk the gateway's operation
 * validation performs over an envelope.
 */
function countBodyNodes(node: TemplateNode, context: string): number {
    switch (node.type) {
        case "json":
        case "string":
            return 0;
        case "body":
            return 1;
        case "object":
            return Object.entries(node.fields).reduce((total, [name, field]) => {
                if (name === "") {
                    throw new ContractViolationError(
                        `${context}.fields carries an empty object member name`,
                    );
                }

                return total + countBodyNodes(field, `${context}.fields.${name}`);
            }, 0);
        case "array":
            return node.items.reduce(
                (total, item, index) => total + countBodyNodes(item, `${context}.items[${index}]`),
                0,
            );
    }
}

/**
 * One operation-shaped connection surface's shared members (README §5): the
 * non-empty URL, the header rows, the envelope carrying exactly one body
 * placeholder, and the response-unwrap member — the browser-side mirror of
 * the gateway's operation-surface walk, shared by the standalone compact
 * operation and the token-count operation.
 */
function expectOperationConnection(
    record: object,
    context: string,
): Pick<CountTokensOperation, "url" | "headers" | "envelope" | "response_unwrap"> {
    const url = expectStringExpr(Reflect.get(record, "url"), `${context}.url`);

    if (url.length === 0) {
        throw new ContractViolationError(`${context}.url must carry at least one expression part`);
    }

    const connection: Pick<
        CountTokensOperation,
        "url" | "headers" | "envelope" | "response_unwrap"
    > = {
        url,
    };

    const headers = Reflect.get(record, "headers");

    if (headers !== undefined) {
        connection.headers = expectParamRows(headers, `${context}.headers`);
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        const node = expectTemplateNode(envelope, `${context}.envelope`);
        const bodyCount = countBodyNodes(node, `${context}.envelope`);

        if (bodyCount !== 1) {
            throw new ContractViolationError(
                `${context}.envelope carries ${bodyCount} body placeholders; exactly one is required`,
            );
        }

        connection.envelope = node;
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        const unwrapped = expectString(responseUnwrap, `${context}.response_unwrap`);

        if (unwrapped === "") {
            throw new ContractViolationError(`${context}.response_unwrap must not be empty`);
        }

        connection.response_unwrap = unwrapped;
    }

    return connection;
}

/** The standalone compact connection of a Responses definition (README §5). */
function expectStandaloneOperation(value: unknown, context: string): StandaloneCompactOperation {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "url",
            "headers",
            "envelope",
            "response_unwrap",
            "unsupported",
        ],
        context,
    );

    const operation: StandaloneCompactOperation = expectOperationConnection(record, context);

    const unsupported = Reflect.get(record, "unsupported");

    if (isPresentValue(unsupported)) {
        operation.unsupported = expectUnsupportedClassifier(unsupported, `${context}.unsupported`);
    }

    return operation;
}

/** The optional native token-count operation of a Messages definition (README §5). */
function expectCountTokensOperation(value: unknown, context: string): CountTokensOperation {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "url",
            "headers",
            "envelope",
            "response_unwrap",
        ],
        context,
    );

    return expectOperationConnection(record, context);
}

/**
 * Rejects a compaction declaration (or operation) the payload's format can
 * never speak: the declaration is an operation the definition could not
 * render, whatever its content (README §5).
 */
function requireCompactionFormat(
    format: UpstreamFormat,
    required: UpstreamFormat,
    subject: string,
    _context: string,
): void {
    if (format !== required) {
        throw new ContractViolationError(
            `the \`${subject}\` compaction declaration requires the \`${required}\` format, ` +
                `but the definition speaks \`${format}\``,
        );
    }
}

/** The Responses standalone-compact declaration (README §5). */
function expectStandaloneDeclaration(
    value: unknown,
    context: string,
): StandaloneCompactDeclaration {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "default",
            "operation",
            "unsupported",
        ],
        context,
    );

    const declaration: StandaloneCompactDeclaration = {
        default: expectDefaultSupport(Reflect.get(record, "default"), `${context}.default`),
    };

    const operation = Reflect.get(record, "operation");

    if (isPresentValue(operation)) {
        declaration.operation = expectStandaloneOperation(operation, `${context}.operation`);
    }

    const unsupported = Reflect.get(record, "unsupported");

    if (isPresentValue(unsupported)) {
        declaration.unsupported = expectUnsupportedClassifier(
            unsupported,
            `${context}.unsupported`,
        );
    }

    // A definition cannot advertise native support without configuring how
    // to execute it. The check runs over the rebuilt declaration, so an
    // explicit `null` operation counts as absent, exactly as the gateway's
    // parse makes it.
    if (declaration.default === "supported" && declaration.operation === undefined) {
        throw new ContractViolationError(
            `${context} declares \`supported\` without an operation; a definition cannot ` +
                "advertise native support without configuring how to execute it",
        );
    }

    return declaration;
}

/**
 * The trigger-shaped declarations — the Responses explicit trigger and the
 * automatic threshold compaction (README §5): the mechanism rides the
 * ordinary inference connection, so no operation data exists to require.
 */
function expectTriggerLikeDeclaration(value: unknown, context: string): TriggerDeclaration {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "default",
            "unsupported",
        ],
        context,
    );

    const declaration: TriggerDeclaration = {
        default: expectDefaultSupport(Reflect.get(record, "default"), `${context}.default`),
    };

    const unsupported = Reflect.get(record, "unsupported");

    if (isPresentValue(unsupported)) {
        declaration.unsupported = expectUnsupportedClassifier(
            unsupported,
            `${context}.unsupported`,
        );
    }

    return declaration;
}

/**
 * The Messages declarations — the on-demand and the threshold compaction
 * (README §5): the beta overlay headers the operation requires, present and
 * non-empty whenever the declaration declares support. The shared shape is
 * stated structurally — both generated declarations carry it identically,
 * and an added member breaks the assignment into the declarations record.
 */
function expectOverlayDeclaration(
    value: unknown,
    context: string,
    mechanism: string,
): {
    default: MechanismSupport;
    overlay_headers?: Array<ParamRow>;
    unsupported?: UnsupportedClassifier;
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "default",
            "overlay_headers",
            "unsupported",
        ],
        context,
    );

    const declaration: {
        default: MechanismSupport;
        overlay_headers?: Array<ParamRow>;
        unsupported?: UnsupportedClassifier;
    } = {
        default: expectDefaultSupport(Reflect.get(record, "default"), `${context}.default`),
    };

    const overlayHeaders = Reflect.get(record, "overlay_headers");

    if (isPresentValue(overlayHeaders)) {
        declaration.overlay_headers = expectParamRows(overlayHeaders, `${context}.overlay_headers`);
    }

    const unsupported = Reflect.get(record, "unsupported");

    if (isPresentValue(unsupported)) {
        declaration.unsupported = expectUnsupportedClassifier(
            unsupported,
            `${context}.unsupported`,
        );
    }

    if (declaration.default === "supported") {
        const headers = declaration.overlay_headers;

        if (headers === undefined || headers.length === 0) {
            throw new ContractViolationError(
                `the \`${mechanism}\` compaction declaration declares \`supported\` without ` +
                    "overlay headers; a definition cannot advertise native support without " +
                    "configuring how to execute it",
            );
        }
    }

    return declaration;
}

/** The per-mechanism support declarations of one compaction section (README §5). */
function expectMechanismDeclarations(
    value: unknown,
    context: string,
    format: UpstreamFormat,
): MechanismDeclarations {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "responses_standalone_compact",
            "responses_explicit_trigger",
            "responses_automatic_compaction",
            "messages_on_demand_compaction",
            "messages_threshold_compaction",
        ],
        context,
    );

    const declarations: MechanismDeclarations = {};

    const standalone = Reflect.get(record, "responses_standalone_compact");

    if (isPresentValue(standalone)) {
        requireCompactionFormat(
            format,
            "openai_response",
            "responses_standalone_compact",
            `${context}.responses_standalone_compact`,
        );
        declarations.responses_standalone_compact = expectStandaloneDeclaration(
            standalone,
            `${context}.responses_standalone_compact`,
        );
    }

    const trigger = Reflect.get(record, "responses_explicit_trigger");

    if (isPresentValue(trigger)) {
        requireCompactionFormat(
            format,
            "openai_response",
            "responses_explicit_trigger",
            `${context}.responses_explicit_trigger`,
        );
        declarations.responses_explicit_trigger = expectTriggerLikeDeclaration(
            trigger,
            `${context}.responses_explicit_trigger`,
        );
    }

    const automatic = Reflect.get(record, "responses_automatic_compaction");

    if (isPresentValue(automatic)) {
        requireCompactionFormat(
            format,
            "openai_response",
            "responses_automatic_compaction",
            `${context}.responses_automatic_compaction`,
        );
        declarations.responses_automatic_compaction = expectTriggerLikeDeclaration(
            automatic,
            `${context}.responses_automatic_compaction`,
        );
    }

    const onDemand = Reflect.get(record, "messages_on_demand_compaction");

    if (isPresentValue(onDemand)) {
        requireCompactionFormat(
            format,
            "anthropic_messages",
            "messages_on_demand_compaction",
            `${context}.messages_on_demand_compaction`,
        );
        declarations.messages_on_demand_compaction = expectOverlayDeclaration(
            onDemand,
            `${context}.messages_on_demand_compaction`,
            "messages_on_demand_compaction",
        );
    }

    const threshold = Reflect.get(record, "messages_threshold_compaction");

    if (isPresentValue(threshold)) {
        requireCompactionFormat(
            format,
            "anthropic_messages",
            "messages_threshold_compaction",
            `${context}.messages_threshold_compaction`,
        );
        declarations.messages_threshold_compaction = expectOverlayDeclaration(
            threshold,
            `${context}.messages_threshold_compaction`,
            "messages_threshold_compaction",
        );
    }

    return declarations;
}

/**
 * The remote-compaction section of one provider definition payload
 * (README §5), rebuilt field by field: the strategy, the positive budgets,
 * the export policy, the per-format-gated mechanism declarations, and the
 * optional token-count operation. Every member the operator omitted lands
 * on its shipped default, the exact members the gateway's serde defaults
 * parse — so the rebuilt payload restates the section canonically.
 */
function expectCompactionSection(
    value: unknown,
    context: string,
    format: UpstreamFormat,
): CompactionSection {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "strategy",
            "summary_budget",
            "retention_budget",
            "can_export_native_summary",
            "mechanisms",
            "count_tokens",
        ],
        context,
    );

    const strategy = Reflect.get(record, "strategy");
    const summaryBudget = Reflect.get(record, "summary_budget");
    const retentionBudget = Reflect.get(record, "retention_budget");
    const canExportNativeSummary = Reflect.get(record, "can_export_native_summary");
    const mechanisms = Reflect.get(record, "mechanisms");

    const section: CompactionSection = {
        strategy:
            strategy === undefined
                ? "native_preferred"
                : expectCompactionStrategy(strategy, `${context}.strategy`),
        summary_budget:
            summaryBudget === undefined
                ? 4096
                : expectBudget(summaryBudget, `${context}.summary_budget`),
        retention_budget:
            retentionBudget === undefined
                ? 8192
                : expectBudget(retentionBudget, `${context}.retention_budget`),
        can_export_native_summary:
            canExportNativeSummary === undefined
                ? false
                : expectBoolean(canExportNativeSummary, `${context}.can_export_native_summary`),
        mechanisms:
            mechanisms === undefined
                ? {}
                : expectMechanismDeclarations(mechanisms, `${context}.mechanisms`, format),
    };

    const countTokens = Reflect.get(record, "count_tokens");

    if (isPresentValue(countTokens)) {
        if (format !== "anthropic_messages") {
            throw new ContractViolationError(
                `the \`count_tokens\` compaction operation requires the \`anthropic_messages\` ` +
                    `format, but the definition speaks \`${format}\``,
            );
        }

        section.count_tokens = expectCountTokensOperation(countTokens, `${context}.count_tokens`);
    }

    return section;
}

/** One durable slot initializer, rebuilt (README §5.3). */
function expectSlotInitializer(
    value: unknown,
    context: string,
): {
    name: string;
    generator: "uuid" | "uuid_hex";
} {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "name",
            "generator",
        ],
        context,
    );
    const generator = expectOneOf(
        expectString(Reflect.get(record, "generator"), `${context}.generator`),
        [
            "uuid",
            "uuid_hex",
        ],
        `${context}.generator`,
        "a slot generator",
    );

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        generator,
    };
}

/** The OAuth authorization grant a flow runs, rebuilt from its kind (README §5.3). */
function expectGrant(value: unknown, context: string): Grant {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "authorization_code") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorize_url",
                "redirect_uri",
                "pkce",
                "extra_params",
            ],
            context,
        );

        const pkce = expectOneOf(
            expectString(Reflect.get(record, "pkce"), `${context}.pkce`),
            [
                "none",
                "s256",
            ],
            `${context}.pkce`,
            "a PKCE mode",
        );

        const grant: Grant = {
            type: "authorization_code",
            authorize_url: expectStringExpr(
                Reflect.get(record, "authorize_url"),
                `${context}.authorize_url`,
            ),
            redirect_uri: expectString(
                Reflect.get(record, "redirect_uri"),
                `${context}.redirect_uri`,
            ),
            pkce,
        };

        const extraParams = Reflect.get(record, "extra_params");

        if (extraParams !== undefined) {
            grant.extra_params = expectParamRows(extraParams, `${context}.extra_params`);
        }

        return grant;
    }

    if (type === "device") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "authorization",
                "expires_in_fallback_seconds",
            ],
            context,
        );

        const authorizationRecord = expectRecord(
            Reflect.get(record, "authorization"),
            `${context}.authorization`,
        );
        expectDeclaredKeys(
            authorizationRecord,
            [
                "request",
                "response",
            ],
            `${context}.authorization`,
        );

        const grant: Grant = {
            type: "device",
            authorization: {
                request: expectControlRequest(
                    Reflect.get(authorizationRecord, "request"),
                    `${context}.authorization.request`,
                ),
                response: expectResponseDescriptor(
                    Reflect.get(authorizationRecord, "response"),
                    `${context}.authorization.response`,
                ),
            },
        };

        const fallback = Reflect.get(record, "expires_in_fallback_seconds");

        if (isPresentValue(fallback)) {
            grant.expires_in_fallback_seconds = expectInteger(
                fallback,
                `${context}.expires_in_fallback_seconds`,
            );
        }

        return grant;
    }

    throw new ContractViolationError(`${context}.type is not a grant kind: ${type}`);
}

/** The token exchange redeeming a grant's product (README §5.3). */
function expectExchange(value: unknown, context: string): Exchange {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "request",
            "response",
            "terminal_codes",
        ],
        context,
    );

    return {
        request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
        response: expectResponseDescriptor(Reflect.get(record, "response"), `${context}.response`),
        terminal_codes: expectTerminalCodes(
            Reflect.get(record, "terminal_codes"),
            `${context}.terminal_codes`,
        ),
    };
}

/** How an OAuth-acquired credential renews, rebuilt from its mode (README §5.3). */
function expectTokenRefresh(value: unknown, context: string): TokenRefresh {
    const record = expectRecord(value, context);
    const type = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (type === "grant") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "request",
                "response",
                "terminal_codes",
            ],
            context,
        );

        const refresh: Extract<
            TokenRefresh,
            {
                type: "grant";
            }
        > = {
            type: "grant",
            request: expectControlRequest(Reflect.get(record, "request"), `${context}.request`),
            terminal_codes: expectTerminalCodes(
                Reflect.get(record, "terminal_codes"),
                `${context}.terminal_codes`,
            ),
        };

        const response = Reflect.get(record, "response");

        if (isPresentValue(response)) {
            refresh.response = expectResponseDescriptor(response, `${context}.response`);
        }

        return refresh;
    }

    if (type === "reprovision" || type === "none") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type,
        };
    }

    throw new ContractViolationError(`${context}.type is not a refresh mode: ${type}`);
}

/**
 * The credential acquisition of a definition, rebuilt from its kind
 * (README §5.3): an `api_key` credential is its tag alone, an `oauth`
 * credential the full recipe.
 */
function expectCredential(value: unknown, context: string): CredentialAcquisition {
    const record = expectRecord(value, context);
    const kind = expectString(Reflect.get(record, "type"), `${context}.type`);

    if (kind === "api_key") {
        expectDeclaredKeys(
            record,
            [
                "type",
            ],
            context,
        );
        return {
            type: "api_key",
        };
    }

    if (kind === "oauth") {
        expectDeclaredKeys(
            record,
            [
                "type",
                "client_id",
                "client_secret",
                "access_token_lifetime_seconds",
                "initial_slots",
                "grant",
                "exchange",
                "id_token_claims",
                "refresh",
            ],
            context,
        );

        const oauth: Extract<
            CredentialAcquisition,
            {
                type: "oauth";
            }
        > = {
            type: "oauth",
            client_id: expectString(Reflect.get(record, "client_id"), `${context}.client_id`),
            grant: expectGrant(Reflect.get(record, "grant"), `${context}.grant`),
            exchange: expectExchange(Reflect.get(record, "exchange"), `${context}.exchange`),
            refresh: expectTokenRefresh(Reflect.get(record, "refresh"), `${context}.refresh`),
        };

        const clientSecret = Reflect.get(record, "client_secret");

        if (isPresentValue(clientSecret)) {
            oauth.client_secret = expectString(clientSecret, `${context}.client_secret`);
        }

        const lifetime = Reflect.get(record, "access_token_lifetime_seconds");

        if (isPresentValue(lifetime)) {
            oauth.access_token_lifetime_seconds = expectInteger(
                lifetime,
                `${context}.access_token_lifetime_seconds`,
            );
        }

        const initialSlots = Reflect.get(record, "initial_slots");

        if (initialSlots !== undefined) {
            oauth.initial_slots = expectArray(initialSlots, `${context}.initial_slots`).map(
                (slot, index) => expectSlotInitializer(slot, `${context}.initial_slots[${index}]`),
            );
        }

        const idTokenClaims = Reflect.get(record, "id_token_claims");

        if (idTokenClaims !== undefined) {
            oauth.id_token_claims = expectArray(idTokenClaims, `${context}.id_token_claims`).map(
                (claim, index) => {
                    const claimRecord = expectRecord(claim, `${context}.id_token_claims[${index}]`);
                    expectDeclaredKeys(
                        claimRecord,
                        [
                            "claim",
                            "slot",
                        ],
                        `${context}.id_token_claims[${index}]`,
                    );

                    return {
                        claim: expectResponsePath(
                            Reflect.get(claimRecord, "claim"),
                            `${context}.id_token_claims[${index}].claim`,
                        ),
                        slot: expectString(
                            Reflect.get(claimRecord, "slot"),
                            `${context}.id_token_claims[${index}].slot`,
                        ),
                    };
                },
            );
        }

        return oauth;
    }

    throw new ContractViolationError(`${context}.type is not a credential kind: ${kind}`);
}

/**
 * One stored definition version: the `(name, version)` identity and the
 * creation timestamp — the fields the app reads — built beside the payload
 * the shared walk above reconstructs, so the value is assembled from
 * validated fields and never asserted.
 */
function parseDefinitionResponse(value: unknown, context: string): DefinitionResponse {
    const record = expectRecord(value, context);

    const payload = expectDefinitionPayload(
        {
            format: Reflect.get(record, "format"),
            inference: Reflect.get(record, "inference"),
            credential: Reflect.get(record, "credential"),
            provisioning: Reflect.get(record, "provisioning"),
            discovery: Reflect.get(record, "discovery"),
            compaction: Reflect.get(record, "compaction"),
            quota: Reflect.get(record, "quota"),
        },
        context,
    );

    return {
        name: expectString(Reflect.get(record, "name"), `${context}.name`),
        version: expectInteger(Reflect.get(record, "version"), `${context}.version`),
        created_at: expectInteger(Reflect.get(record, "created_at"), `${context}.created_at`),
        ...payload,
    };
}

export function parseListProviderDefinitionsResponse(
    value: unknown,
): ListProviderDefinitionsResponse {
    return parseListResponse(value, "provider_definitions", parseDefinitionResponse);
}

export function parseDefinitionEnvelope(value: unknown): DefinitionEnvelope {
    return parseEnvelopeResponse(value, "provider_definition", parseDefinitionResponse);
}
