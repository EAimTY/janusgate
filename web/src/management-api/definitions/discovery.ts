import { ContractViolationError } from "../errors";
import type { Discovery, DiscoveryExtract, Pagination } from "../generated/v0";
import {
    expectDeclaredKeys,
    expectNonEmptyString,
    expectPositiveInteger,
    expectRecord,
    expectString,
    isPresentValue,
} from "../validation-primitives";
import { expectControlRequest, expectResponsePath } from "./control";

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

        // The identifier field names the member inside every page entry: an
        // empty chain addresses the entry itself, which is never the
        // identifier, so the extraction would fail on every entry.
        if (extract.id_field.length === 0) {
            throw new ContractViolationError(`${context}.id_field must not be empty`);
        }

        const stripPrefix = Reflect.get(record, "strip_prefix");

        if (isPresentValue(stripPrefix)) {
            extract.strip_prefix = expectNonEmptyString(stripPrefix, `${context}.strip_prefix`);
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

        const responsePath = expectResponsePath(
            Reflect.get(record, "response_path"),
            `${context}.response_path`,
        );

        // An empty chain addresses the page root, which can never continue
        // the walk: the root object reads as a malformed token on every page.
        if (responsePath.length === 0) {
            throw new ContractViolationError(`${context}.response_path must not be empty`);
        }

        return {
            type: "token",
            request_param: expectNonEmptyString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            response_path: responsePath,
            max_pages: expectPositiveInteger(
                Reflect.get(record, "max_pages"),
                `${context}.max_pages`,
            ),
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

        const hasMorePath = expectResponsePath(
            Reflect.get(record, "has_more_path"),
            `${context}.has_more_path`,
        );
        const lastIdPath = expectResponsePath(
            Reflect.get(record, "last_id_path"),
            `${context}.last_id_path`,
        );

        // An empty has-more chain addresses the page root, whose truthy read
        // is always false: the walk would end after page one with the page
        // budget never firing. An empty last-identifier chain names no member
        // to continue from, so both must address at least one member.
        if (hasMorePath.length === 0) {
            throw new ContractViolationError(`${context}.has_more_path must not be empty`);
        }

        if (lastIdPath.length === 0) {
            throw new ContractViolationError(`${context}.last_id_path must not be empty`);
        }

        return {
            type: "cursor",
            has_more_path: hasMorePath,
            last_id_path: lastIdPath,
            request_param: expectNonEmptyString(
                Reflect.get(record, "request_param"),
                `${context}.request_param`,
            ),
            max_pages: expectPositiveInteger(
                Reflect.get(record, "max_pages"),
                `${context}.max_pages`,
            ),
        };
    }

    throw new ContractViolationError(`${context}.type is not a pagination mode: ${type}`);
}

/** The upstream model-list discovery of a definition (README §5.5). */
export function expectDiscovery(value: unknown, context: string): Discovery {
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
