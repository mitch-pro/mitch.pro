// Run against the built Woodcreek image with --network none. The account
// provider and all HTTP/WebSocket traffic stay on loopback; no bots are launched.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { resolve } from "node:path";
const app =
  process.env.WOODCREEK_APP_ROOT ||
  resolve(import.meta.dirname, "../integrations/woodcreek/backend");
const provider = createServer(async (req, res) => {
  if (req.headers.authorization !== "Bearer wc1.fixture") {
    res.writeHead(401);
    return res.end("{}");
  }
  res.setHeader("Content-Type", "application/json");
  res.end(
    JSON.stringify({
      active: true,
      expires_at: Date.now() + 60000,
      account: { username: "fixture", displayName: "Fixture" },
    }),
  );
});
await new Promise((resolve) => provider.listen(0, "127.0.0.1", resolve));
const reserve = createServer();
await new Promise((resolve) => reserve.listen(0, "127.0.0.1", resolve));
const port = reserve.address().port;
await new Promise((resolve) => reserve.close(resolve));
const base = "http://127.0.0.1:" + port,
  auth = { Authorization: "Bearer wc1.fixture" };
const child = spawn(process.execPath, [app + "/server.mjs"], {
  cwd: app,
  stdio: ["ignore", "ignore", "pipe"],
  env: {
    ...process.env,
    PORT: String(port),
    MITCH_AUTH_ORIGIN: "http://127.0.0.1:" + provider.address().port,
  },
});
let errors = "";
child.stderr.on("data", (data) => (errors += data));
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
let ws;
try {
  for (let i = 0; i < 100; i++) {
    try {
      if ((await fetch(base + "/api/state", { headers: auth })).ok) break;
    } catch {}
    if (i === 99) throw new Error("Startup failed: " + errors);
    await pause(50);
  }
  assert.equal((await fetch(base + "/api/state")).status, 401);
  const root = await fetch(base + "/?pin=123456", { redirect: "manual" });
  assert.equal(root.status, 303);
  assert.equal(
    root.headers.get("location"),
    "https://mitchdog.com/blooket-bot/?pin=123456",
  );
  const document = await fetch(base + "/", { headers: auth });
  assert.equal(document.status, 200);
  assert((await document.text()).includes('id="btn-answering"'));
  const post = (path, body) =>
    fetch(base + path, {
      method: "POST",
      headers: { ...auth, "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  let response = await post("/api/fleet/settings", {
    autoAnswer: true,
    accuracy: "bad",
  });
  assert.equal(response.status, 400);
  assert.equal(
    (await (await fetch(base + "/api/state", { headers: auth })).json())
      .autoAnswer,
    false,
  );
  response = await post("/api/fleet/settings", {
    accuracy: 91,
    answerDelayMinMs: 0,
    answerDelayMaxMs: 0,
  });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).state.accuracyRate, 91);
  for (const route of ["/api/start", "/api/launch", "/api/fleet/launch"])
    assert.equal((await post(route, { pin: "invalid" })).status, 400);
  assert.equal((await post("/api/bot/retry", { id: "missing" })).status, 400);
  assert.equal(
    (await fetch(base + "/api/fleet/settings", { headers: auth })).status,
    405,
  );
  assert.equal(
    (await fetch(base + "/api/unknown", { headers: auth })).status,
    404,
  );
  // The service's own WebSocket dependency supplies the fixture bearer header;
  // the account provider accepts only that exact token and stays on loopback.
  const { WebSocket } = await import(
    app + "/node_modules/ws/wrapper.mjs"
  ).catch(() => import("ws"));
  ws = new WebSocket(base.replace("http:", "ws:") + "/socket", {
    headers: auth,
  });
  const messages = [];
  ws.on("message", (raw) => messages.push(JSON.parse(String(raw))));
  await new Promise((resolve, reject) => {
    ws.once("open", resolve);
    ws.once("error", reject);
  });
  ws.send(JSON.stringify({ op: "settings", autoAnswer: true }));
  for (
    let i = 0;
    i < 100 &&
    !messages.some(
      (message) =>
        message.type === "command-result" && message.op === "settings",
    );
    i++
  )
    await pause(10);
  assert(
    messages.some(
      (message) =>
        message.type === "state" && message.state.autoAnswer === true,
    ),
  );
  ws.send(JSON.stringify({ op: "settings", autoAnswer: "false" }));
  for (
    let i = 0;
    i < 100 && !messages.some((message) => message.type === "error");
    i++
  )
    await pause(10);
  assert(
    messages.some(
      (message) =>
        message.type === "error" && message.message.includes("true or false"),
    ),
  );
  assert.equal(
    (await (await fetch(base + "/api/state", { headers: auth })).json())
      .autoAnswer,
    true,
  );
  assert.equal((await post("/api/stop", {})).status, 200);
  assert.equal(
    (await (await fetch(base + "/api/state", { headers: auth })).json())
      .autoAnswer,
    false,
  );
  console.log(
    "PASS: built app HTTP/WebSocket commands, shared settings and validation, account gate, canonical redirect, command acknowledgements, unknown routes, and stop cleanup. Loopback fixtures only.",
  );
} finally {
  ws?.terminate();
  if (child.exitCode === null) {
    child.kill("SIGTERM");
    await new Promise((resolve) => child.once("exit", resolve));
  }
  provider.closeAllConnections();
  await new Promise((resolve) => provider.close(resolve));
}
