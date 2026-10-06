import { render } from "@solidjs/web";
import { describe, expect, it } from "vitest";
import { connectedMessage, discoveredMessage, networkTitleMessage } from "./rich";

describe("application-owned rich slots", () => {
    it("obeys catalog order and inserts hostile-looking values as literal isolated text", () => {
        const name = "<script> [[baseUrl]] العربية";
        const host = document.createElement("div");
        document.body.append(host);
        const dispose = render(
            () => (
                <>
                    {discoveredMessage(name, 1500)}
                    {networkTitleMessage("unreachable", "https://example.com")}
                    {connectedMessage("https://example.com")}
                </>
            ),
            host,
        );
        expect(host.textContent).toBe(
            `1,500 models discovered from ${name}.Nothing answered at https://example.com.Connected to https://example.com — the token lives only in this browser.`,
        );
        expect(host.querySelector("script")).toBeNull();
        expect(host.querySelectorAll("bdi[dir=ltr]")).toHaveLength(3);
        expect(host.querySelector("bdi[dir=auto]")?.textContent).toBe(name);
        dispose();
        host.remove();
    });
});
