import { Hono } from "hono";
import api from "./api/app";
import panelApi from "./panel-api/app";

const app = new Hono();

app.route("/api", api);
app.route("/panel-api", panelApi);
app.notFound((context) => context.body(null, 404));

export default app;
