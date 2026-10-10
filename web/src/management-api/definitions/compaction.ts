import { ContractViolationError } from "../errors";
import type {
    CompactionSection,
    CompactionStrategy,
    CountTokensOperation,
    MechanismDeclarations,
    MechanismSupport,
    OverlayDeclaration,
    RideAlongDeclaration,
    StandaloneCompactDeclaration,
    StandaloneCompactOperation,
    UnsupportedClassifier,
    UpstreamFormat,
} from "../generated/v0";
import {
    expectArray,
    expectBoolean,
    expectDeclaredKeys,
    expectInteger,
    expectNonEmptyString,
    expectOneOf,
    expectPositiveInteger,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import type { CompactionMechanism } from "../vocabulary";
import {
    countTemplateBodyNodes,
    expectHeaderRows,
    expectStringExpr,
    expectTemplateNode,
} from "./expressions";

/** The tagged compaction strategy a definition follows (`docs/design/providers.md`). */
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

/**
 * One typed positive unsupported-response classifier (`docs/design/providers.md`): the
 * statuses are non-success HTTP statuses only — the classifier inspects
 * rejected native calls alone, so a success status or a value outside the
 * HTTP status range is a criterion no response can ever carry — and the code
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

            if (parsed < 100 || parsed > 999 || (parsed >= 200 && parsed < 300)) {
                throw new ContractViolationError(
                    `${context}.statuses[${index}] is not a non-success HTTP status: ${String(status)}`,
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
 * One operation-shaped connection surface's shared members (`docs/design/providers.md`): the
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
        connection.headers = expectHeaderRows(headers, `${context}.headers`);
    }

    const envelope = Reflect.get(record, "envelope");

    if (isPresentValue(envelope)) {
        const node = expectTemplateNode(envelope, `${context}.envelope`);
        const bodyCount = countTemplateBodyNodes(node);

        if (bodyCount !== 1) {
            throw new ContractViolationError(
                `${context}.envelope carries ${bodyCount} body placeholders; exactly one is required`,
            );
        }

        connection.envelope = node;
    }

    const responseUnwrap = Reflect.get(record, "response_unwrap");

    if (isPresentValue(responseUnwrap)) {
        connection.response_unwrap = expectNonEmptyString(
            responseUnwrap,
            `${context}.response_unwrap`,
        );
    }

    return connection;
}

/** The standalone compact connection of a Responses definition (`docs/design/providers.md`). */
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

/** The optional native token-count operation of a Messages definition (`docs/design/providers.md`). */
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
 * render, whatever its content (`docs/design/providers.md`).
 */
function requireCompactionFormat(
    format: UpstreamFormat,
    required: UpstreamFormat,
    subject: string,
): void {
    if (format !== required) {
        throw new ContractViolationError(
            `the \`${subject}\` compaction declaration requires the \`${required}\` format, ` +
                `but the definition speaks \`${format}\``,
        );
    }
}

/** The Responses standalone-compact declaration (`docs/design/providers.md`). */
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
 * automatic threshold compaction (`docs/design/providers.md`): the mechanism rides the
 * ordinary inference connection, so no operation data exists to require.
 */
function expectTriggerLikeDeclaration(value: unknown, context: string): RideAlongDeclaration {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        [
            "default",
            "unsupported",
        ],
        context,
    );

    const declaration: RideAlongDeclaration = {
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
 * (`docs/design/providers.md`): the beta overlay headers the operation requires, present and
 * non-empty whenever the declaration declares support.
 */
function expectOverlayDeclaration(
    value: unknown,
    context: string,
    mechanism: string,
): OverlayDeclaration {
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

    const declaration: OverlayDeclaration = {
        default: expectDefaultSupport(Reflect.get(record, "default"), `${context}.default`),
    };

    const overlayHeaders = Reflect.get(record, "overlay_headers");

    if (isPresentValue(overlayHeaders)) {
        declaration.overlay_headers = expectHeaderRows(
            overlayHeaders,
            `${context}.overlay_headers`,
        );
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

        if (headers === undefined || headers === null || headers.length === 0) {
            throw new ContractViolationError(
                `the \`${mechanism}\` compaction declaration declares \`supported\` without ` +
                    "overlay headers; a definition cannot advertise native support without " +
                    "configuring how to execute it",
            );
        }
    }

    return declaration;
}

/**
 * One mechanism's declaration walk, beside the one upstream format that can
 * speak it (`docs/design/providers.md`), in the mechanisms record's declared-key order. Each
 * walk parses the present declaration into the section's rebuilt record; the
 * overlay walks restate the mechanism key their rejection message names.
 */
const MECHANISM_DECLARATIONS: ReadonlyArray<{
    readonly key: CompactionMechanism;
    readonly requiredFormat: UpstreamFormat;
    readonly parse: (
        value: unknown,
        context: string,
        declarations: MechanismDeclarations,
        mechanism: string,
    ) => void;
}> = [
    {
        key: "responses_standalone_compact",
        requiredFormat: "openai_response",
        parse: (value, context, declarations) => {
            declarations.responses_standalone_compact = expectStandaloneDeclaration(value, context);
        },
    },
    {
        key: "responses_explicit_trigger",
        requiredFormat: "openai_response",
        parse: (value, context, declarations) => {
            declarations.responses_explicit_trigger = expectTriggerLikeDeclaration(value, context);
        },
    },
    {
        key: "responses_automatic_compaction",
        requiredFormat: "openai_response",
        parse: (value, context, declarations) => {
            declarations.responses_automatic_compaction = expectTriggerLikeDeclaration(
                value,
                context,
            );
        },
    },
    {
        key: "messages_on_demand_compaction",
        requiredFormat: "anthropic_messages",
        parse: (value, context, declarations, mechanism) => {
            declarations.messages_on_demand_compaction = expectOverlayDeclaration(
                value,
                context,
                mechanism,
            );
        },
    },
    {
        key: "messages_threshold_compaction",
        requiredFormat: "anthropic_messages",
        parse: (value, context, declarations, mechanism) => {
            declarations.messages_threshold_compaction = expectOverlayDeclaration(
                value,
                context,
                mechanism,
            );
        },
    },
];

/** The per-mechanism support declarations of one compaction section (`docs/design/providers.md`). */
function expectMechanismDeclarations(
    value: unknown,
    context: string,
    format: UpstreamFormat,
): MechanismDeclarations {
    const record = expectRecord(value, context);
    expectDeclaredKeys(
        record,
        MECHANISM_DECLARATIONS.map((declaration) => declaration.key),
        context,
    );

    const declarations: MechanismDeclarations = {};

    for (const { key, requiredFormat, parse } of MECHANISM_DECLARATIONS) {
        const declaration = Reflect.get(record, key);

        if (isPresentValue(declaration)) {
            requireCompactionFormat(format, requiredFormat, key);
            parse(declaration, `${context}.${key}`, declarations, key);
        }
    }

    return declarations;
}

/**
 * The remote-compaction section of one provider definition payload
 * (`docs/design/providers.md`), rebuilt field by field: the strategy, the positive budgets,
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
                : expectPositiveInteger(summaryBudget, `${context}.summary_budget`),
        retention_budget:
            retentionBudget === undefined
                ? 8192
                : expectPositiveInteger(retentionBudget, `${context}.retention_budget`),
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
