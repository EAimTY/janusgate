import { ContractViolationError } from "../errors";
import type { Discovery, DiscoveryExtract, Pagination } from "../generated/v0";
import {
    expectDeclaredKeys,
    expectInteger,
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
