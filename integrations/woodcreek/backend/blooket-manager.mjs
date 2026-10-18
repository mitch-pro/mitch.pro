import {
  DEFAULT_SETTINGS,
  MAX_BOTS,
  normalizeSettings,
  targetAccuracy,
} from "./blooket-settings.mjs";
import { readGameScreen } from "./blooket-screen.mjs";
import { playMinigame } from "./blooket-minigames.mjs";
import { JoinScheduler } from "./join-scheduler.mjs";
import {
  submitAnswer,
  advanceFeedback,
  chooseAnswer,
} from "./blooket-answers.mjs";
import { EventEmitter } from "events";
import { chromium } from "playwright";
import {
  PLAYER_URL,
  PIN_SELECTOR,
  NAME_SELECTOR,
  normalizePin,
  visibleField,
  submitJoinField,
  readJoinScreen,
} from "./blooket-connection.mjs";
import { FlareSolverrClient } from "./flaresolverr-client.mjs";

const CHROME_PATH = process.env.CHROME_PATH || chromium.executablePath();

import {
  BLOOK_TYPES,
  REAL_SHORT_NAMES,
  REAL_INITIAL_NAMES,
  WHS_NAMES,
  CASUAL_STUDENT_NAMES,
} from "./blooket-names.mjs";
export * from "./blooket-names.mjs";

export class BlooketInstanceManager extends EventEmitter {
  constructor() {
    super();
    this.browser = null;
    this.browserPromise = null;
    this.fleetGeneration = 0;
    this.starting = false;
    this.stopping = false;
    this.joinScheduler = new JoinScheduler();
    this.instances = new Map(); // id -> instance
    this.currentPin = "";
    Object.assign(this, DEFAULT_SETTINGS);
    this.baseName = "Player";
    this.customNames = [];
    this.answerDelayMs = 2000;

    this.autoAnswerGlobal = false;
    this.answerGeneration = 0;
    this.autoMinigames = true;
    this.logHistory = [];
    this.nextInstanceId = 1;
    this.totalQuestionsAnswered = 0;
    this.totalCorrectAnswers = 0;
    this.totalKnownAnswers = 0;
    this.flaresolverr = new FlareSolverrClient();
    this.latestQuestion = null;
  }

  log(message, type = "info", botName = "") {
    const entry = {
      id: `log-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      timestamp: new Date().toISOString(),
      type,
      message,
      botName,
    };
    this.logHistory.push(entry);
    if (this.logHistory.length > 250) this.logHistory.shift();
    this.emit("log", entry);
  }

  async ensureBrowser() {
    if (this.stopping)
      throw new Error(
        "The previous session is still stopping. Please try again.",
      );
    if (this.browser?.isConnected()) return this.browser;
    if (this.browserPromise) return this.browserPromise;
    this.browserPromise = this.launchBrowser();
    try {
      return await this.browserPromise;
    } finally {
      this.browserPromise = null;
    }
  }

  async launchBrowser() {
    this.log("Starting browser...", "system");
    const browser = await chromium.launch({
      executablePath: CHROME_PATH,
      headless: true,
      args: [
        "--no-sandbox",
        "--disable-setuid-sandbox",
        "--disable-dev-shm-usage",
        "--disable-gpu",
        "--mute-audio",
        "--disable-software-rasterizer",
        "--disable-background-timer-throttling",
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
        "--disable-blink-features=AutomationControlled",
        "--blink-settings=imagesEnabled=false",
        "--no-first-run",
      ],
    });
    this.browser = browser;
    browser.on("disconnected", () => {
      if (this.browser !== browser) return;
      this.browser = null;
      if (this.stopping) return;
      this.log("Browser disconnected. Retry the affected connections.", "warn");
      for (const instance of this.instances.values()) {
        if (instance.cancelled || ["error", "ended"].includes(instance.status))
          continue;
        clearTimeout(instance.pollTimer);
        instance.runVersion++;
        instance.status = "error";
        instance.statusText = "Browser disconnected. Retry to reconnect.";
      }
      this.joinScheduler.cancelInactive();
      this.emit("state-change", this.getState());
    });
    return browser;
  }

  generateUniqueRealName(usedSet = new Set()) {
    const pool = [...REAL_SHORT_NAMES];
    const offset = Math.floor(Math.random() * pool.length);
    for (let i = 0; i < pool.length; i++) {
      const candidate = pool[(offset + i) % pool.length];
      if (!usedSet.has(candidate.toLowerCase())) {
        return candidate;
      }
    }

    const initPool = [...REAL_INITIAL_NAMES];
    const initOffset = Math.floor(Math.random() * initPool.length);
    for (let i = 0; i < initPool.length; i++) {
      const candidate = initPool[(initOffset + i) % initPool.length];
      if (!usedSet.has(candidate.toLowerCase())) {
        return candidate;
      }
    }

    const initials = [
      "M",
      "S",
      "T",
      "B",
      "C",
      "W",
      "L",
      "D",
      "K",
      "R",
      "P",
      "H",
      "J",
      "F",
      "G",
      "V",
      "N",
      "Z",
    ];
    for (const first of pool) {
      for (const init of initials) {
        const candidate = `${first} ${init}`;
        if (!usedSet.has(candidate.toLowerCase())) {
          return candidate;
        }
      }
    }

    return `Player_${Math.floor(Math.random() * 899 + 100)}`;
  }

  generateName(index, total, usedSet = new Set()) {
    let pool = REAL_SHORT_NAMES;
    if (this.namingStyle === "initials") pool = REAL_INITIAL_NAMES;
    else if (this.namingStyle === "woodcreek") pool = WHS_NAMES;
    else if (this.namingStyle === "casual") pool = CASUAL_STUDENT_NAMES;

    const idx = (index - 1) % pool.length;
    let candidate =
      this.namingStyle === "pattern"
        ? this.namePattern
            .replaceAll("{n}", String(this.nameStartNumber + index - 1))
            .slice(0, 16)
        : pool[idx];
    if (usedSet.has(candidate.toLowerCase())) {
      candidate = this.generateUniqueRealName(usedSet);
    }
    return candidate.slice(0, 16);
  }

  getPreviewNames(count = 5) {
    const list = [];
    const used = new Set();
    for (let i = 1; i <= Math.min(count, 10); i++) {
      const name = this.generateName(i, count, used);
      used.add(name.toLowerCase());
      list.push(name);
    }
    return list;
  }

  assignBlook() {
    if (this.blookTheme === "wolf") return "wolf";
    if (this.blookTheme === "megabot") return "megabot";
    if (this.blookTheme === "dragon") return "dragon";
    if (this.blookTheme === "wizard") return "wizard";
    return "random";
  }

  validateStart(options = {}) {
    if (!options || typeof options !== "object" || Array.isArray(options))
      throw new Error("Invalid game settings.");
    const pin = normalizePin(options.pin);
    if (this.starting || this.stopping)
      throw new Error("Wait for the current join or stop operation to finish.");
    normalizeSettings(options, this);
    const requested = this.fleetCount(options);
    const active = [...this.instances.values()].filter(
      (instance) =>
        !instance.cancelled && !["error", "ended"].includes(instance.status),
    ).length;
    if (active + requested > 50)
      throw new Error(
        `Up to 50 bots can be connected or queued. ${50 - active} slots are available.`,
      );
    return pin;
  }

  fleetCount(options = {}) {
    for (const key of ["names", "customNames", "botBlooks"]) {
      if (
        options[key] !== undefined &&
        (!Array.isArray(options[key]) ||
          options[key].length > MAX_BOTS ||
          options[key].some((value) => typeof value !== "string"))
      )
        throw new Error(`Invalid ${key}: use up to ${MAX_BOTS} text entries.`);
    }
    const names = options.names?.length
      ? options.names
      : options.customNames || [];
    const count =
      options.count === undefined ? names.length || 5 : Number(options.count);
    if (!Number.isFinite(count) || count < 1 || count > MAX_BOTS)
      throw new Error(`Choose between 1 and ${MAX_BOTS} bots.`);
    return Math.max(Math.floor(count), names.length);
  }

  async startFleet(options = {}) {
    const pin = this.validateStart(options);
    this.starting = true;
    const generation = this.fleetGeneration;
    try {
      return await this.launchFleet({ ...options, pin }, generation);
    } finally {
      if (generation === this.fleetGeneration) {
        this.starting = false;
        this.emit("state-change", this.getState());
      }
    }
  }

  async launchFleet(options, generation) {
    const { pin, names = [], customNames = [], botBlooks = [] } = options;
    const safeCount = this.fleetCount(options);
    this.updateSettings(options);
    this.currentPin = pin;
    const cleanNames = (names.length ? names : customNames).map((name) =>
      name.trim().slice(0, 16),
    );
    this.customNames = cleanNames;
    this.log(`Joining ${safeCount} bots…`, "system");

    if (this.useFlareSolverr) {
      this.log(
        "Consulting FlareSolverr for Cloudflare clearance session & tokens...",
        "system",
      );
      try {
        const clearance = await this.flaresolverr.getClearance();
        if (clearance && clearance.cookies.length > 0) {
          this.log(
            `FlareSolverr clearance active (${clearance.cookies.length} cookies, ${clearance.latencyMs}ms).`,
            "info",
          );
        } else {
          this.log(
            "FlareSolverr unavailable; proceeding with direct browser launch.",
            "warn",
          );
        }
      } catch (err) {
        this.log(
          `FlareSolverr clearance notice: ${err.message}. Proceeding directly.`,
          "warn",
        );
      }
    }

    if (generation !== this.fleetGeneration) return;
    await this.ensureBrowser();
    if (generation !== this.fleetGeneration) return;

    // Strict deduplication pass: eliminate any duplicate/repeating names and ensure short, real names
    const usedFleetNames = new Set(
      [...this.instances.values()]
        .filter(
          (instance) =>
            !instance.cancelled &&
            !["error", "ended"].includes(instance.status) &&
            instance.pin === pin,
        )
        .map((instance) => instance.name.toLowerCase()),
    );
    const finalFleetNames = [];
    for (let i = 0; i < safeCount; i++) {
      let rawCandidate = cleanNames[i] ? cleanNames[i].trim().slice(0, 16) : "";
      if (!rawCandidate || usedFleetNames.has(rawCandidate.toLowerCase())) {
        rawCandidate = this.generateName(i + 1, safeCount, usedFleetNames);
      }
      usedFleetNames.add(rawCandidate.toLowerCase());
      finalFleetNames.push(rawCandidate);
    }

    // Reserve every name before another request can enqueue joins.
    for (let i = 1; i <= safeCount; i++) {
      if (generation !== this.fleetGeneration) return;
      const name = finalFleetNames[i - 1];
      let blook =
        Array.isArray(botBlooks) && botBlooks[i - 1]
          ? botBlooks[i - 1]
          : this.assignBlook();
      if (!BLOOK_TYPES.includes(blook) && blook !== "random") {
        blook = this.assignBlook();
      }

      const instanceId = `bot-${this.nextInstanceId++}`;

      const inst = this.createBotInstance(
        instanceId,
        name,
        blook,
        this.currentPin,
        i,
      );
      this.runBotLifecycle(inst).catch((err) =>
        this.log(`Join failed: ${err.message}`, "error", inst.name),
      );
    }

    this.emit("state-change", this.getState());
  }

  createBotInstance(id, name, blook, pin, fleetIndex = 1) {
    const startTime = Date.now();

    const individualAccuracy = targetAccuracy(this, fleetIndex);

    const instance = {
      id,
      name,
      blook,
      pin,
      fleetIndex,
      targetAccuracy: individualAccuracy,
      status: "starting",
      statusText: "Preparing connection…",
      phase: "PHASE_SANDBOX",
      questionsAnswered: 0,
      streak: 0,
      score: null,
      place: null,
      lastAnswer: "",
      startTime,
      durationMs: 0,
      pingMs: null,
      cancelled: false,
      lifecycleActive: false,
      runVersion: 0,
      diagnostic: null,
      timeline: [],
      technicalTrace: [],
      context: null,
      page: null,
      pollTimer: null,
      isAnswering: false,
      lastAnswerAttemptKey: null,
      lastAnswerAttemptTime: 0,
      knownAnswers: 0,
      correctAnswers: 0,
    };

    instance.addEvent = (msg, type = "info", phase = instance.phase) => {
      const timeOffsetMs = Date.now() - instance.startTime;
      const formattedTime = new Date().toLocaleTimeString();

      instance.timeline.push({
        time: formattedTime,
        timeOffsetMs,
        msg,
        type,
        phase,
      });
      if (instance.timeline.length > 30) instance.timeline.shift();

      instance.technicalTrace.push({
        timeOffsetMs,
        phase,
        status: type === "error" ? "FAIL" : "OK",
        msg,
      });
      if (instance.technicalTrace.length > 40) instance.technicalTrace.shift();
    };

    instance.addEvent(
      "Allocating isolated Chromium sandbox context",
      "info",
      "PHASE_SANDBOX",
    );

    this.instances.set(id, instance);
    this.emit("state-change", this.getState());

    return instance;
  }

  async runBotLifecycle(instance) {
    if (instance.cancelled || instance.lifecycleActive) return;
    instance.lifecycleActive = true;
    const version = ++instance.runVersion;
    const active = () =>
      !instance.cancelled &&
      instance.runVersion === version &&
      this.instances.get(instance.id) === instance;
    const startTime = Date.now();
    instance.status = "queued";
    instance.statusText = "Waiting to join...";
    try {
      await this.joinScheduler.run(async () => {
        for (let attempt = 0; attempt < 2 && active(); attempt++) {
          try {
            await this.executeBotJoin(instance, active);
            return;
          } catch (err) {
            if (!active()) return;
            const fatal =
              /invalid game pin|game (lobby )?is full|maximum player capacity|game session has already concluded/i.test(
                err.message,
              );
            if (attempt === 0 && !fatal) {
              instance.addEvent(
                `Connection interrupted. Retrying: ${err.message}`,
                "warn",
                instance.phase,
              );
              if (instance.context)
                await instance.context.close().catch(() => {});
              instance.context = null;
              instance.page = null;
              await new Promise((resolve) => setTimeout(resolve, 1200));
            } else {
              this.handleBotError(instance, err, startTime);
              if (instance.context)
                await instance.context.close().catch(() => {});
              instance.context = null;
              instance.page = null;
              return;
            }
          }
        }
      }, active);
    } catch (err) {
      if (active()) this.handleBotError(instance, err, startTime);
    } finally {
      instance.lifecycleActive = false;
    }
  }

  async executeBotJoin(
    instance,
    active = () =>
      !instance.cancelled && this.instances.get(instance.id) === instance,
  ) {
    const ensureActive = () => {
      if (!active()) throw new Error("Connection cancelled.");
    };
    ensureActive();
    const { pin } = instance;
    const startTime = Date.now();

    if (!this.browser || !this.browser.isConnected()) {
      await this.ensureBrowser();
    }

    instance.phase = "PHASE_SANDBOX";
    const clearance = this.useFlareSolverr
      ? this.flaresolverr.getCachedClearance()
      : null;
    const userAgent = clearance?.userAgent;

    ensureActive();
    const context = await this.browser.newContext({
      viewport: { width: 800, height: 600 },
      ...(userAgent ? { userAgent } : {}),
      serviceWorkers: "block",
      deviceScaleFactor: 1,
    });

    if (!active()) {
      await context.close().catch(() => {});
      ensureActive();
    }
    instance.context = context;
    if (clearance?.cookies?.length) {
      await context.addCookies(clearance.cookies);
      instance.addEvent(
        `Injected FlareSolverr clearance (${clearance.cookies.length} cookies, UA synchronized)`,
        "info",
        "PHASE_SANDBOX",
      );
    }

    await context.route("**/*", (route) => {
      const type = route.request().resourceType();
      const url = route.request().url();
      if (
        ["media", "font"].includes(type) ||
        url.includes("google-analytics") ||
        url.includes("googletagmanager") ||
        url.includes("doubleclick")
      ) {
        return route.abort();
      }
      return route.fallback();
    });

    instance.context = context;
    ensureActive();
    instance.page = await context.newPage();
    ensureActive();
    await instance.page.addInitScript(() => {
      try {
        Object.defineProperty(navigator, "webdriver", { get: () => undefined });
        window.chrome = { runtime: {}, loadTimes() {}, csi() {}, app: {} };
        Object.defineProperty(navigator, "plugins", {
          get: () => [1, 2, 3, 4, 5],
        });
        Object.defineProperty(navigator, "languages", {
          get: () => ["en-US", "en"],
        });
      } catch {}
      const style = document.createElement("style");
      style.textContent =
        "*, *::before, *::after { transition: none !important; animation: none !important; scroll-behavior: auto !important; }";
      if (document.documentElement) document.documentElement.appendChild(style);
      else
        document.addEventListener(
          "DOMContentLoaded",
          () => document.documentElement.appendChild(style),
          { once: true },
        );
    });
    instance.page.setDefaultTimeout(25000);
    instance.page.setDefaultNavigationTimeout(35000);

    // Phase 2: Navigation
    instance.phase = "PHASE_NAVIGATE";
    instance.status = "navigating";
    instance.statusText = "Loading Blooket join portal...";
    instance.addEvent(
      "Navigating to https://play.blooket.com/play",
      "info",
      "PHASE_NAVIGATE",
    );
    this.emit("state-change", this.getState());

    let navResponse = await instance.page.goto(PLAYER_URL, {
      waitUntil: "domcontentloaded",
      timeout: 30000,
    });

    ensureActive();
    // Check for Cloudflare challenge interstitials
    const pageTitle = await instance.page.title().catch(() => "");
    if (
      /just a moment|attention required|cloudflare|turnstile/i.test(pageTitle)
    ) {
      instance.addEvent(
        "Cloudflare challenge screen detected; requesting FlareSolverr clearance...",
        "warn",
        "PHASE_NAVIGATE",
      );
      if (this.useFlareSolverr) {
        const fresh = await this.flaresolverr.getClearance({
          forceRefresh: true,
        });
        if (fresh?.cookies?.length) {
          if (fresh.userAgent && fresh.userAgent !== userAgent)
            throw new Error("Verification refreshed. Reopening the join page.");
          await context.addCookies(fresh.cookies);
          instance.addEvent(
            "Injected refreshed FlareSolverr clearance; reloading page...",
            "info",
            "PHASE_NAVIGATE",
          );
          navResponse = await instance.page.reload({
            waitUntil: "domcontentloaded",
            timeout: 30000,
          });
        }
      }
    }

    const httpStatus = navResponse ? navResponse.status() : 200;
    if (httpStatus >= 400) {
      throw new Error(`HTTP ${httpStatus} received from play.blooket.com.`);
    }

    // Phase 3: Cookie Banner
    instance.phase = "PHASE_COOKIE";
    const cookieButtons = instance.page.locator(
      'button:has-text("Accept All"), button:has-text("Accept"), button:has-text("Got it")',
    );
    if ((await cookieButtons.count()) > 0) {
      await cookieButtons
        .first()
        .click()
        .catch(() => {});
      instance.addEvent(
        "Dismissed cookie consent banner",
        "info",
        "PHASE_COOKIE",
      );
      await instance.page.waitForTimeout(150);
    }

    // Phase 4: Submit Game PIN
    instance.phase = "PHASE_PIN_INPUT";
    instance.status = "entering_pin";
    instance.statusText = `Submitting Game PIN: ${pin}`;
    instance.addEvent(`Entering Game PIN: ${pin}`, "info", "PHASE_PIN_INPUT");
    this.emit("state-change", this.getState());

    ensureActive();
    const pinInput = visibleField(instance.page, PIN_SELECTOR);
    await pinInput.waitFor({ state: "visible", timeout: 20000 });
    await pinInput.fill(pin);
    await submitJoinField(pinInput);

    // Phase 5: PIN Verification & Wait for Nickname field
    instance.phase = "PHASE_PIN_VERIFY";
    let nameInputFound = false;
    let randomNameFound = false;
    const pinWaitStart = Date.now();

    while (Date.now() - pinWaitStart < 22000) {
      ensureActive();
      const checkRes = await readJoinScreen(instance.page).catch(() => null);

      if (checkRes) {
        if (checkRes.invalidPin) {
          throw new Error(
            "Invalid Game PIN: Blooket reported that this game could not be found.",
          );
        }
        if (checkRes.isEnded) {
          throw new Error("Game session has already concluded.");
        }
        if (checkRes.isFull) {
          throw new Error(
            "Game lobby is full (maximum player capacity reached).",
          );
        }
        if (checkRes.hasName) {
          nameInputFound = true;
          break;
        }
        if (checkRes.hasRandom) {
          randomNameFound = true;
          break;
        }
      }

      await instance.page.waitForTimeout(100);
    }

    if (!nameInputFound && !randomNameFound) {
      const lateCheck = await instance.page
        .evaluate(() => document.body.innerText)
        .catch(() => "");
      if (/invalid|not found|does not exist/i.test(lateCheck)) {
        throw new Error("Invalid Game PIN: PIN rejected by Blooket.");
      }
      throw new Error(
        "Nickname input not found. Possible PIN rejection or slow network response.",
      );
    }

    // Phase 6: Submit Nickname with Auto-Retry on duplicates/filters
    instance.phase = "PHASE_NAME_INPUT";
    instance.status = "entering_name";

    let inLobbyConfirmed = false;
    let joinedGame = false;
    let nicknameAttempts = 0;

    while (!inLobbyConfirmed && nicknameAttempts < 4) {
      ensureActive();
      nicknameAttempts++;
      instance.statusText = `Submitting nickname: ${instance.name}`;
      instance.addEvent(
        `Entering nickname "${instance.name}" (attempt ${nicknameAttempts})`,
        "info",
        "PHASE_NAME_INPUT",
      );
      this.emit("state-change", this.getState());

      if (randomNameFound) {
        const randomBtn = instance.page
          .locator(
            'button:has-text("Generate"), button:has-text("Spin"), [class*="randomButton"]',
          )
          .first();
        await randomBtn.click().catch(() => {});
        await instance.page.waitForTimeout(400);
        const joinBtn = instance.page
          .locator(
            'button:has-text("Join"), button:has-text("Let\'s Go"), form button, button[type="submit"]',
          )
          .first();
        if (await joinBtn.isVisible().catch(() => false)) {
          await joinBtn.click().catch(() => {});
        }
      } else {
        // Native Playwright input fill ensures React's synthetic input tracker registers the nickname
        const nameInput = visibleField(instance.page, NAME_SELECTOR);
        await nameInput.waitFor({ state: "visible", timeout: 8000 });
        await nameInput.fill(instance.name);
        await submitJoinField(nameInput);
      }

      // Phase 7: Strict Multi-Stage Lobby Confirmation
      instance.phase = "PHASE_NAME_VERIFY";
      const verifyStart = Date.now();

      while (Date.now() - verifyStart < 14000) {
        ensureActive();
        const check = await readJoinScreen(instance.page).catch(() => null);

        if (!check) {
          await instance.page.waitForTimeout(100);
          continue;
        }

        if (check.isFull) {
          throw new Error(
            "Game lobby is full (maximum player capacity reached).",
          );
        }
        if (check.isEnded) {
          throw new Error("Game session has already concluded.");
        }

        if (check.isTaken) {
          instance.addEvent(
            `Nickname "${instance.name}" was rejected/taken; auto-picking fresh tag...`,
            "warn",
            "PHASE_NAME_INPUT",
          );
          const usedNames = new Set(
            [...this.instances.values()].map((i) => i.name.toLowerCase()),
          );
          instance.name = this.generateUniqueRealName(usedNames);
          break; // break to outer loop to resubmit new name
        }

        // CONFIRMATION: Name input GONE, PIN input GONE, and either lobby text or blook box visible or URL is lobby
        const currentUrl = instance.page ? instance.page.url() : "";
        const isLobbyUrl = currentUrl.includes("/lobby");
        if (
          !check.hasName &&
          !check.hasPin &&
          (check.hasLobbyTxt || check.hasBlooks || check.hasGame || isLobbyUrl)
        ) {
          inLobbyConfirmed = true;
          joinedGame = check.hasGame;
          break;
        }
        await instance.page.waitForTimeout(100);
      }
    }

    if (!inLobbyConfirmed) {
      throw new Error(
        `Failed to confirm lobby entry for "${instance.name}". Nickname prompt did not advance or lobby failed to load.`,
      );
    }

    ensureActive();
    // Phase 8: In Genuine Lobby
    instance.phase = "PHASE_LOBBY";
    instance.status = joinedGame ? "game" : "lobby";
    instance.statusText = joinedGame
      ? "Game connected."
      : "Waiting for the host.";
    instance.durationMs = Date.now() - startTime;
    instance.addEvent(
      `Joined lobby as "${instance.name}"`,
      "success",
      "PHASE_LOBBY",
    );
    this.log(
      `Joined game lobby as [${instance.name}]!`,
      "success",
      instance.name,
    );
    this.emit("state-change", this.getState());

    await this.pickLobbyBlook(instance).catch(() => {});
    this.startBotLoop(instance);
  }

  handleBotError(instance, err, startTime) {
    const { name } = instance;
    instance.durationMs = Date.now() - startTime;

    let category = "SYSTEM_ERROR";
    let code = "UNKNOWN_ERROR";
    let explanation = err.message;
    let remediation = [
      "Check your home server internet connection.",
      "Verify that play.blooket.com is accessible.",
    ];

    const errMsg = err.message.toLowerCase();

    if (errMsg.includes("invalid game pin")) {
      category = "PIN_ERROR";
      code = "PIN_NOT_FOUND_OR_EXPIRED";
      explanation =
        "Blooket reported that the Game PIN does not correspond to an active game lobby.";
      remediation = [
        "Check the host's screen to make sure the game lobby is still open.",
        "Ensure no trailing spaces or misread digits (e.g. 0 vs O).",
        "If the host closed or re-hosted the game, obtain the new code.",
      ];
    } else if (
      errMsg.includes("nickname") ||
      errMsg.includes("taken") ||
      errMsg.includes("not allowed")
    ) {
      category = "NAME_ERROR";
      code = "NICKNAME_REJECTED";
      explanation = `The nickname "${name}" was rejected by Blooket because it is either already claimed or filtered by the host.`;
      remediation = [
        "Use 'Randomize All' to pick new unique names.",
        "Click 'Retry with New Name' to automatically deploy with a fresh unique tag.",
      ];
    } else if (errMsg.includes("server is at capacity")) {
      category = "SERVER_CAPACITY";
      code = "SERVER_BUSY";
      explanation = "The server has no room for another browser connection.";
      remediation = ["Stop unused bots, then retry."];
    } else if (errMsg.includes("full") || errMsg.includes("capacity")) {
      category = "CAPACITY_ERROR";
      code = "LOBBY_MAX_CAPACITY";
      explanation = "The host’s game lobby is full.";
      remediation = [
        "Ask the host to upgrade their lobby capacity or remove idle players.",
        "Reduce the number of bots being deployed from your fleet.",
      ];
    } else if (errMsg.includes("ended") || errMsg.includes("concluded")) {
      category = "SESSION_ERROR";
      code = "GAME_ALREADY_ENDED";
      explanation =
        "The host ended this game session before the bot could complete joining.";
      remediation = [
        "Wait for the host to start a new game match.",
        "Update the Game PIN in the hub when the new lobby opens.",
      ];
    } else if (errMsg.includes("timeout") || errMsg.includes("timed out")) {
      category = "NETWORK_ERROR";
      code = "CONNECTION_TIMEOUT_30S";
      explanation =
        "Network connection to play.blooket.com timed out while waiting for page response.";
      remediation = [
        "The server launch rate has been auto-throttled to keep connections smooth.",
        "Verify the server upstream connection speed.",
      ];
    } else if (
      errMsg.includes("cloudflare") ||
      errMsg.includes("challenge") ||
      errMsg.includes("captcha")
    ) {
      category = "SECURITY_BLOCK";
      code = "CLOUDFLARE_BOT_CHALLENGE";
      explanation =
        "Blooket Cloudflare edge triggered a verification challenge on the server IP.";
      remediation = [
        "Click 'Re-solve Clearance' in the FlareSolverr status bar to get fresh clearance tokens.",
        "Pause launches for 1-2 minutes to allow IP cooldown.",
      ];
    }

    instance.status = "error";
    instance.statusText = `${category}: ${code}`;
    instance.diagnostic = {
      category,
      code,
      phase: instance.phase,
      durationMs: instance.durationMs,
      error: err.message,
      explanation,
      remediation,
      capturedText: "Diagnostic event logged during join pipeline.",
      pageUrl: instance.page
        ? instance.page.url()
        : "https://play.blooket.com/play",
      timestamp: new Date().toISOString(),
    };

    instance.addEvent(`Failed: ${err.message}`, "error", instance.phase);
    this.log(`${category} (${code}): ${err.message}`, "error", name);
    this.emit("state-change", this.getState());
  }

  async retryBot(id, customNameOrBool = "") {
    const inst = this.instances.get(id);
    if (
      !inst ||
      inst.cancelled ||
      inst.lifecycleActive ||
      inst.retrying ||
      this.stopping
    )
      return;
    if (
      ["error", "ended"].includes(inst.status) &&
      [...this.instances.values()].filter(
        (other) =>
          !other.cancelled && !["error", "ended"].includes(other.status),
      ).length >= MAX_BOTS
    )
      throw new Error(
        "Up to 50 bots can be connected or queued. Stop one before retrying.",
      );
    inst.retrying = true;
    inst.status = "queued";
    inst.runVersion++;
    try {
      return await this.retryInstance(inst, customNameOrBool);
    } finally {
      inst.retrying = false;
    }
  }

  async retryInstance(inst, customNameOrBool) {
    if (typeof customNameOrBool === "string" && customNameOrBool.trim()) {
      inst.name = customNameOrBool.trim().slice(0, 16);
      this.log(
        `Updated bot ${inst.id} name to [${inst.name}]`,
        "system",
        inst.name,
      );
    } else if (customNameOrBool === true) {
      const currentUsed = new Set(
        [...this.instances.values()].map((i) => i.name.toLowerCase()),
      );
      currentUsed.delete(inst.name.toLowerCase());
      inst.name = this.generateUniqueRealName(currentUsed);
      this.log(
        `Assigned fresh unique real name [${inst.name}] to bot ${inst.id}`,
        "system",
        inst.name,
      );
    }

    const usedNames = new Set(
      [...this.instances.values()]
        .filter(
          (other) =>
            other !== inst &&
            other.pin === inst.pin &&
            !other.cancelled &&
            other.status !== "error",
        )
        .map((other) => other.name.toLowerCase()),
    );
    if (usedNames.has(inst.name.toLowerCase()))
      inst.name = this.generateUniqueRealName(usedNames);
    this.log(`Retrying bot [${inst.name}]...`, "system", inst.name);

    if (inst.pollTimer) clearTimeout(inst.pollTimer);
    try {
      if (inst.page && !inst.page.isClosed())
        await inst.page.close().catch(() => {});
      if (inst.context) await inst.context.close().catch(() => {});
    } catch {}

    if (inst.cancelled || this.instances.get(inst.id) !== inst) return;
    inst.pollRunning = false;
    inst.isAnswering = false;
    inst.lastAnswerAttemptKey = null;
    inst.connectionLostAt = null;
    inst.forceAnswerOnce = false;
    inst.status = "starting";
    inst.statusText = "Retrying connection...";
    inst.phase = "PHASE_SANDBOX";
    inst.diagnostic = null;
    inst.startTime = Date.now();
    inst.addEvent(
      "Initiating manual retry sequence...",
      "system",
      "PHASE_SANDBOX",
    );
    this.emit("state-change", this.getState());

    await this.runBotLifecycle(inst);
  }

  async retryAllErrors(useNewNames = false) {
    const errorBots = [...this.instances.values()].filter(
      (i) => i.status === "error",
    );
    if (errorBots.length === 0) return;
    this.log(`Retrying all ${errorBots.length} failed bots...`, "system");
    for (const inst of errorBots) {
      this.retryBot(inst.id, useNewNames).catch((err) =>
        this.log(`Retry failed: ${err.message}`, "error", inst.name),
      );
      await new Promise((r) => setTimeout(r, 220));
    }
  }

  async pickLobbyBlook(instance, shuffle = false) {
    if (
      !instance.page ||
      instance.page.isClosed() ||
      instance.cancelled ||
      instance.status !== "lobby"
    )
      return;
    const boxes = instance.page
      .locator(
        '[class*="blookBox"], [class*="blookContainer"], [class*="blookWrapper"]',
      )
      .filter({ visible: true });
    const count = await boxes.count();
    if (!count) return;
    let choice;
    if (shuffle || instance.blook === "random")
      choice = boxes.nth(Math.floor(Math.random() * count));
    else {
      const labels = await boxes.evaluateAll((elements) =>
        elements.map((element) =>
          `${element.innerText} ${element.getAttribute("aria-label") || ""} ${[...element.querySelectorAll("img")].map((image) => image.alt).join(" ")}`.toLowerCase(),
        ),
      );
      const index = labels.findIndex((label) =>
        label.includes(
          instance.blook.toLowerCase().replace("megabot", "mega bot"),
        ),
      );
      if (index < 0) return;
      choice = boxes.nth(index);
    }
    if (!instance.cancelled) await choice.click({ timeout: 1500 });
  }

  startBotLoop(instance) {
    const version = instance.runVersion;
    const active = () =>
      !instance.cancelled &&
      instance.runVersion === version &&
      this.instances.get(instance.id) === instance;
    if (instance.pollTimer) {
      clearInterval(instance.pollTimer);
      clearTimeout(instance.pollTimer);
    }

    const step = async () => {
      if (!active()) return;
      if (!instance.page || instance.page.isClosed()) {
        if (!["error", "ended"].includes(instance.status)) {
          this.handleBotError(
            instance,
            new Error("Browser page closed. Retry to reconnect."),
            instance.startTime,
          );
          await instance.context?.close().catch(() => {});
          instance.context = null;
          instance.page = null;
        }
        return;
      }

      if (instance.pollRunning || instance.isAnswering) return;
      clearTimeout(instance.pollTimer);

      instance.pollRunning = true;

      try {
        const state = await readGameScreen(instance.page);
        if (!active()) return;
        instance.pollFailures = 0;
        if (state.ended) {
          instance.status = "ended";
          instance.phase = "PHASE_ENDED";
          instance.statusText = "Game ended.";
          instance.forceAnswerOnce = false;
          const context = instance.context;
          instance.page = null;
          instance.context = null;
          await context?.close().catch(() => {});
          this.emit("state-change", this.getState());
          return;
        }

        // Distinguish an explicit host removal from a lost connection.
        const hostRemoved = state.hasKicked && !state.hasQuestion;
        if (hostRemoved || state.hasPinInput || state.hasNameInput) {
          instance.connectionLostAt ??= Date.now();
          if (hostRemoved || Date.now() - instance.connectionLostAt >= 2500) {
            instance.status = "error";
            instance.phase = hostRemoved
              ? "PHASE_REMOVED"
              : "PHASE_DISCONNECTED";
            instance.statusText = hostRemoved
              ? "Removed by the host."
              : "Connection closed.";
            instance.diagnostic = {
              category: "CONNECTION",
              code: hostRemoved ? "HOST_REMOVED" : "CONNECTION_LOST",
              explanation: instance.statusText,
              remediation: hostRemoved
                ? ["Check with the host before joining again."]
                : ["Check whether the game is still running, then retry."],
            };
            instance.addEvent(instance.statusText, "error", instance.phase);
            this.emit("state-change", this.getState());
            await instance.context?.close().catch(() => {});
            instance.context = null;
            instance.page = null;
          }
          return;
        }
        instance.connectionLostAt = null;

        // 3. In genuine lobby
        if (state.inLobby && instance.status !== "lobby") {
          instance.status = "lobby";
          instance.phase = "PHASE_LOBBY";
          instance.statusText = "Waiting for the host.";
          this.emit("state-change", this.getState());
          return;
        }

        // 4. Feedback / Transition / Next Screen Auto-Dismiss (Continuous Unblocking)
        if (
          (this.autoAnswerGlobal || instance.forceAnswerOnce) &&
          (state.hasFeedback ||
            state.hasNext ||
            state.hasChestReward ||
            state.isTransitionScreen)
        ) {
          if (
            await advanceFeedback(
              instance.page,
              () =>
                active() && (this.autoAnswerGlobal || instance.forceAnswerOnce),
            ).catch(() => false)
          ) {
            instance.lastAnswerAttemptKey = null;
          }
          return;
        }

        if (
          this.autoMinigames &&
          (this.autoAnswerGlobal || instance.forceAnswerOnce)
        ) {
          const generation = this.answerGeneration;
          const enabled = () =>
            active() &&
            generation === this.answerGeneration &&
            (this.autoAnswerGlobal || instance.forceAnswerOnce);
          if (await playMinigame(instance.page, state, this, enabled)) {
            instance.lastAnswerAttemptKey = null;
            return;
          }
        }

        // Answer only this bot’s current question.
        if (state.hasQuestion) {
          if (instance.status !== "game") {
            instance.status = "game";
            instance.phase = "PHASE_GAME";
            instance.statusText = "Game connected.";
            instance.addEvent(
              "Game match began · Answering questions",
              "action",
              "PHASE_GAME",
            );
            this.emit("state-change", this.getState());
          }

          const qData = {
            questionText: state.questionText,
            answers:
              state.answers.length > 0 ? state.answers : state.domAnswers,
            correctAnswers: state.correctAnswers || [],
            questionType: state.questionType || "mc",
            hasQuestion: true,
          };

          const currentQKey =
            (qData.questionText || "") + "::" + qData.answers.join("|");

          this.latestQuestion = {
            ...qData,
            botName: instance.name,
            timestamp: Date.now(),
          };
          if (this.autoAnswerGlobal || instance.forceAnswerOnce) {
            const force = Boolean(instance.forceAnswerOnce);
            if (instance.lastAnswerAttemptKey !== currentQKey) {
              if (await this.performBotAnswer(instance, qData, { force }))
                instance.forceAnswerOnce = false;
            }
          }
        }
      } catch (err) {
        if (active()) {
          instance.pollFailures = (instance.pollFailures || 0) + 1;
          if (instance.pollFailures === 3)
            this.log(
              `Could not read game screen: ${err.message}`,
              "warn",
              instance.name,
            );
        }
      } finally {
        if (instance.runVersion === version) instance.pollRunning = false;
        if (active() && instance.page && !instance.page.isClosed()) {
          const nextDelay = this.pollDelay(instance);
          instance.pollTimer = setTimeout(step, nextDelay);
        }
      }
    };

    instance.pollNow = step;
    instance.pollTimer = setTimeout(step, 80 + (instance.fleetIndex % 10) * 25);
  }

  pollDelay(instance) {
    return instance.status === "lobby"
      ? 1000
      : this.instances.size > 10
        ? 500
        : 300;
  }

  async performBotAnswer(instance, state, { force = false } = {}) {
    if (
      (!this.autoAnswerGlobal && !force) ||
      instance.isAnswering ||
      instance.cancelled ||
      !instance.page ||
      instance.page.isClosed()
    )
      return;
    const page = instance.page,
      version = instance.runVersion,
      generation = this.answerGeneration;
    const active = () =>
      this.answerGeneration === generation &&
      !instance.cancelled &&
      instance.runVersion === version &&
      instance.page === page &&
      !page.isClosed() &&
      (this.autoAnswerGlobal || force);
    const questionKey =
      (state.questionText || "") + "::" + (state.answers || []).join("|");
    if (instance.lastAnswerAttemptKey === questionKey) return;
    instance.isAnswering = true;
    try {
      let delay = 0;
      if (this.answerSpeedMode !== "instant") {
        const min = Math.max(0, this.answerDelayMinMs || 0),
          max = Math.max(min, this.answerDelayMaxMs || min);
        delay = min + Math.floor(Math.random() * (max - min + 1));
        if (this.adaptiveReadingTime)
          delay = Math.max(
            delay,
            Math.min(2000, (state.questionText || "").length * 12),
          );
        if (this.staggerFleet) delay += (instance.fleetIndex % 4) * 40;
      }
      if (delay) await new Promise((resolve) => setTimeout(resolve, delay));
      if (!active()) return;
      const choice = chooseAnswer(state, this, instance);
      if (!choice) return false;
      const submitted = await submitAnswer(
        page,
        state,
        {
          ...choice,
          humanKeystrokes: this.humanKeystrokes,
          clickOffsets: this.clickOffsets,
        },
        active,
      );
      if (!submitted || !active()) return false;
      instance.lastAnswerAttemptKey = questionKey;
      instance.lastAnswerAttemptTime = Date.now();
      instance.questionsAnswered++;
      this.totalQuestionsAnswered++;
      if (choice.known) {
        instance.knownAnswers++;
        this.totalKnownAnswers++;
        const isCorrect = (state.correctAnswers || []).some(
          (answer) =>
            String(answer).trim().toLowerCase() ===
            submitted.text.trim().toLowerCase(),
        );
        if (isCorrect) {
          instance.streak++;
          instance.correctAnswers++;
          this.totalCorrectAnswers++;
        } else instance.streak = 0;
      } else instance.streak = 0;
      instance.lastAnswer = submitted.text;
      instance.addEvent(
        `Answered: "${submitted.text}"`,
        "action",
        "PHASE_GAME",
      );
      this.emit("state-change", this.getState());
      return true;
    } finally {
      if (instance.runVersion === version) instance.isAnswering = false;
    }
  }

  async shuffleLobbyBlooks() {
    this.log("Shuffling Blooks across all bots in lobby...", "system");
    for (const inst of this.instances.values()) {
      if (inst.status === "lobby") {
        await this.pickLobbyBlook(inst, true).catch(() => {});
      }
    }
  }

  async forceAnswerAll() {
    for (const instance of this.instances.values()) {
      if (
        !instance.page ||
        instance.page.isClosed() ||
        instance.status === "error"
      )
        continue;
      instance.forceAnswerOnce = true;
      if (!instance.pollRunning && !instance.isAnswering && instance.pollNow) {
        clearTimeout(instance.pollTimer);
        instance.pollNow().catch(() => {});
      }
    }
  }

  async trimToOneBot() {
    const activeInstances = [...this.instances.values()].filter(
      (inst) => !inst.cancelled && !["error", "ended"].includes(inst.status),
    );
    if (activeInstances.length <= 1) {
      return { success: true, count: activeInstances.length };
    }
    const toStop = activeInstances.slice(1);
    for (const inst of toStop) {
      await this.stopBot(inst.id);
    }
    this.log(
      `Trimmed fleet from ${activeInstances.length} bots to 1 bot (${activeInstances[0].name})`,
      "action",
    );
    this.emit("state-change", this.getState());
    return { success: true, count: 1, kept: activeInstances[0].name };
  }

  async injectScript(scriptCode, scriptName = "Custom Cheat") {
    const activeInstances = [...this.instances.values()].filter(
      (inst) => inst.page && !inst.page.isClosed(),
    );
    if (activeInstances.length === 0) {
      return {
        success: false,
        unavailable: true,
        message:
          "Unavailable: No active bot running. Please launch 1 bot first.",
      };
    }
    if (activeInstances.length > 1) {
      return {
        success: false,
        unavailable: true,
        message: `Unavailable: Cheats only support 1 bot at a time. You currently have ${activeInstances.length} bots running. Please stop extra bots.`,
      };
    }

    const inst = activeInstances[0];
    this.log(
      `Fleet Injection: Deploying "${scriptName}" to single bot "${inst.name}"`,
      "action",
    );

    try {
      await inst.page.evaluate((botName) => {
        window.__botName = botName;
        window.getStateNode = () => {
          for (const element of document.querySelectorAll("*")) {
            const key = Object.keys(element).find(
              (name) =>
                name.startsWith("__reactFiber$") ||
                name.startsWith("__reactInternalInstance$"),
            );
            let fiber = key && element[key];
            for (
              let depth = 0;
              fiber && depth < 80;
              depth++, fiber = fiber.return
            ) {
              const node = fiber.stateNode;
              if (
                node &&
                !(node instanceof Element) &&
                typeof node.setState === "function" &&
                (node.props?.liveGameController ||
                  node.state?.stage ||
                  node.state?.question ||
                  node.state?.gold !== undefined)
              )
                return node;
            }
          }
          throw new Error(
            "This script is unavailable for the current game screen.",
          );
        };
        window.getStatenode = window.getStateNode;
        window.getClientName = (node) =>
          node?.props?.client?.name || node?.state?.name || window.__botName;
      }, inst.name);

      const res = await inst.page.evaluate((code) => {
        try {
          return Promise.resolve(eval(code)).then(
            (result) => ({ ok: true, result }),
            (error) => ({ ok: false, error: error.message }),
          );
        } catch (e) {
          return { ok: false, error: e.message };
        }
      }, scriptCode);

      inst.addEvent(
        `${res.ok ? "Ran" : "Failed"}: ${scriptName}`,
        res.ok ? "action" : "error",
        "SCRIPT_INJECT",
      );
      this.emit("state-change", this.getState());

      if (
        res.ok &&
        res.result &&
        (res.result.error || res.result.success === false)
      ) {
        return {
          success: false,
          error: res.result.error || res.result.message || "Script failed.",
          successful: 0,
          total: 1,
          message: res.result.message || res.result.error || "Script failed.",
        };
      }

      return {
        success: res.ok,
        successful: res.ok ? 1 : 0,
        total: 1,
        botName: inst.name,
        result: res.result,
        message: res.ok
          ? `Successfully executed "${scriptName}" on ${inst.name}!`
          : `Error: ${res.error}`,
      };
    } catch (err) {
      return {
        success: false,
        error: err.message,
        message: `Evaluation error: ${err.message}`,
      };
    }
  }

  async stopBot(id) {
    const inst = this.instances.get(id);
    if (!inst) return;
    inst.cancelled = true;
    this.joinScheduler.cancelInactive();
    this.instances.delete(id);
    if (inst.pollTimer) clearTimeout(inst.pollTimer);
    try {
      if (inst.page && !inst.page.isClosed())
        await inst.page.close().catch(() => {});
      if (inst.context) await inst.context.close().catch(() => {});
    } catch {}
    this.log(`Bot [${inst.name}] disconnected.`, "warn", inst.name);
    this.emit("state-change", this.getState());
  }

  stopFleet() {
    if (this.stopPromise) return this.stopPromise;
    this.stopPromise = this.stopSession().finally(() => {
      this.stopPromise = null;
      this.stopping = false;
    });
    return this.stopPromise;
  }

  async stopSession() {
    this.stopping = true;
    this.fleetGeneration++;
    for (const instance of this.instances.values()) {
      instance.cancelled = true;
      clearTimeout(instance.pollTimer);
    }
    this.joinScheduler.cancelInactive();
    if (this.browserPromise) await this.browserPromise.catch(() => {});
    this.log("Terminating active bot fleet...", "warn");
    for (const inst of this.instances.values()) {
      if (inst.pollTimer) clearTimeout(inst.pollTimer);
      try {
        if (inst.page && !inst.page.isClosed())
          await inst.page.close().catch(() => {});
        if (inst.context) await inst.context.close().catch(() => {});
      } catch {}
    }
    this.instances.clear();
    if (this.browser) {
      await this.browser.close().catch(() => {});
      this.browser = null;
    }
    this.starting = false;
    this.autoAnswerGlobal = false;
    this.answerGeneration++;
    this.latestQuestion = null;
    this.totalQuestionsAnswered = 0;
    this.totalCorrectAnswers = 0;
    this.totalKnownAnswers = 0;
    this.stopping = false;
    this.log("Session stopped.", "system");
    this.emit("state-change", this.getState());
  }

  updateSettings(input) {
    const patch = normalizeSettings(input, this);
    const enabled = patch.autoAnswer;
    delete patch.autoAnswer;
    Object.assign(this, patch);
    this.answerDelayMs = Math.round(
      (this.answerDelayMinMs + this.answerDelayMaxMs) / 2,
    );
    for (const instance of this.instances.values())
      instance.targetAccuracy = targetAccuracy(this, instance.fleetIndex);
    if (enabled !== undefined) {
      const changed = enabled !== this.autoAnswerGlobal;
      this.autoAnswerGlobal = enabled;
      if (changed || !enabled) this.answerGeneration++;
      for (const instance of this.instances.values()) {
        if (!enabled) instance.forceAnswerOnce = false;
        if (
          enabled &&
          changed &&
          instance.pollNow &&
          !instance.pollRunning &&
          !instance.isAnswering
        ) {
          clearTimeout(instance.pollTimer);
          instance.pollNow().catch(() => {});
        }
      }
    }
    this.emit("state-change", this.getState());
  }

  getState() {
    const list = [...this.instances.values()].map((i) => ({
      id: i.id,
      name: i.name,
      blook: i.blook,
      pin: i.pin,
      fleetIndex: i.fleetIndex,
      status: i.status,
      statusText: i.statusText,
      phase: i.phase,
      durationMs: i.durationMs,
      questionsAnswered: i.questionsAnswered,
      streak: i.streak,
      score: i.score,
      place: i.place,
      lastAnswer: i.lastAnswer,
      pingMs: i.pingMs,
      diagnostic: i.diagnostic,
      timeline: i.timeline,
      technicalTrace: i.technicalTrace,
    }));

    const errorCount = list.filter((i) => i.status === "error").length;
    const lobbyCount = list.filter((i) => i.status === "lobby").length;
    const inGameCount = list.filter((i) => i.status === "game").length;
    const activeBots = list.filter((i) =>
      [
        "queued",
        "starting",
        "navigating",
        "entering_pin",
        "entering_name",
        "lobby",
        "game",
      ].includes(i.status),
    );
    const accuracy =
      this.totalKnownAnswers > 0
        ? Math.round((this.totalCorrectAnswers / this.totalKnownAnswers) * 100)
        : null;

    return {
      pin: this.currentPin,
      baseName: this.baseName,
      namingStyle: this.namingStyle,
      namePattern: this.namePattern,
      nameStartNumber: this.nameStartNumber,
      previewNames: this.getPreviewNames(5),
      blookTheme: this.blookTheme,
      answerDelay: this.answerDelayMs,
      answerSpeedMode: this.answerSpeedMode,
      answerDelayMinMs: this.answerDelayMinMs,
      answerDelayMaxMs: this.answerDelayMaxMs,
      staggerFleet: this.staggerFleet,
      adaptiveReadingTime: this.adaptiveReadingTime,
      accuracyRate: this.accuracyRate,
      scoreSpread: this.scoreSpread,
      wrongAnswerStrategy: this.wrongAnswerStrategy,
      streakBreaker: this.streakBreaker,
      gameMode: this.gameMode,
      chestStrategy: this.chestStrategy,
      swapTarget: this.swapTarget,
      cryptoTarget: this.cryptoTarget,
      brawlStat: this.brawlStat,
      humanKeystrokes: this.humanKeystrokes,
      clickOffsets: this.clickOffsets,
      autoAnswer: this.autoAnswerGlobal,
      autoMinigames: this.autoMinigames,
      useFlareSolverr: this.useFlareSolverr,
      flaresolverr: this.flaresolverr.getStatus(),
      totalQuestions: this.totalQuestionsAnswered,
      accuracyPct: accuracy,
      instances: list,
      activeCount: activeBots.length,
      errorCount,
      lobbyCount,
      inGameCount,
      totalCount: list.length,
      latestQuestion: this.latestQuestion,
      answering: this.autoAnswerGlobal,
      starting: this.starting,
      stopping: this.stopping,
      logs: this.logHistory.slice(-50),
    };
  }
}
