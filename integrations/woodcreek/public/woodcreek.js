(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const base = document.documentElement.dataset.appBase || "";
  let requiresSignIn = false;
  let account = null,
    state = {},
    socket = null,
    connecting = false,
    disposed = false;
  let retryTimer,
    heartbeat,
    connectDeadline,
    retryDelay = 1000,
    lastMessage = 0;
  let selectedBot = null,
    screenshotUrl = null,
    scripts = [];
  const busy = new Set(),
    logs = new Map(),
    dirty = new Set();
  const paths = {
    launch: "/api/fleet/launch",
    settings: "/api/fleet/settings",
    "stop-all": "/api/fleet/stop",
    "stop-bot": "/api/bot/stop",
    "retry-bot": "/api/bot/retry",
    "retry-all-errors": "/api/bot/retry-all",
    "force-answer": "/api/fleet/force-answer",
    "shuffle-blooks": "/api/fleet/shuffle-blooks",
    "trim-to-one": "/api/fleet/trim-to-one",
    "inject-script": "/api/fleet/inject",
  };
  const fields = {
    answerSpeedMode: "select-speed",
    gameMode: "select-mode",
    chestStrategy: "chest-strategy",
    swapTarget: "swap-target",
    cryptoTarget: "crypto-target",
    brawlStat: "brawl-stat",
    autoMinigames: "auto-minigames",
    humanKeystrokes: "human-keystrokes",
    answerDelayMinMs: "min-delay",
    answerDelayMaxMs: "max-delay",
    accuracyRate: "range-accuracy",
  };
  const node = (tag, text, className) => {
    const result = document.createElement(tag);
    if (text !== undefined) result.textContent = text;
    if (className) result.className = className;
    return result;
  };
  function error(message) {
    $("connection-error-text").textContent = message;
    $("connection-error").hidden = false;
  }
  function signedOut(
    message = "Sign in with your Mitch account to use Blooket.",
  ) {
    requiresSignIn = true;
    socket?.close();
    account = null;
    $("account-notice").hidden = false;
    $("account-notice-text").textContent = message;
    $("account-menu").hidden = true;
    $("account-signin").hidden = false;
    status("Sign in required");
    controls();
  }
  function status(text, connected = false) {
    $("status-text").textContent = text;
    $("status-text").dataset.connected = String(connected);
  }
  async function request(path, options = {}) {
    const response = await fetch(base + path, {
      credentials: "same-origin",
      redirect: "error",
      cache: "no-store",
      signal: AbortSignal.timeout(12000),
      ...options,
      headers: {
        "X-Mitch-Requested-With": "1",
        "X-Tylerium-Requested-With": "1",
        ...options.headers,
      },
    });
    if (response.status === 401) {
      signedOut();
      throw new Error("Sign in again to continue.");
    }
    if (!response.ok) {
      const data = await response.json().catch(() => ({}));
      throw new Error(
        data.error || data.message || `Request failed (${response.status}).`,
      );
    }
    return response;
  }
  async function refresh() {
    render(await (await request("/api/state")).json());
  }

  // HTTP is the acknowledged command channel. The WebSocket carries shared state
  // only; a dropped socket cannot swallow a click or leave a button disabled.
  async function command(payload, button) {
    const key = button?.id || payload.op;
    if (busy.has(key)) return;
    busy.add(key);
    if (button) button.disabled = true;
    $("connection-error").hidden = true;
    try {
      const result = await (
        await request(paths[payload.op], {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(payload),
        })
      ).json();
      if (result.success === false)
        throw new Error(
          result.error || result.message || "Command could not finish.",
        );
      if (result.state) render(result.state);
      else await refresh();
      return result;
    } catch (cause) {
      error(
        cause.name === "TimeoutError"
          ? "The command was not confirmed. Check the session before trying again."
          : cause.message,
      );
      throw cause;
    } finally {
      busy.delete(key);
      if (button) button.disabled = false;
      controls();
    }
  }
  const run = (payload, button) => command(payload, button).catch(() => {});
  let settingsQueue = Promise.resolve(),
    accuracyTimer;
  function setting(patch) {
    const keys = Object.keys(patch).map((key) =>
      key === "accuracy" ? "accuracyRate" : key,
    );
    keys.forEach((key) => dirty.add(key));
    settingsQueue = settingsQueue
      .then(() => command({ op: "settings", ...patch }))
      .catch(() => {})
      .finally(() => {
        keys.forEach((key) => dirty.delete(key));
        render(state);
      });
  }
  const connectedBots = () =>
    (state.instances || []).filter(
      (bot) => !["error", "ended"].includes(bot.status),
    );
  function controls() {
    $("btn-answering").disabled =
      !account || busy.has("btn-answering") || Boolean(state.stopping);
    $("btn-launch").disabled =
      !account ||
      busy.has("btn-launch") ||
      Boolean(state.starting || state.stopping);
    $("btn-force-answer").disabled =
      !account ||
      !connectedBots().some((bot) => ["lobby", "game"].includes(bot.status)) ||
      busy.has("btn-force-answer");
    $("btn-stop-all").disabled =
      !account ||
      busy.has("btn-stop-all") ||
      !(state.instances?.length || state.starting);
    $("btn-tool-master-solver").disabled =
      !account || !connectedBots().length || busy.has("btn-tool-master-solver");
    $("btn-shuffle-blooks").disabled =
      !account ||
      busy.has("btn-shuffle-blooks") ||
      !(state.instances || []).some((bot) => bot.status === "lobby");
    $("btn-trim-to-one").disabled =
      !account || busy.has("btn-trim-to-one") || connectedBots().length <= 1;
    $("btn-retry-all").disabled =
      !account ||
      busy.has("btn-retry-all") ||
      !(state.instances || []).some((bot) => bot.status === "error");
  }
  function addLogs(entries) {
    for (const entry of entries || [])
      logs.set(
        entry.id || `${entry.timestamp}:${entry.botName}:${entry.message}`,
        entry,
      );
    while (logs.size > 100) logs.delete(logs.keys().next().value);
    $("activity-log").replaceChildren(
      ...[...logs.values()].reverse().map((entry) => {
        const li = node("li");
        li.dataset.type = entry.type;
        const date = new Date(entry.timestamp),
          time = node(
            "time",
            Number.isNaN(date.getTime()) ? "" : date.toLocaleTimeString(),
          );
        li.append(
          time,
          node(
            "span",
            (entry.botName ? entry.botName + ": " : "") + entry.message,
          ),
        );
        return li;
      }),
    );
  }
  function action(text, payload) {
    const button = node("button", text);
    button.type = "button";
    button.addEventListener("click", () => run(payload, button));
    return button;
  }
  function render(next) {
    if (
      next.sessionId &&
      next.sessionId === state.sessionId &&
      next.revision < state.revision
    )
      return;
    if (next.sessionId && next.sessionId !== state.sessionId) logs.clear();
    state = next;
    $("btn-answering").textContent = state.autoAnswer
      ? "Stop answering"
      : "Start answering";
    $("btn-answering").setAttribute(
      "aria-pressed",
      String(Boolean(state.autoAnswer)),
    );
    $("answering-status").textContent = state.autoAnswer
      ? state.inGameCount ||
        connectedBots().some((bot) => bot.status === "game")
        ? "Answering enabled"
        : "Ready when the game starts"
      : "Paused";
    for (const [key, id] of Object.entries(fields)) {
      const field = $(id);
      if (
        state[key] !== undefined &&
        document.activeElement !== field &&
        !dirty.has(key)
      ) {
        if (field.type === "checkbox") field.checked = Boolean(state[key]);
        else field.value = state[key];
      }
    }
    $("accuracy-badge-box").textContent = (state.accuracyRate ?? 95) + "%";
    if (document.activeElement !== $("range-accuracy"))
      $("accuracy-output").textContent = (state.accuracyRate ?? 95) + "%";
    $("question-count").textContent = (state.totalQuestions || 0) + " answers";
    $("active-count").textContent = connectedBots().length;
    $("session-empty").hidden = Boolean(state.instances?.length);
    // Preserve row buttons and keyboard focus across periodic state broadcasts.
    const container = $("bot-list"),
      retained = new Set();
    for (const bot of state.instances || []) {
      retained.add(bot.id);
      let row = [...container.children].find(
        (row) => row.dataset.id === bot.id,
      );
      if (!row) {
        row = node("article", undefined, "bot-row");
        row.dataset.id = bot.id;
        const identity = node("div");
        identity.append(node("strong"), node("small", "", "bot-answers"));
        const connection = node("div");
        connection.append(
          node("small", "", "bot-state"),
          node("small", "", "bot-answer-status"),
        );
        const actions = node("div", undefined, "actions");
        const details = node("button", "Details");
        details.type = "button";
        details.addEventListener("click", () => openBot(bot.id));
        actions.append(
          details,
          action("Retry", { op: "retry-bot", id: bot.id }),
          action("Stop", { op: "stop-bot", id: bot.id }),
        );
        row.append(identity, connection, actions);
        container.append(row);
      }
      row.dataset.status = bot.status;
      row.querySelector("strong").textContent = bot.name;
      row.querySelector(".bot-answers").textContent =
        (bot.questionsAnswered || 0) + " answers";
      row.querySelector(".bot-state").textContent =
        bot.statusText || bot.status;
      row.querySelector(".bot-answer-status").textContent =
        bot.answerStatus ||
        (state.autoAnswer ? "Waiting for game controls" : "Paused");
      const retry = row.querySelectorAll("button")[1];
      retry.hidden = bot.status !== "error";
    }
    for (const row of [...container.children])
      if (!retained.has(row.dataset.id)) row.remove();
    if (selectedBot && $("bot-dialog").open) renderBot();
    addLogs(state.logs);
    controls();
  }
  async function loadAccount() {
    const data = await (await request("/api/account")).json();
    if (!data.account) {
      signedOut();
      return false;
    }
    requiresSignIn = false;
    account = data.account;
    $("account-notice").hidden = true;
    $("account-signin").hidden = true;
    $("account-menu").hidden = false;
    const name = account.displayName || account.username || "Account";
    $("account-name").textContent = name;
    $("account-name").dataset.color = account.activeColor || "";
    $("account-menu").querySelector("summary").dataset.effect =
      account.activeProfileEffect || "";
    $("account-initial").textContent = Array.from(name)[0].toUpperCase();
    const badges = {
      verified_badge: ["✔", "Verified Badge"],
      og_badge: ["📜", "OG Badge"],
      artist_badge: ["🎨", "Artist Badge"],
      chess_badge: ["♟", "Chess Badge"],
      builder_badge: ["🛠", "Builder Badge"],
      lucky_badge: ["🍀", "Lucky Badge"],
      premium_star_badge: ["⭐", "Premium Star Badge"],
      owner_fan_badge: ["👑", "Mitch Fan Badge"],
    };
    const badge = badges[account.activeBadge];
    $("account-badge").hidden = !badge;
    $("account-badge").textContent = badge?.[0] || "";
    $("account-badge").title = badge?.[1] || "";
    const photo = $("account-photo");
    photo.hidden = true;
    $("account-initial").hidden = false;
    if (account.pfp) {
      const url = new URL(account.pfp, "https://mitchdog.com/");
      if (["http:", "https:"].includes(url.protocol)) {
        photo.src = url.href;
        photo.hidden = false;
        $("account-initial").hidden = true;
      }
      photo.onerror = () => {
        photo.hidden = true;
        $("account-initial").hidden = false;
      };
    }
    controls();
    return true;
  }
  function reconnect() {
    if (disposed || retryTimer || requiresSignIn) return;
    retryTimer = setTimeout(() => {
      retryTimer = null;
      connect();
    }, retryDelay);
    retryDelay = Math.min(15000, retryDelay * 2);
  }
  async function connect() {
    if (disposed || connecting || (socket && socket.readyState < 2)) return;
    connecting = true;
    status("Connecting");
    try {
      if (!(await loadAccount())) return;
      await refresh();
      const ws = new WebSocket(
        `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}${base}/socket`,
      );
      socket = ws;
      connectDeadline = setTimeout(() => {
        if (ws.readyState === WebSocket.CONNECTING) ws.close();
      }, 10000);
      ws.onopen = () => {
        clearTimeout(connectDeadline);
        retryDelay = 1000;
        lastMessage = Date.now();
        status("Connected", true);
        heartbeat = setInterval(() => {
          if (Date.now() - lastMessage > 45000) {
            ws.close();
            return;
          }
          if (ws.readyState === WebSocket.OPEN)
            ws.send(JSON.stringify({ op: "ping", id: Date.now() }));
        }, 15000);
      };
      ws.onmessage = (event) => {
        if (socket !== ws) return;
        lastMessage = Date.now();
        try {
          const message = JSON.parse(event.data);
          if (
            (message.type === "state" || message.type === "pong") &&
            message.state
          )
            render(message.state);
          else if (message.type === "log") addLogs([message.log]);
          else if (message.type === "error")
            error(message.message || "Command failed.");
        } catch {
          error("An invalid session update was received. Reconnecting.");
          ws.close();
        }
      };
      ws.onerror = () => ws.close();
      ws.onclose = (event) => {
        if (socket !== ws) return;
        clearTimeout(connectDeadline);
        clearInterval(heartbeat);
        socket = null;
        if (event.code === 1008) {
          signedOut("Your session expired. Sign in again.");
          return;
        }
        status("Reconnecting");
        reconnect();
      };
    } catch (cause) {
      status(account ? "Reconnecting" : "Sign in unavailable");
      error(cause.message);
      reconnect();
    } finally {
      connecting = false;
    }
  }
  async function names() {
    try {
      const data = await (
        await request(
          "/api/preview-names?count=" + Number($("input-count").value),
        )
      ).json();
      const entries = Array.isArray(data.preview)
        ? data.preview.map((value) =>
            typeof value === "string" ? value : value.name,
          )
        : [];
      $("bulk-editor-textarea").value =
        entries.filter(Boolean).join("\n") ||
        Array.from(
          { length: Number($("input-count").value) || 5 },
          (_, index) => "Player_" + (index + 1),
        ).join("\n");
    } catch (cause) {
      error(cause.message);
    }
  }
  $("fleet-form").addEventListener("submit", (event) => {
    event.preventDefault();
    const pin = $("input-pin").value.replace(/[\s-]/g, "");
    if (!/^\d{6,9}$/.test(pin)) {
      error("Enter a Game PIN with 6–9 digits.");
      return;
    }
    const count = Number($("input-count").value),
      roster = $("bulk-editor-textarea")
        .value.split("\n")
        .map((name) => name.trim().slice(0, 16));
    if (!Number.isInteger(count) || count < 1 || count > 50) {
      error("Choose between 1 and 50 bots.");
      return;
    }
    run(
      {
        op: "launch",
        pin,
        count,
        names: roster.slice(0, count),
        botBlooks: Array(count).fill($("select-blook").value),
      },
      $("btn-launch"),
    );
  });
  $("btn-answering").addEventListener("click", () =>
    run({ op: "settings", autoAnswer: !state.autoAnswer }, $("btn-answering")),
  );
  $("btn-force-answer").addEventListener("click", () =>
    run({ op: "force-answer" }, $("btn-force-answer")),
  );
  $("btn-stop-all").addEventListener("click", () =>
    run({ op: "stop-all" }, $("btn-stop-all")),
  );
  $("btn-retry-all").addEventListener("click", () =>
    run({ op: "retry-all-errors" }, $("btn-retry-all")),
  );
  $("btn-shuffle-blooks").addEventListener("click", () =>
    run({ op: "shuffle-blooks" }, $("btn-shuffle-blooks")),
  );
  $("btn-randomize-roster").addEventListener("click", names);
  $("input-count").addEventListener("change", () => {
    $("roster-summary").textContent =
      "Names beyond the selected count will not join.";
  });
  $("dismiss-connection-error").addEventListener("click", () => {
    $("connection-error").hidden = true;
  });
  $("settings-form").addEventListener("submit", (event) =>
    event.preventDefault(),
  );
  for (const field of document.querySelectorAll("[data-setting]"))
    field.addEventListener("change", () => {
      const key = field.dataset.setting;
      if (key === "answerSpeedMode") {
        const delays = {
          instant: [0, 0],
          fast: [500, 900],
          human: [1200, 2800],
          casual: [3000, 5000],
        }[field.value];
        setting({
          answerSpeedMode: field.value,
          ...(delays
            ? { answerDelayMinMs: delays[0], answerDelayMaxMs: delays[1] }
            : {}),
        });
      } else
        setting({
          [key]: field.type === "checkbox" ? field.checked : field.value,
        });
    });
  $("range-accuracy").addEventListener("input", () => {
    $("accuracy-output").textContent = $("range-accuracy").value + "%";
    clearTimeout(accuracyTimer);
    accuracyTimer = setTimeout(
      () => setting({ accuracy: Number($("range-accuracy").value) }),
      150,
    );
  });
  for (const id of ["min-delay", "max-delay"])
    $(id).addEventListener("change", () => {
      const min = Number($("min-delay").value),
        max = Number($("max-delay").value);
      if (min < 0 || max > 15000 || min > max) {
        error("Use delays between 0 and 15000 ms, with the minimum first.");
        return;
      }
      setting({
        answerSpeedMode: "custom",
        answerDelayMinMs: min,
        answerDelayMaxMs: max,
      });
    });
  document.querySelectorAll(".site-nav a").forEach((link) =>
    link.addEventListener("click", () => {
      document
        .querySelector(".site-nav .is-current")
        ?.classList.remove("is-current");
      link.classList.add("is-current");
      const target = document.querySelector(link.hash);
      if (target?.tagName === "DETAILS") target.open = true;
    }),
  );
  document
    .querySelectorAll("[data-close]")
    .forEach((button) =>
      button.addEventListener("click", () => button.closest("dialog").close()),
    );
  $("account-signout").addEventListener("click", async () => {
    try {
      const response = base
        ? await fetch("/api/logout", {
            method: "POST",
            credentials: "same-origin",
            headers: { "X-Mitch-Requested-With": "1" },
          })
        : await request("/auth/logout", { method: "POST" });
      if (!response.ok) throw new Error("Sign out could not finish.");
      location.href = base ? "/enroll/" : "/";
    } catch (cause) {
      error(cause.message);
    }
  });
  function renderBot() {
    const bot = (state.instances || []).find((bot) => bot.id === selectedBot);
    if (!bot) {
      $("bot-dialog").close();
      return;
    }
    $("bot-dialog-title").textContent = bot.name;
    $("bot-diagnostic").textContent =
      bot.diagnostic?.explanation || bot.statusText;
    $("bot-timeline").replaceChildren(
      ...(bot.timeline || []).map((event) => node("li", event.msg)),
    );
  }
  function openBot(id) {
    selectedBot = id;
    $("bot-screenshot").hidden = true;
    renderBot();
    $("bot-dialog").showModal();
  }
  $("bot-dialog").addEventListener("close", () => {
    selectedBot = null;
    if (screenshotUrl) URL.revokeObjectURL(screenshotUrl);
    screenshotUrl = null;
    $("bot-screenshot").removeAttribute("src");
  });
  $("btn-screenshot").addEventListener("click", async () => {
    try {
      const id = selectedBot,
        response = await request(
          "/api/bot/screenshot?id=" + encodeURIComponent(id),
        );
      const blob = await response.blob();
      if (selectedBot !== id || !$("bot-dialog").open) return;
      if (screenshotUrl) URL.revokeObjectURL(screenshotUrl);
      screenshotUrl = URL.createObjectURL(blob);
      $("bot-screenshot").src = screenshotUrl;
      $("bot-screenshot").hidden = false;
    } catch (cause) {
      $("bot-diagnostic").textContent = cause.message;
    }
  });
  $("btn-open-cheats").addEventListener("click", async () => {
    $("tools-dialog").showModal();
    try {
      const catalog = await (await request("/api/cheats/catalog")).json();
      scripts = Array.isArray(catalog.allCheats) ? catalog.allCheats : [];
      renderTools();
    } catch (cause) {
      $("tool-status").textContent = cause.message;
    }
  });
  $("btn-tool-master-solver").addEventListener("click", () =>
    run({ op: "settings", autoAnswer: true }, $("btn-tool-master-solver")),
  );
  $("btn-trim-to-one").addEventListener("click", () =>
    run({ op: "trim-to-one" }, $("btn-trim-to-one")),
  );
  $("tool-search").addEventListener("input", renderTools);
  function renderTools() {
    const query = $("tool-search").value.trim().toLowerCase();
    $("tool-list").replaceChildren(
      ...scripts
        .filter((script) =>
          `${script.name} ${script.categoryName}`.toLowerCase().includes(query),
        )
        .map((script) => {
          const button = node("button", script.name);
          button.append(node("small", script.categoryName));
          button.addEventListener("click", async () => {
            $("tool-status").textContent = "Running…";
            try {
              const result = await command(
                {
                  op: "inject-script",
                  fileName: script.fileName,
                  scriptName: script.name,
                },
                button,
              );
              $("tool-status").textContent =
                result?.message || "Script finished.";
            } catch (cause) {
              $("tool-status").textContent = cause.message;
            }
          });
          return button;
        }),
    );
  }
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden && !socket) connect();
  });
  window.addEventListener("pagehide", () => {
    disposed = true;
    clearTimeout(retryTimer);
    clearInterval(heartbeat);
    clearTimeout(connectDeadline);
    socket?.close();
  });
  window.addEventListener("pageshow", (event) => {
    if (event.persisted) {
      disposed = false;
      socket = null;
      connect();
    }
  });
  controls();
  connect().then(() => {
    if (account) names();
  });
})();
