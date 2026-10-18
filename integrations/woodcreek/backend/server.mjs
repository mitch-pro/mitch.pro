import http from "node:http";
import { createAccountAuth } from "./woodcreek-auth.mjs";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer, WebSocket } from "ws";
import { BlooketInstanceManager } from "./blooket-manager.mjs";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const PUBLIC_DIR = path.resolve(__dirname, "public");
const PORT = Number(process.env.PORT || 8093);
const HOST = "0.0.0.0";
const accountAuth = createAccountAuth();

const MIME_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".ico": "image/x-icon"
};

const manager = new BlooketInstanceManager();
const clients = new Set();

function broadcastJson(payload) {
  const msg = JSON.stringify(payload);
  for (const client of clients) {
    if (client.readyState === WebSocket.OPEN) {
      try {
        client.send(msg);
      } catch {}
    }
  }
}

manager.on("state-change", (state) => {
  broadcastJson({ type: "state", state });
});

manager.on("log", (log) => {
  broadcastJson({ type: "log", log });
});

// Periodic telemetry broadcast every 1.5s
setInterval(() => {
  if (clients.size > 0) {
    broadcastJson({ type: "state", state: manager.getState() });
  }
}, 1500);

async function readBody(req) {
  return new Promise((resolve, reject) => {
    const chunks = []; let size = 0, failed = false;
    const fail = (status, message) => { if (!failed) { failed = true; chunks.length = 0; reject(Object.assign(new Error(message), {status})); } };
    req.on("data", chunk => {
      size += chunk.length;
      if (size > 262144) { fail(413, "Request is too large."); return; }
      if (!failed) chunks.push(chunk);
    });
    req.on("end", () => {
      if (failed) return;
      try {
        const text = Buffer.concat(chunks).toString("utf8");
        const body = text ? JSON.parse(text) : {};
        if (!body || typeof body !== "object" || Array.isArray(body)) throw new Error("Invalid request.");
        resolve(body);
      } catch { fail(400, "Send a JSON object."); }
    });
    req.on("error", () => fail(400, "Request could not be read."));
    req.on("aborted", () => fail(400, "Request was interrupted."));
  });
}

function sendJson(res, statusCode, data) {
  const body = Buffer.from(JSON.stringify(data));
  res.writeHead(statusCode, {
    "Content-Type": "application/json; charset=utf-8",
    "Content-Length": body.length,
    "Cache-Control": "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0",
    "Pragma": "no-cache"
  });
  res.end(body);
}

const server = http.createServer(async (req, res) => {
  try {
    const url = new URL(req.url, `http://${req.headers.host || "localhost"}`);
    const pathname = url.pathname;
    if (await accountAuth.handle(req, res, url)) return;

    if (pathname === "/api/state") {
      return sendJson(res, 200, manager.getState());
    }

    if (pathname === "/api/bot/screenshot") {
      const id = url.searchParams.get("id") || [...manager.instances.keys()][0];
      const inst = manager.instances.get(id);
      if (!inst || !inst.page || inst.page.isClosed()) {
        return sendJson(res, 404, { error: "Bot page not active" });
      }
      try {
        const buf = await inst.page.screenshot();
        res.writeHead(200, {
          "Content-Type": "image/png",
          "Content-Length": buf.length,
          "Cache-Control": "no-store, no-cache"
        });
        return res.end(buf);
      } catch (err) {
        return sendJson(res, 500, { error: err.message });
      }
    }

    if (pathname === "/api/preview-names") {
      const count = Number(url.searchParams.get("count")) || 5;
      return sendJson(res, 200, { preview: manager.getPreviewNames(count) });
    }

    if ((pathname === "/api/fleet/launch" || pathname === "/api/start" || pathname === "/api/launch") && req.method === "POST") {
      const body = await readBody(req);
      try {
        body.pin = manager.validateStart(body);
        manager.startFleet(body).catch((err) => {
          manager.log(`Could not join game: ${err.message}`, "error");
          broadcastJson({type:"error", message:err.message});
        });
        return sendJson(res, 200, { success: true });
      } catch (err) {
        return sendJson(res, 400, { error: err.message });
      }
    }

    if ((pathname === "/api/fleet/stop" || pathname === "/api/stop-all" || pathname === "/api/stop") && req.method === "POST") {
      await manager.stopFleet();
      return sendJson(res, 200, { success: true });
    }

    if (pathname === "/api/bot/retry" && req.method === "POST") {
      const body = await readBody(req);
      if (body.id) {
        await manager.retryBot(body.id, Boolean(body.newName));
        return sendJson(res, 200, { success: true });
      }
      return sendJson(res, 400, { error: "Missing bot id." });
    }

    if (pathname === "/api/bot/retry-all" && req.method === "POST") {
      const body = await readBody(req);
      await manager.retryAllErrors(Boolean(body.useNewNames));
      return sendJson(res, 200, { success: true });
    }

    if (pathname === "/api/fleet/force-answer" && req.method === "POST") {
      await manager.forceAnswerAll();
      return sendJson(res, 200, { success: true });
    }

    if (pathname === "/api/fleet/shuffle-blooks" && req.method === "POST") {
      await manager.shuffleLobbyBlooks();
      return sendJson(res, 200, { success: true });
    }

    if (pathname === "/api/fleet/trim-to-one" && req.method === "POST") {
      const result = await manager.trimToOneBot();
      return sendJson(res, 200, result);
    }

    if (pathname === "/api/fleet/inject" && req.method === "POST") {
      const body = await readBody(req);
      let scriptCode = body.scriptCode || "";
      const scriptName = body.scriptName || "Custom Cheat";
      const fileName = body.fileName;

      if (!scriptCode && fileName) {
        try {
          const safeRel = path.normalize(fileName).replace(/^(\.\.[\/\\])+/, "");
          const fullPath = path.join(PUBLIC_DIR, "cheats", safeRel);
          scriptCode = await fs.readFile(fullPath, "utf8");
        } catch (err) {
          return sendJson(res, 404, { error: `Cheat file not found: ${fileName}` });
        }
      }

      if (!scriptCode) {
        return sendJson(res, 400, { error: "No scriptCode or valid fileName provided" });
      }

      const result = await manager.injectScript(scriptCode, scriptName);
      return sendJson(res, 200, result);
    }

    if (pathname === "/bookmarklets" || pathname === "/cheats/bookmarklets") {
      const bmPath = path.join(PUBLIC_DIR, "bookmarklets.html");
      const content = await fs.readFile(bmPath);
      res.writeHead(200, {
        "Content-Type": "text/html; charset=utf-8",
        "Content-Length": content.length
      });
      return res.end(content);
    }

    if (pathname === "/api/cheats/catalog") {
      const catPath = path.join(PUBLIC_DIR, "cheats", "catalog.json");
      const content = await fs.readFile(catPath);
      res.writeHead(200, {
        "Content-Type": "application/json; charset=utf-8",
        "Content-Length": content.length
      });
      return res.end(content);
    }

    if (pathname === "/api/fleet/settings" && req.method === "POST") {
      const body = await readBody(req);
      manager.updateSettings(body);
      return sendJson(res, 200, { success: true, state: manager.getState() });
    }

    if (pathname === "/api/flaresolverr/status") {
      return sendJson(res, 200, manager.flaresolverr.getStatus());
    }

    if (pathname === "/api/flaresolverr/refresh" && req.method === "POST") {
      const session = await manager.flaresolverr.getClearance({ forceRefresh: true });
      return sendJson(res, 200, { success: Boolean(session), flaresolverr: manager.flaresolverr.getStatus() });
    }

    // Static assets with strict no-cache
    let filePath = path.join(PUBLIC_DIR, pathname === "/" ? "index.html" : pathname);
    try {
      const stat = await fs.stat(filePath);
      if (stat.isDirectory()) {
        filePath = path.join(filePath, "index.html");
      }
      const content = await fs.readFile(filePath);
      const ext = path.extname(filePath).toLowerCase();
      const contentType = MIME_TYPES[ext] || "application/octet-stream";
      res.writeHead(200, {
        "Content-Type": contentType,
        "Content-Length": content.length,
        "Cache-Control": "no-store, no-cache, must-revalidate, proxy-revalidate, max-age=0",
        "Pragma": "no-cache"
      });
      res.end(content);
    } catch {
      res.writeHead(404, { "Content-Type": "text/plain" });
      res.end("Not Found");
    }
  } catch (err) {
    if (!res.headersSent) sendJson(res, err.status || 500, { error: err.status ? err.message : "Internal server error" });
  }
});

const wss = new WebSocketServer({ noServer: true, maxPayload: 262144 });
const heartbeat = setInterval(() => {
  for (const socket of wss.clients) {
    if (!socket.isAlive || socket.bufferedAmount > 1048576) { socket.terminate(); continue; }
    socket.isAlive = false;
    if (socket.readyState === WebSocket.OPEN) socket.ping();
  }
}, 30000);
heartbeat.unref();

wss.on("connection", (socket, request) => {
  const authCheck = setInterval(async () => {
    if (!await accountAuth.authenticate(request)) socket.close(1008, "Sign in again");
  }, 15000);
  socket.isAlive = true;
  socket.on("pong", () => { socket.isAlive = true; });
  clients.add(socket);
  try {
    socket.send(JSON.stringify({ type: "state", state: manager.getState() }));
  } catch {}

  socket.on("message", async (data, isBinary) => {
    if (isBinary) return;
    try {
      if (!await accountAuth.authenticate(request)) { socket.close(1008, "Sign in again"); return; }
      const payload = JSON.parse(data.toString("utf8"));
      if (!payload || typeof payload !== "object" || Array.isArray(payload)) throw new Error("Invalid command.");
      const op = payload.op;

      if (op === "ping") {
        socket.send(JSON.stringify({ type: "pong", id: payload.id, state: manager.getState() }));
        return;
      }
      if (op === "launch" || op === "start") {
        payload.pin = manager.validateStart(payload);
        manager.startFleet(payload).catch((err) => {
          manager.log(`Could not join game: ${err.message}`, "error");
          broadcastJson({type:"error", message:err.message});
        });
        return;
      }
      if (op === "stop-all" || op === "stop") {
        await manager.stopFleet();
        return;
      }
      if (op === "stop-bot") {
        if (payload.id) await manager.stopBot(payload.id);
        return;
      }
      if (op === "retry-bot") {
        if (payload.id) await manager.retryBot(payload.id, Boolean(payload.newName));
        return;
      }
      if (op === "retry-all-errors") {
        await manager.retryAllErrors(Boolean(payload.useNewNames));
        return;
      }
      if (op === "force-answer") {
        await manager.forceAnswerAll();
        return;
      }
      if (op === "shuffle-blooks") {
        await manager.shuffleLobbyBlooks();
        return;
      }
      if (op === "inject-script") {
        let scriptCode = payload.scriptCode || "";
        const scriptName = payload.scriptName || "Cheat Injection";
        const fileName = payload.fileName;
        if (!scriptCode && fileName) {
          try {
            const safeRel = path.normalize(fileName).replace(/^(\.\.[\/\\])+/, "");
            const fullPath = path.join(PUBLIC_DIR, "cheats", safeRel);
            scriptCode = await fs.readFile(fullPath, "utf8");
          } catch {}
        }
        if (scriptCode) {
          await manager.injectScript(scriptCode, scriptName);
        }
        return;
      }

      if (op === "settings") {
        manager.updateSettings(payload);
        return;
      }
      if (op === "flaresolverr-refresh") {
        await manager.flaresolverr.getClearance({ forceRefresh: true });
        manager.emit("state-change", manager.getState());
        return;
      }
      throw new Error("Unknown command.");
    } catch (err) {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify({type:"error",message:err?.message || "Command failed. Please try again."}));
    }
  });

  socket.on("close", () => {
    clearInterval(authCheck);
    clients.delete(socket);
  });
  socket.on("error", () => {
    clearInterval(authCheck);
    clients.delete(socket);
  });
});

server.on("upgrade", async (request, socket, head) => {
  const url = new URL(request.url, `http://${request.headers.host || "localhost"}`);
  if (url.pathname === "/socket") {
    if (!accountAuth.socketAllowed(request) || !await accountAuth.authenticate(request)) {
      socket.end("HTTP/1.1 401 Unauthorized\r\nConnection: close\r\n\r\n"); return;
    }
    if (socket.destroyed) return;
    wss.handleUpgrade(request, socket, head, (ws) => {
      wss.emit("connection", ws, request);
    });
  } else {
    socket.destroy();
  }
});

server.listen(PORT, HOST, () => {
  console.log(`Tylerium Services running on http://${HOST}:${PORT}`);
});

function gracefulExit() {
  console.log("Shutting down Tylerium Services...");
  try {
    manager.stopFleet().catch(() => {});
  } catch {}
  setTimeout(() => process.exit(0), 100);
}

process.on("SIGTERM", gracefulExit);
process.on("SIGINT", gracefulExit);

process.on("uncaughtException", (err) => {
  console.error("[Server Guard] Intercepted uncaughtException:", err?.message || err);
});

process.on("unhandledRejection", (reason) => {
  console.error("[Server Guard] Intercepted unhandledRejection:", reason?.message || reason);
});
