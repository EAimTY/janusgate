import type { CreateProviderDefinitionRequest } from "../generated/v0";

/**
 * The authored definition payload's validator (`docs/design/providers.md`): the browser-side
 * mirror of the gateway's descriptor contract. A definition's descriptor is
 * authored JSON — a preset's emission or an operator's edit — and one
 * structural walk over it serves both directions: the creation page validates
 * the edited text before it is sent, and the stored-definition responses
 * build their values through the same walk.
 */

/**
 * The authored definition payload (`docs/design/providers.md`): a definition's descriptor as
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
