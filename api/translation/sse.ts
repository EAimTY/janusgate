export interface SseFrame {
    event?: string;
    data: string;
}

export type SseFrameHandler = (frame: SseFrame) => void;

export function encodeSse(event: string | undefined, data: unknown): string {
    const encodedData = typeof data === "string" ? data : JSON.stringify(data);
    const eventLine = event === undefined ? "" : `event: ${event}\n`;
    return `${eventLine}data: ${encodedData}\n\n`;
}

export class SseParser {
    readonly #handleFrame: SseFrameHandler;
    #buffer = "";

    constructor(handleFrame: SseFrameHandler) {
        this.#handleFrame = handleFrame;
    }

    push(chunk: string): void {
        this.#buffer += chunk.replaceAll("\r\n", "\n").replaceAll("\r", "\n");
        let boundary = this.#buffer.indexOf("\n\n");
        while (boundary >= 0) {
            const block = this.#buffer.slice(0, boundary);
            this.#buffer = this.#buffer.slice(boundary + 2);
            this.#emitBlock(block);
            boundary = this.#buffer.indexOf("\n\n");
        }
    }

    finish(): void {
        if (this.#buffer.length > 0) {
            this.#emitBlock(this.#buffer);
            this.#buffer = "";
        }
    }

    #emitBlock(block: string): void {
        let event: string | undefined;
        const dataLines: string[] = [];
        for (const line of block.split("\n")) {
            if (line.startsWith(":")) {
                continue;
            }
            if (line.startsWith("event:")) {
                event = line.slice(6).trimStart();
            } else if (line.startsWith("data:")) {
                const data = line.slice(5);
                dataLines.push(data.startsWith(" ") ? data.slice(1) : data);
            }
        }
        if (dataLines.length === 0) {
            return;
        }
        const frame: SseFrame = {
            data: dataLines.join("\n"),
        };
        if (event !== undefined) {
            frame.event = event;
        }
        this.#handleFrame(frame);
    }
}
