import { render } from "@solidjs/web";
import { App } from "./app";
import { initializeLocalization } from "./i18n";
import { initTheme } from "./theme";
import "./tailwind.css";

initTheme();

const root = document.getElementById("root");
if (root === null) throw new Error("Missing root element");

await initializeLocalization();
render(() => <App />, root);
