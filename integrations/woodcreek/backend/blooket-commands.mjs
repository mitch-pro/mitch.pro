import fs from "node:fs/promises";
import path from "node:path";

export const HTTP_COMMANDS = Object.freeze({
  "/api/fleet/launch": "launch",
  "/api/start": "launch",
  "/api/launch": "launch",
  "/api/fleet/stop": "stop-all",
  "/api/stop-all": "stop-all",
  "/api/stop": "stop-all",
  "/api/bot/stop": "stop-bot",
  "/api/bot/retry": "retry-bot",
  "/api/bot/retry-all": "retry-all-errors",
  "/api/fleet/force-answer": "force-answer",
  "/api/fleet/shuffle-blooks": "shuffle-blooks",
  "/api/fleet/trim-to-one": "trim-to-one",
  "/api/fleet/inject": "inject-script",
  "/api/fleet/settings": "settings",
  "/api/flaresolverr/refresh": "flaresolverr-refresh",
});
export async function readCheatScript(publicDir, fileName) {
  if (typeof fileName !== "string" || !fileName || fileName.includes("\0"))
    throw new Error("Invalid script file.");
  const root = await fs.realpath(path.join(publicDir, "cheats"));
  const candidate = path.resolve(root, fileName);
  if (!candidate.startsWith(root + path.sep))
    throw new Error("Invalid script path.");
  const real = await fs.realpath(candidate);
  if (!real.startsWith(root + path.sep) || !real.endsWith(".js"))
    throw new Error("Invalid script path.");
  return fs.readFile(real, "utf8");
}

// Both transports use the same command validation, aliases, and return values.
export async function executeCommand(
  manager,
  payload,
  { publicDir, onError = () => {} } = {},
) {
  if (!payload || typeof payload !== "object" || Array.isArray(payload))
    throw new Error("Invalid command.");
  switch (payload.op) {
    case "start":
    case "launch":
      manager.validateStart(payload);
      manager.startFleet(payload).catch((error) => {
        manager.log(`Could not join game: ${error.message}`, "error");
        onError(error);
      });
      return { success: true };
    case "stop":
    case "stop-all":
      await manager.stopFleet();
      break;
    case "stop-bot":
    case "retry-bot":
      if (typeof payload.id !== "string" || !manager.instances.has(payload.id))
        throw new Error("Choose an existing bot.");
      if (payload.op === "stop-bot") await manager.stopBot(payload.id);
      else await manager.retryBot(payload.id, payload.newName ?? false);
      break;
    case "retry-all-errors":
      await manager.retryAllErrors(payload.useNewNames === true);
      break;
    case "force-answer":
      await manager.forceAnswerAll();
      break;
    case "shuffle-blooks":
      await manager.shuffleLobbyBlooks();
      break;
    case "trim-to-one":
      return manager.trimToOneBot();
    case "settings":
      manager.updateSettings(payload);
      return { success: true, state: manager.getState() };
    case "flaresolverr-refresh": {
      const session = await manager.flaresolverr.getClearance({
        forceRefresh: true,
      });
      manager.emit("state-change", manager.getState());
      return {
        success: Boolean(session),
        flaresolverr: manager.flaresolverr.getStatus(),
      };
    }
    case "inject-script": {
      const label = `${payload.scriptName || ""} ${path.basename(payload.fileName || "")}`;
      if (/auto[\s_-]*answer|master[\s_-]*solver/i.test(label)) {
        if (/once/i.test(label)) await manager.forceAnswerAll();
        else manager.updateSettings({ autoAnswer: true });
        return {
          success: true,
          state: manager.getState(),
          message: "Answering enabled.",
        };
      }
      const scriptCode =
        payload.scriptCode ||
        (await readCheatScript(publicDir, payload.fileName));
      if (typeof scriptCode !== "string" || !scriptCode.trim())
        throw new Error("Choose a script to run.");
      return manager.injectScript(
        scriptCode,
        payload.scriptName || "Custom script",
      );
    }
    default:
      throw new Error("Unknown command.");
  }
  return { success: true };
}
