/**
 * The gateway's outward serving surfaces: the client dialects one model set
 * answers, each at its own path (README §1 *Outward Protocols*). Routing is
 * by public model ID alone, so every surface serves every upstream kind and
 * the paths are the only thing that distinguishes them from a client's side.
 */
export type OutwardSurface = {
    /** The dialect family, as the console names it. */
    readonly label: string;
    /** The path, relative to the instance's base URL. */
    readonly path: string;
    /** What a client speaks when it talks to this path. */
    readonly speaks: string;
};

/** The three surfaces, in the order the console lists them. */
export const OUTWARD_SURFACES: ReadonlyArray<OutwardSurface> = [
    {
        label: "OpenAI-compatible",
        path: "/api/openai-v1",
        speaks: "Chat Completions and Responses",
    },
    {
        label: "Gemini",
        path: "/api/gemini-v1beta",
        speaks: "GenerateContent and Interactions",
    },
    {
        label: "Anthropic",
        path: "/api/anthropic-v1",
        speaks: "Messages",
    },
];
