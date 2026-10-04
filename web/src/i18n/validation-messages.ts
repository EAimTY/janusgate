import type { BaseUrlFailure } from "/src/connection/model";
import { translation } from "./index";

/** Semantic input failure from normalization; no language enters connection data logic. */
export function baseUrlFailureMessage(reason: BaseUrlFailure): string {
    switch (reason) {
        case "empty":
            return translation()(($) => $.connectionValidation.empty);
        case "not-absolute":
            return translation()(($) => $.connectionValidation.notAbsolute);
        case "scheme":
            return translation()(($) => $.connectionValidation.scheme);
        case "credentials":
            return translation()(($) => $.connectionValidation.credentials);
        case "not-origin":
            return translation()(($) => $.connectionValidation.notOrigin);
    }
}
