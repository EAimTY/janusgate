# JanusGate Console

The official administration frontend for [JanusGate](../README.md): a statically hosted SPA that connects to a gateway using its management base URL and token. Use it to manage upstream accounts, model inventory, pools, public routes, serving-key permissions, and usage. The gateway and console deploy independently.

## Run locally

From the repository root:

```bash
npm ci
npm run generate-bindings -w web
npm run dev:web
```

Open `http://localhost:56625`. The bindings step requires Rust and generates the exact management API types from the gateway; rerun it after payload changes. Configure that origin in the gateway's `management_api.cors.allowed_origin` and restart the gateway.

Enter the gateway address and raw management token on the connection screen. Create an upstream provider, discover or add physical models, and issue an enabled serving key with access to the desired models. Standalone models are exposed automatically; pools and explicit aliases are optional. The [administration guide](../docs/guides/administration.md) explains setup and recovery.

## Build and host

```bash
npm run build:web
```

Host `target/web/dist/` on a static server. Serve `index.html` for unknown application paths so bookmarked detail pages work. Preserve real asset 404s and use suitable cache headers for the HTML entry and hashed assets. The gateway does not host this directory.

Add the deployed console's exact origin to the gateway's management CORS policy. An HTTPS console needs an HTTPS gateway reachable by that browser. Each browser connects directly to the chosen gateway; there is no console backend or proxy.

## Browser behavior

The console remembers the connection address/token, theme, and language preference in localStorage when available. Failed storage leaves the session usable in memory. Disconnect clears the saved connection; a current client's 401 returns to connection setup. The token is stored as plaintext, so use a browser/profile you trust.

Themes follow the OS unless overridden. Languages are English, Simplified Chinese, Traditional Chinese, Japanese, Korean, and Spanish; system selection follows browser language preferences. User-entered identifiers and gateway messages remain their original text. No external fonts, images, or runtime asset services are required.

Reads run on page entry and explicit refresh. The console does not continuously poll account quota. Unsaved edits survive refresh; navigation asks for confirmation while an edit or OAuth flow is active. Resource actions report their results where performed.

## Develop

Run `npm run check` from the repository root after web code changes. SolidJS, TypeScript, Vite, and Tailwind implement the app; generated bindings own management wire shapes. See [development](../docs/development.md), [web architecture](docs/architecture.md), [localization](docs/localization.md), and [working rules](AGENTS.md).

[Product intent](../PRODUCT.md) defines the audience and promises. [DESIGN.md](DESIGN.md) defines the visual system; the [surface brief](.impeccable/surfaces/web.md) records the console's interaction direction. These documents apply together when changing the interface.
