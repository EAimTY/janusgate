import { render } from "@solidjs/web";
import { createInstance } from "i18next";
import { describe, expect, it } from "vitest";
import { connectedMessage, discoveredMessage, providerModelMessage } from "./rich";

describe("application-owned rich slots", () => {
    it("obeys catalog order and inserts hostile-looking values as literal isolated text", async () => {
        const instance = createInstance();
        await instance.init({
            lng: "en",
            defaultNS: "translation",
            interpolation: {
                escapeValue: false,
            },
            resources: {
                en: {
                    translation: {
                        notices: {
                            discovered_other: "[[quantity]] models from [[providerName]].",
                        },
                        rich: {
                            providerModel: "[[modelId]] belongs to [[providerLink]].",
                            connected: "Connected to [[baseUrl]].",
                        },
                    },
                },
            },
        });
        const host = document.createElement("div");
        document.body.append(host);
        const name = "<script> [[modelId]] العربية";
        const dispose = render(
            () => (
                <>
                    {providerModelMessage(instance.getFixedT("en", "translation"), {
                        modelId: "gpt-5",
                        providerName: name,
                        providerHref: "/providers/1",
                    })}
                    {discoveredMessage(instance.getFixedT("en", "translation"), name, 1500)}
                    {connectedMessage(
                        instance.getFixedT("en", "translation"),
                        "https://example.com",
                    )}
                </>
            ),
            host,
        );
        expect(host.textContent).toBe(
            `gpt-5 belongs to ${name}.1,500 models from ${name}.Connected to https://example.com.`,
        );
        expect(host.querySelector("script")).toBeNull();
        expect(host.querySelectorAll("a")).toHaveLength(1);
        expect(host.querySelector("a bdi")?.getAttribute("dir")).toBe("auto");
        expect(host.querySelector("bdi")?.getAttribute("dir")).toBe("ltr");
        dispose();
        host.remove();
    });
});
