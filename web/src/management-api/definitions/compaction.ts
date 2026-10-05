import { ContractViolationError } from "../errors";
import type {
    CompactionSection,
    CompactionStrategy,
    CountTokensOperation,
    MechanismDeclarations,
    MechanismSupport,
    ParamRow,
    StandaloneCompactDeclaration,
    StandaloneCompactOperation,
    TemplateNode,
    TriggerDeclaration,
    UnsupportedClassifier,
    UpstreamFormat,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectInteger,
    expectRecord,
    expectString,
} from "../validation-primitives";
import { expectParamRows, expectStringExpr, expectTemplateNode } from "./expressions";
import { expectDeclaredKeys, expectOneOf, isPresentValue } from "./validation";

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
export function expectCompactionSection(
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
