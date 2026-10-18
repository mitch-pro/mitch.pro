import { JoinScheduler } from "./join-scheduler.mjs";
import { submitAnswer, advanceFeedback } from "./blooket-answers.mjs";
import { EventEmitter } from "events";
import fs from "fs";
import os from "os";
import { chromium } from "playwright";
import { PLAYER_URL, PIN_SELECTOR, NAME_SELECTOR, normalizePin, visibleField, submitJoinField, readJoinScreen } from "./blooket-connection.mjs";
import { FlareSolverrClient } from "./flaresolverr-client.mjs";

const CHROME_PATH = process.env.CHROME_PATH || chromium.executablePath();
const CGROUP_PATH = "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice/woodcreek.service";

const BLOOK_TYPES = [
  "wolf", "megabot", "dragon", "wizard", "astronaut",
  "king", "ghost", "frog", "shark", "cat"
];

export const REAL_SHORT_NAMES = [
  "Tyler", "Emma", "Noah", "Liam", "Ava", "Lucas", "Mia", "Ethan", "Chloe", "Jack",
  "Maya", "Henry", "Ella", "Owen", "Lily", "Leo", "Zoe", "Cole", "Nora", "Eli",
  "Sam", "Grace", "Ben", "Ruby", "Max", "Luke", "Alex", "Finn", "Ryan", "Jade",
  "Dean", "Paige", "Chase", "Kira", "Jude", "Gwen", "Reid", "Hazel", "Beau", "Iris",
  "Troy", "Quinn", "Caleb", "Ivy", "Grant", "Eden", "Drew", "Faye", "Cruz", "Piper",
  "Nash", "Thea", "Knox", "Vera", "Colt", "Rose", "Wade", "Hope", "Lane", "Joy",
  "Jace", "Bryn", "Zack", "Skye", "Kobe", "Tori", "Beck", "Demi", "Cash", "Elle",
  "Gage", "Sage", "Kip", "Mira", "Tate", "Zara", "Blake", "Lia", "Brock", "Nadia",
  "Bryce", "Cade", "Kyra", "Rhett", "Clark", "Fiona", "Graham", "Zane", "Tessa", "Shane",
  "Amber", "Brett", "Megan", "Cody", "Jenna", "Derek", "Holly", "Jesse", "Leah", "Kaden",
  "Gavin", "Trent", "Colin", "Naomi", "Wyatt", "Sean", "Brady", "Austin", "Travis", "Dalton",
  "Tanner", "Corey", "Dustin", "Wesley", "Trevor", "Kyle", "Jamie", "Devin", "Marcus", "Colton",
  "Derrick", "Hayden", "Damian", "Jonah", "Rowan", "Parker", "Bailey", "Carson", "Sawyer", "Declan",
  "Micah", "Josiah", "Elena", "Silas", "Elise", "Griffin", "Emmett", "Hudson", "Lincoln", "Lana",
  "Easton", "Bennett", "Weston", "Axel", "Roman", "Brooks", "Sasha", "Walker", "Maddox", "Camden",
  "Archer", "Holden", "Zander", "Dax", "Felix", "Theo", "Jules", "Milo", "Arlo", "Otis"
];

export const REAL_INITIAL_NAMES = [
  "Tyler M", "Emma S", "Noah T", "Liam B", "Ava C", "Lucas W", "Mia L", "Ethan D", "Chloe P", "Jack K",
  "Maya Z", "Henry P", "Ella V", "Owen J", "Lily G", "Leo F", "Zoe T", "Cole S", "Nora B", "Eli H",
  "Sam R", "Grace M", "Ben K", "Ruby W", "Max N", "Luke D", "Alex V", "Finn C", "Ryan H", "Jade L",
  "Dean M", "Paige R", "Chase T", "Kira S", "Jude B", "Gwen P", "Reid K", "Hazel N", "Beau D", "Iris G",
  "Troy J", "Quinn F", "Caleb L", "Ivy M", "Grant W", "Eden H", "Drew S", "Faye C", "Cruz R", "Piper K"
];

export const WHS_NAMES = [
  "Wolf_Ty", "Timber_Em", "WHS_Noah", "Creek_Sam", "WHS_Ava", "Timber_Max", "Wolf_Leo", "Creek_Zoe",
  "WHS_Cole", "Timber_Ben", "Wolf_Mia", "Creek_Eli", "WHS_Luke", "Timber_Ruby", "Wolf_Jack", "Creek_Nora",
  "WHS_Maya", "Timber_Alex", "Wolf_Finn", "Creek_Jade", "WHS_Ryan", "Timber_Dean", "Wolf_Paige", "Creek_Reid",
  "WHS_Gwen", "Timber_Beau", "Wolf_Iris", "Creek_Drew", "WHS_Nash", "Timber_Knox", "Wolf_Colt", "Creek_Hope",
  "WHS_Joy", "Timber_Jace", "Wolf_Zack", "Creek_Skye", "WHS_Beck", "Timber_Cash", "Wolf_Elle", "Creek_Gage",
  "WHS_Tate", "Timber_Zara", "Wolf_Blake", "Creek_Brock", "WHS_Bryce", "Timber_Cade", "Wolf_Rhett", "Creek_Clark",
  "WHS_Shane", "Timber_Cody"
];

export const STUDENT_NAMES = REAL_SHORT_NAMES;

export const CASUAL_STUDENT_NAMES = [
  "tyler_k", "emma_s", "noah_t", "liam_b", "ava_c", "lucas_w", "mia_l", "ethan_d", "chloe_p", "jack_k",
  "maya_z", "henry_p", "ella_v", "owen_j", "lily_g", "leo_f", "zoe_t", "cole_s", "nora_b", "eli_h",
  "sam_r", "grace_m", "ben_k", "ruby_w", "max_n", "luke_d", "alex_v", "finn_c", "ryan_h", "jade_l",
  "dean_m", "paige_r", "chase_t", "kira_s", "jude_b", "gwen_p", "reid_k", "hazel_n", "beau_d", "iris_g",
  "troy_j", "quinn_f", "caleb_l", "ivy_m", "grant_w", "eden_h", "drew_s", "faye_c", "cruz_r", "piper_k"
];

export const ESPORTS_NAMES = [
  "Vortex", "Phantom", "HyperNova", "NeonPulse", "AeroStrike",
  "OmegaShift", "ShadowByte", "Apex", "Zenith", "GlitchZero",
  "CyberBlitz", "RogueOne", "ViperX", "ZeroCool", "PixelGod",
  "Frostbite", "EchoStrike", "Flux", "Onyx", "Pulse"
];

export const MEME_NAMES = [
  "BlooketGod", "QuizletKing", "Brainiac", "200IQ", "Smarty",
  "MathWizard", "Speedy", "QuizMaster", "BigBrain", "GoldHoarder"
];

export const CYBER_NAMES = [
  "0xALPHA", "daemon", "byte_run", "null_ptr", "root_cmd",
  "syn_ack", "buffer", "stack", "socket", "heap"
];

export const FIRST_NAMES = REAL_SHORT_NAMES;

export const WOLF_WORDS = ["Timber", "Wolf", "Creek", "Wolfpack", "Apex", "Howl", "Fang", "Silver"];

function formatUptime(seconds) {
  const sec = Math.floor(seconds);
  const days = Math.floor(sec / 86400);
  const hours = Math.floor((sec % 86400) / 3600);
  const mins = Math.floor((sec % 3600) / 60);
  const s = sec % 60;

  if (days > 0) return `${days}d ${hours}h ${mins}m`;
  if (hours > 0) return `${hours}h ${mins}m ${s}s`;
  return `${mins}m ${s}s`;
}

function readCgroupFile(filename) {
  try {
    return fs.readFileSync(`${CGROUP_PATH}/${filename}`, "utf8").trim();
  } catch {
    return null;
  }
}

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
    this.baseName = "Tyler";
    this.namingStyle = "woodcreek";
    this.namePattern = "WHS_{n}";
    this.nameStartNumber = 1;
    this.customNames = [];
    this.blookTheme = "random";

    // Advanced Answer Timing & Jitter
    this.answerSpeedMode = "instant";
    this.answerDelayMinMs = 0;
    this.answerDelayMaxMs = 0;
    this.answerDelayMs = 0;
    this.staggerFleet = false;
    this.adaptiveReadingTime = false;

    // Advanced Accuracy & Score Distribution
    this.accuracyRate = 95;
    this.scoreSpread = "bell_curve";
    this.wrongAnswerStrategy = "smart_miss";
    this.streakBreaker = 12;

    // Game Mode Specialist Automations
    this.gameMode = "gold_quest";
    this.chestStrategy = "esp";
    this.swapTarget = "leader";
    this.cryptoTarget = "leader";
    this.cryptoDefense = "shields";
    this.fishingStyle = "deep";
    this.brawlStat = "damage";

    // Anti-Detection & Human Simulation
    this.humanKeystrokes = true;
    this.clickOffsets = true;

    this.autoAnswerGlobal = false;
    this.answerGeneration = 0;
    this.autoMinigames = true;
    this.logHistory = [];
    this.nextInstanceId = 1;
    this.totalQuestionsAnswered = 0;
    this.totalCorrectAnswers = 0;
    this.serviceStartTime = Date.now();
    this.flaresolverr = new FlareSolverrClient();
    this.useFlareSolverr = true;
    this.latestQuestion = null;
    this.currentFleetQuestionKey = null;
    this.currentFleetQuestionTime = 0;
    this.currentFleetQuestionData = null;
    this.isDispatchingFleetAnswer = false;
  }

  log(message, type = "info", botName = "") {
    const entry = {
      id: `log-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      timestamp: new Date().toISOString(),
      type,
      message,
      botName
    };
    this.logHistory.push(entry);
    if (this.logHistory.length > 250) this.logHistory.shift();
    this.emit("log", entry);
  }

  async ensureBrowser() {
    if (this.stopping) throw new Error("The previous session is still stopping. Please try again.");
    if (this.browser?.isConnected()) return this.browser;
    if (this.browserPromise) return this.browserPromise;
    this.browserPromise = this.launchBrowser();
    try { return await this.browserPromise; }
    finally { this.browserPromise = null; }
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
        "--no-first-run"
      ]
    });
    this.browser = browser;
    browser.on("disconnected", () => {
      if (this.browser !== browser) return;
      this.browser = null;
      if (this.stopping) return;
      this.log("Browser disconnected. Retry the affected connections.", "warn");
      for (const instance of this.instances.values()) {
        if (instance.cancelled) continue;
        clearTimeout(instance.autoAnswerLoop);
        instance.status = "error";
        instance.statusText = "Browser disconnected. Retry to reconnect.";
      }
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

    const initials = ["M", "S", "T", "B", "C", "W", "L", "D", "K", "R", "P", "H", "J", "F", "G", "V", "N", "Z"];
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
    let candidate = pool[idx];
    if (usedSet.has(candidate.toLowerCase())) {
      candidate = this.generateUniqueRealName(usedSet);
    }
    return candidate.slice(0, 16);
  }

  getPreviewNames(count = 5) {
    const list = [];
    for (let i = 1; i <= Math.min(count, 10); i++) {
      list.push(this.generateName(i, count));
    }
    return list;
  }

  assignBlook(index) {
    if (this.blookTheme === "wolf") return "wolf";
    if (this.blookTheme === "megabot") return "megabot";
    if (this.blookTheme === "dragon") return "dragon";
    if (this.blookTheme === "wizard") return "wizard";
    return BLOOK_TYPES[(index - 1) % BLOOK_TYPES.length];
  }

  validateStart(options = {}) {
    if (!options || typeof options !== "object" || Array.isArray(options)) throw new Error("Invalid game settings.");
    const pin = normalizePin(options.pin);
    if (this.starting || this.stopping) throw new Error("Wait for the current join or stop operation to finish.");
    const requested = this.fleetCount(options);
    const active = [...this.instances.values()].filter(instance=>!instance.cancelled&&instance.status!=="error").length;
    if (active+requested>50) throw new Error(`Up to 50 bots can be connected or queued. ${50-active} slots are available.`);
    return pin;
  }

  fleetCount(options={}) {
    const names=Array.isArray(options.names)&&options.names.length?options.names:options.customNames;
    const named=Array.isArray(names)?names.filter(name=>String(name).trim()).length:0;
    const requested=Number.isFinite(Number(options.count))&&Number(options.count)>0?Math.floor(Number(options.count)):named||5;
    return Math.max(1,Math.min(50,Math.max(requested,named)));
  }

  async startFleet(options = {}) {
    const pin = this.validateStart(options);
    this.starting = true;
    const generation = this.fleetGeneration;
    try { return await this.launchFleet({...options, pin}, generation); }
    finally { this.starting = false; }
  }

  async launchFleet({
    count,
    pin = "",
    names = [],
    customNames = [],
    botBlooks = [],
    baseName = "Tyler",
    namingStyle = "woodcreek",
    namePattern = "WHS_{n}",
    nameStartNumber = 1,
    blookTheme = "random",
    answerDelay = 400,
    answerSpeedMode = "fast",
    answerDelayMinMs = 250,
    answerDelayMaxMs = 250,
    staggerFleet = false,
    adaptiveReadingTime = true,
    accuracy = 95,
    scoreSpread = "bell_curve",
    wrongAnswerStrategy = "smart_miss",
    streakBreaker = 12,
    gameMode = "gold_quest",
    chestStrategy = "random",
    swapTarget = "leader",
    cryptoTarget = "leader",
    cryptoDefense = "shields",
    fishingStyle = "deep",
    brawlStat = "damage",
    humanKeystrokes = true,
    clickOffsets = true,
    autoAnswer,
    autoMinigames = true,
    useFlareSolverr = true
  }, generation) {
    this.currentPin = pin;
    this.baseName = String(baseName || "Tyler").trim().slice(0, 16);
    this.namingStyle = namingStyle;
    this.namePattern = String(namePattern || "WHS_{n}").trim();
    this.nameStartNumber = Math.max(1, Math.min(9999, Number(nameStartNumber) || 1));

    if (useFlareSolverr !== undefined) {
      this.useFlareSolverr = Boolean(useFlareSolverr);
    }

    // Combine explicit individual names or customNames array
    const rawExplicit = Array.isArray(names) && names.length > 0 ? names : customNames;
    const cleanNames = Array.isArray(rawExplicit)
      ? rawExplicit.map((n) => String(n).trim().slice(0, 16)).filter(Boolean)
      : [];

    this.customNames = cleanNames;
    this.blookTheme = blookTheme;
    this.answerSpeedMode = answerSpeedMode;
    this.answerDelayMinMs = Math.max(50, Math.min(10000, Number(answerDelayMinMs) || 1200));
    this.answerDelayMaxMs = Math.max(this.answerDelayMinMs, Math.min(15000, Number(answerDelayMaxMs) || 2800));
    this.answerDelayMs = Math.max(100, Math.min(15000, Number(answerDelay) || Math.round((this.answerDelayMinMs + this.answerDelayMaxMs) / 2)));
    this.staggerFleet = Boolean(staggerFleet);
    this.adaptiveReadingTime = Boolean(adaptiveReadingTime);

    this.accuracyRate = Math.max(50, Math.min(100, Number(accuracy) || 95));
    this.scoreSpread = scoreSpread || "bell_curve";
    this.wrongAnswerStrategy = wrongAnswerStrategy || "smart_miss";
    this.streakBreaker = Number(streakBreaker) || 0;

    this.gameMode = gameMode || "gold_quest";
    this.chestStrategy = chestStrategy || "random";
    this.swapTarget = swapTarget || "leader";
    this.cryptoTarget = cryptoTarget || "leader";
    this.cryptoDefense = cryptoDefense || "shields";
    this.fishingStyle = fishingStyle || "deep";
    this.brawlStat = brawlStat || "damage";

    this.humanKeystrokes = Boolean(humanKeystrokes);
    this.clickOffsets = Boolean(clickOffsets);
    if (autoAnswer !== undefined) this.autoAnswerGlobal = Boolean(autoAnswer);
    this.autoMinigames = Boolean(autoMinigames);

    if (!this.currentPin) {
      throw new Error("A valid Blooket Game PIN is required.");
    }

    const safeCount = this.fleetCount({count,names:cleanNames});

    this.log(`Deploying customized fleet of ${safeCount} bots for Game PIN ${this.currentPin}...`, "system");

    if (this.useFlareSolverr) {
      this.log("Consulting FlareSolverr for Cloudflare clearance session & tokens...", "system");
      try {
        const clearance = await this.flaresolverr.getClearance();
        if (clearance && clearance.cookies.length > 0) {
          this.log(`FlareSolverr clearance active (${clearance.cookies.length} cookies, ${clearance.latencyMs}ms).`, "info");
        } else {
          this.log("FlareSolverr unavailable; proceeding with direct browser launch.", "warn");
        }
      } catch (err) {
        this.log(`FlareSolverr clearance notice: ${err.message}. Proceeding directly.`, "warn");
      }
    }

    if (generation !== this.fleetGeneration) return;
    await this.ensureBrowser();
    if (generation !== this.fleetGeneration) return;

    // Strict deduplication pass: eliminate any duplicate/repeating names and ensure short, real names
    const usedFleetNames = new Set([...this.instances.values()].filter(instance=>!instance.cancelled&&instance.status!=="error"&&instance.pin===pin).map(instance=>instance.name.toLowerCase()));
    const finalFleetNames = [];
    for (let i = 0; i < safeCount; i++) {
      let rawCandidate = cleanNames[i] ? cleanNames[i].trim().slice(0, 16) : "";
      if (!rawCandidate || usedFleetNames.has(rawCandidate.toLowerCase())) {
        rawCandidate = this.generateUniqueRealName(usedFleetNames);
      }
      usedFleetNames.add(rawCandidate.toLowerCase());
      finalFleetNames.push(rawCandidate);
    }

    // Reserve every name before another request can enqueue joins.
    for (let i = 1; i <= safeCount; i++) {
      if (generation !== this.fleetGeneration) return;
      const name = finalFleetNames[i - 1];
      let blook = Array.isArray(botBlooks) && botBlooks[i - 1] ? botBlooks[i - 1] : this.assignBlook(i);
      if (!BLOOK_TYPES.includes(blook) && blook !== "random") {
        blook = this.assignBlook(i);
      }

      const instanceId = `bot-${this.nextInstanceId++}`;

      const inst = this.createBotInstance(instanceId, name, blook, this.currentPin, i);
      this.runBotLifecycle(inst).catch(err=>this.log(`Join failed: ${err.message}`,"error",inst.name));
    }

    this.emit("state-change", this.getState());
  }

  createBotInstance(id, name, blook, pin, fleetIndex = 1) {
    const startTime = Date.now();

    let individualAccuracy = this.accuracyRate;
    if (this.scoreSpread === "bell_curve") {
      const spreadOffset = ((fleetIndex % 5) - 2) * 3;
      individualAccuracy = Math.max(50, Math.min(100, this.accuracyRate + spreadOffset));
    } else if (this.scoreSpread === "tiered") {
      if (fleetIndex <= 1) individualAccuracy = Math.min(100, this.accuracyRate + 4);
      else if (fleetIndex >= 4) individualAccuracy = Math.max(50, this.accuracyRate - 8);
      else individualAccuracy = this.accuracyRate;
    }

    const instance = {
      id,
      name,
      blook,
      pin,
      fleetIndex,
      targetAccuracy: individualAccuracy,
      status: "starting",
      statusText: "Allocating sandbox browser context...",
      phase: "PHASE_SANDBOX",
      questionsAnswered: 0,
      streak: 0,
      score: 0,
      place: null,
      lastAnswer: "",
      lastActionAt: Date.now(),
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
      autoAnswerLoop: null,
      isAnswering: false,
      lastAnsweredQuestion: null,
      lastAnswerAttemptKey: null,
      lastAnswerAttemptTime: 0,
      answerAttemptCount: 0
    };

    instance.addEvent = (msg, type = "info", phase = instance.phase) => {
      const timeOffsetMs = Date.now() - instance.startTime;
      const formattedTime = new Date().toLocaleTimeString();

      instance.timeline.push({
        time: formattedTime,
        timeOffsetMs,
        msg,
        type,
        phase
      });
      if (instance.timeline.length > 30) instance.timeline.shift();

      instance.technicalTrace.push({
        timeOffsetMs,
        phase,
        status: type === "error" ? "FAIL" : "OK",
        msg
      });
      if (instance.technicalTrace.length > 40) instance.technicalTrace.shift();
    };

    instance.addEvent("Allocating isolated Chromium sandbox context", "info", "PHASE_SANDBOX");

    this.instances.set(id, instance);
    this.emit("state-change", this.getState());

    return instance;
  }

  async runBotLifecycle(instance) {
    if (instance.cancelled || instance.lifecycleActive) return;
    instance.lifecycleActive = true;
    const version = ++instance.runVersion;
    const active = () => !instance.cancelled && instance.runVersion === version && this.instances.get(instance.id) === instance;
    const startTime = Date.now();
    instance.status = "queued";
    instance.statusText = "Waiting to join...";
    try {
      await this.joinScheduler.run(async () => {
      for (let attempt = 0; attempt < 2 && active(); attempt++) {
        try { await this.executeBotJoin(instance, active); return; }
        catch (err) {
          if (!active()) return;
          const fatal = /invalid game pin|game (lobby )?is full|maximum player capacity|game session has already concluded/i.test(err.message);
          if (attempt === 0 && !fatal) {
            instance.addEvent(`Connection interrupted. Retrying: ${err.message}`, "warn", instance.phase);
            if (instance.context) await instance.context.close().catch(() => {});
            instance.context = null; instance.page = null;
            await new Promise(resolve => setTimeout(resolve, 1200));
          } else {
            this.handleBotError(instance, err, startTime);
            if (instance.context) await instance.context.close().catch(() => {});
            instance.context = null; instance.page = null;
            return;
          }
        }
      }
      }, active);
    } catch (err) {
      if (active()) this.handleBotError(instance,err,startTime);
    } finally { instance.lifecycleActive = false; }
  }

  async executeBotJoin(instance, active = () => !instance.cancelled && this.instances.get(instance.id) === instance) {
    const ensureActive = () => { if (!active()) throw new Error("Connection cancelled."); };
    ensureActive();
    const { pin } = instance;
    const startTime = Date.now();

    if (!this.browser || !this.browser.isConnected()) {
      await this.ensureBrowser();
    }

    instance.phase = "PHASE_SANDBOX";
    const clearance = this.useFlareSolverr ? this.flaresolverr.getCachedClearance() : null;
    const userAgent = clearance?.userAgent;

    const context = await this.browser.newContext({
      viewport: { width: 800, height: 600 },
      ...(userAgent ? {userAgent} : {}),
      serviceWorkers: "block",
      deviceScaleFactor: 1
    });

    if (!active()) { await context.close().catch(() => {}); ensureActive(); }
    instance.context = context;
    if (clearance?.cookies?.length) {
      await context.addCookies(clearance.cookies);
      instance.addEvent(`Injected FlareSolverr clearance (${clearance.cookies.length} cookies, UA synchronized)`, "info", "PHASE_SANDBOX");
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
        Object.defineProperty(navigator, 'webdriver', { get: () => undefined });
        window.chrome = { runtime: {}, loadTimes() {}, csi() {}, app: {} };
        Object.defineProperty(navigator, 'plugins', { get: () => [1, 2, 3, 4, 5] });
        Object.defineProperty(navigator, 'languages', { get: () => ['en-US', 'en'] });
      } catch {}
      const style = document.createElement('style');
      style.textContent = '*, *::before, *::after { transition: none !important; animation: none !important; scroll-behavior: auto !important; }';
      if (document.documentElement) document.documentElement.appendChild(style);
      else document.addEventListener("DOMContentLoaded", () => document.documentElement.appendChild(style), {once:true});
    });
    instance.page.setDefaultTimeout(25000);
    instance.page.setDefaultNavigationTimeout(35000);

    // Phase 2: Navigation
    instance.phase = "PHASE_NAVIGATE";
    instance.status = "navigating";
    instance.statusText = "Loading Blooket join portal...";
    instance.addEvent("Navigating to https://play.blooket.com/play", "info", "PHASE_NAVIGATE");
    this.emit("state-change", this.getState());

    let navResponse = await instance.page.goto(PLAYER_URL, {
      waitUntil: "domcontentloaded",
      timeout: 30000
    });

    ensureActive();
    // Check for Cloudflare challenge interstitials
    const pageTitle = await instance.page.title().catch(() => "");
    if (/just a moment|attention required|cloudflare|turnstile/i.test(pageTitle)) {
      instance.addEvent("Cloudflare challenge screen detected; requesting FlareSolverr clearance...", "warn", "PHASE_NAVIGATE");
      if (this.useFlareSolverr) {
        const fresh = await this.flaresolverr.getClearance({ forceRefresh: true });
        if (fresh?.cookies?.length) {
          if (fresh.userAgent && fresh.userAgent !== userAgent) throw new Error("Verification refreshed. Reopening the join page.");
          await context.addCookies(fresh.cookies);
          instance.addEvent("Injected refreshed FlareSolverr clearance; reloading page...", "info", "PHASE_NAVIGATE");
          navResponse = await instance.page.reload({ waitUntil: "domcontentloaded", timeout: 30000 });
        }
      }
    }

    const httpStatus = navResponse ? navResponse.status() : 200;
    if (httpStatus >= 400) {
      throw new Error(`HTTP ${httpStatus} received from play.blooket.com.`);
    }

    // Phase 3: Cookie Banner
    instance.phase = "PHASE_COOKIE";
    const cookieButtons = instance.page.locator('button:has-text("Accept All"), button:has-text("Accept"), button:has-text("Got it")');
    if (await cookieButtons.count() > 0) {
      await cookieButtons.first().click().catch(() => {});
      instance.addEvent("Dismissed cookie consent banner", "info", "PHASE_COOKIE");
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
          throw new Error("Invalid Game PIN: Blooket reported that this game could not be found.");
        }
        if (checkRes.isEnded) {
          throw new Error("Game session has already concluded.");
        }
        if (checkRes.isFull) {
          throw new Error("Game lobby is full (maximum player capacity reached).");
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
      const lateCheck = await instance.page.evaluate(() => document.body.innerText).catch(() => "");
      if (/invalid|not found|does not exist/i.test(lateCheck)) {
        throw new Error("Invalid Game PIN: PIN rejected by Blooket.");
      }
      throw new Error("Nickname input not found. Possible PIN rejection or slow network response.");
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
      instance.addEvent(`Entering nickname "${instance.name}" (attempt ${nicknameAttempts})`, "info", "PHASE_NAME_INPUT");
      this.emit("state-change", this.getState());

      if (randomNameFound) {
        const randomBtn = instance.page.locator('button:has-text("Generate"), button:has-text("Spin"), [class*="randomButton"]').first();
        await randomBtn.click().catch(() => {});
        await instance.page.waitForTimeout(400);
        const joinBtn = instance.page.locator('button:has-text("Join"), button:has-text("Let\'s Go"), form button, button[type="submit"]').first();
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
          throw new Error("Game lobby is full (maximum player capacity reached).");
        }
        if (check.isEnded) {
          throw new Error("Game session has already concluded.");
        }

        if (check.isTaken) {
          instance.addEvent(`Nickname "${instance.name}" was rejected/taken; auto-picking fresh tag...`, "warn", "PHASE_NAME_INPUT");
          const usedNames = new Set([...this.instances.values()].map((i) => i.name.toLowerCase()));
          instance.name = this.generateUniqueRealName(usedNames);
          break; // break to outer loop to resubmit new name
        }

        // CONFIRMATION: Name input GONE, PIN input GONE, and either lobby text or blook box visible or URL is lobby
        const currentUrl = instance.page ? instance.page.url() : "";
        const isLobbyUrl = currentUrl.includes("/lobby");
        if (!check.hasName && !check.hasPin && (check.hasLobbyTxt || check.hasBlooks || check.hasGame || isLobbyUrl)) {
          inLobbyConfirmed = true;
          joinedGame = check.hasGame;
          break;
        }
        await instance.page.waitForTimeout(100);
      }
    }

    if (!inLobbyConfirmed) {
      throw new Error(`Failed to confirm lobby entry for "${instance.name}". Nickname prompt did not advance or lobby failed to load.`);
    }

    ensureActive();
    // Phase 8: In Genuine Lobby
    instance.phase = "PHASE_LOBBY";
    instance.status = joinedGame ? "game" : "lobby";
    instance.statusText = joinedGame ? "Game connected." : "Waiting for the host.";
    instance.durationMs = Date.now() - startTime;
    instance.addEvent(`Joined lobby as "${instance.name}"`, "success", "PHASE_LOBBY");
    this.log(`Joined game lobby as [${instance.name}]!`, "success", instance.name);
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
      "Verify that play.blooket.com is accessible."
    ];

    const errMsg = err.message.toLowerCase();

    if (errMsg.includes("invalid game pin") || errMsg.includes("not found") || errMsg.includes("game id")) {
      category = "PIN_ERROR";
      code = "PIN_NOT_FOUND_OR_EXPIRED";
      explanation = "Blooket reported that the 6-digit Game PIN does not correspond to an active game lobby.";
      remediation = [
        "Check the host's screen to make sure the game lobby is still open.",
        "Ensure no trailing spaces or misread digits (e.g. 0 vs O).",
        "If the host closed or re-hosted the game, obtain the new 6-digit code."
      ];
    } else if (errMsg.includes("nickname") || errMsg.includes("taken") || errMsg.includes("not allowed")) {
      category = "NAME_ERROR";
      code = "NICKNAME_REJECTED";
      explanation = `The nickname "${name}" was rejected by Blooket because it is either already claimed or filtered by the host.`;
      remediation = [
        "Use 'Randomize All' to pick fresh unique student tags.",
        "Click 'Retry with New Name' to automatically deploy with a fresh unique tag."
      ];
    } else if (errMsg.includes("full") || errMsg.includes("capacity")) {
      category = "CAPACITY_ERROR";
      code = "LOBBY_MAX_CAPACITY";
      explanation = "The game lobby has reached maximum player capacity (typically 60 or 300 players depending on the host's Blooket plan).";
      remediation = [
        "Ask the host to upgrade their lobby capacity or remove idle players.",
        "Reduce the number of bots being deployed from your fleet."
      ];
    } else if (errMsg.includes("ended") || errMsg.includes("concluded")) {
      category = "SESSION_ERROR";
      code = "GAME_ALREADY_ENDED";
      explanation = "The host ended this game session before the bot could complete joining.";
      remediation = [
        "Wait for the host to start a new game match.",
        "Update the Game PIN in the hub when the new lobby opens."
      ];
    } else if (errMsg.includes("timeout") || errMsg.includes("timed out")) {
      category = "NETWORK_ERROR";
      code = "CONNECTION_TIMEOUT_30S";
      explanation = "Network connection to play.blooket.com timed out while waiting for page response.";
      remediation = [
        "The server launch rate has been auto-throttled to keep connections smooth.",
        "Verify the server upstream connection speed."
      ];
    } else if (errMsg.includes("cloudflare") || errMsg.includes("challenge") || errMsg.includes("captcha")) {
      category = "SECURITY_BLOCK";
      code = "CLOUDFLARE_BOT_CHALLENGE";
      explanation = "Blooket Cloudflare edge triggered a verification challenge on the server IP.";
      remediation = [
        "Click 'Re-solve Clearance' in the FlareSolverr status bar to get fresh clearance tokens.",
        "Pause launches for 1-2 minutes to allow IP cooldown."
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
      pageUrl: instance.page ? instance.page.url() : "https://play.blooket.com/play",
      timestamp: new Date().toISOString()
    };

    instance.addEvent(`Failed: ${err.message}`, "error", instance.phase);
    this.log(`${category} (${code}): ${err.message}`, "error", name);
    this.emit("state-change", this.getState());
  }

  async retryBot(id, customNameOrBool = "") {
    const inst = this.instances.get(id);
    if (!inst || inst.cancelled || inst.lifecycleActive || inst.retrying || this.stopping) return;
    inst.retrying = true;
    try { return await this.retryInstance(inst, customNameOrBool); }
    finally { inst.retrying = false; }
  }

  async retryInstance(inst, customNameOrBool) {
    if (typeof customNameOrBool === "string" && customNameOrBool.trim()) {
      inst.name = customNameOrBool.trim().slice(0, 16);
      this.log(`Updated bot ${inst.id} name to [${inst.name}]`, "system", inst.name);
    } else if (customNameOrBool === true) {
      const currentUsed = new Set([...this.instances.values()].map(i => i.name.toLowerCase()));
      currentUsed.delete(inst.name.toLowerCase());
      inst.name = this.generateUniqueRealName(currentUsed);
      this.log(`Assigned fresh unique real name [${inst.name}] to bot ${inst.id}`, "system", inst.name);
    }

    this.log(`Retrying bot [${inst.name}]...`, "system", inst.name);

    if (inst.autoAnswerLoop) clearTimeout(inst.autoAnswerLoop);
    try {
      if (inst.page && !inst.page.isClosed()) await inst.page.close().catch(() => {});
      if (inst.context) await inst.context.close().catch(() => {});
    } catch {}

    if (inst.cancelled || this.instances.get(inst.id) !== inst) return;
    inst.isLooping = false; inst.isAnswering = false;
    inst.lastAnsweredQuestion = null; inst.lastAnswerAttemptKey = null; inst.answerAttemptCount = 0;
    inst.status = "starting";
    inst.statusText = "Retrying connection...";
    inst.phase = "PHASE_SANDBOX";
    inst.diagnostic = null;
    inst.startTime = Date.now();
    inst.addEvent("Initiating manual retry sequence...", "system", "PHASE_SANDBOX");
    this.emit("state-change", this.getState());

    await this.runBotLifecycle(inst);
  }

  async retryAllErrors(useNewNames = false) {
    const errorBots = [...this.instances.values()].filter((i) => i.status === "error");
    if (errorBots.length === 0) return;
    this.log(`Retrying all ${errorBots.length} failed bots...`, "system");
    for (const inst of errorBots) {
      this.retryBot(inst.id, useNewNames).catch(err => this.log(`Retry failed: ${err.message}`, "error", inst.name));
      await new Promise((r) => setTimeout(r, 220));
    }
  }

  async pickLobbyBlook(instance) {
    if (!instance.page || instance.page.isClosed()) return;
    try {
      await instance.page.evaluate(() => {
        const boxes = [...document.querySelectorAll('[class*="blookBox"], [class*="blookContainer"], [class*="blookWrapper"]')];
        if (boxes.length > 0) {
          const choice = boxes[Math.floor(Math.random() * boxes.length)];
          choice?.click();
        }
      });
    } catch {}
  }


  startBotLoop(instance) {
    const version = instance.runVersion;
    const active = () => !instance.cancelled && instance.runVersion === version && this.instances.get(instance.id) === instance;
    if (instance.autoAnswerLoop) {
      clearInterval(instance.autoAnswerLoop);
      clearTimeout(instance.autoAnswerLoop);
    }

    const step = async () => {
      if (!active() || !instance.page || instance.page.isClosed()) {
        return;
      }

      let nextDelay = this.pollDelay(instance);

      if (instance.isLooping || instance.isAnswering) {
        if (instance.isAnswering && Date.now() - (instance.lastAnswerAttemptTime || 0) > 4000) {
          instance.isAnswering = false;
        } else {
          instance.autoAnswerLoop = setTimeout(step, nextDelay);
          return;
        }
      }

      instance.isLooping = true;

      try {
        const state = await instance.page.evaluate(() => {
          // 1. Traverse DOM and React 18 Fiber to find question props
          function extractQuestionFromFiber() {
            const candidates = [
              document.querySelector('#app > div > div'),
              document.querySelector('#app > div'),
              document.querySelector('#app'),
              document.querySelector('#root'),
              document.querySelector('div[class*="game"]'),
              document.querySelector('div[class*="answersHolder"]'),
              document.querySelector('div[class*="questionHolder"]'),
              document.body
            ].filter(Boolean);

            function checkProps(props) {
              if (!props || typeof props !== 'object') return null;
              if (props.question && typeof props.question === 'object') return props.question;
              if (typeof props.question === 'string' && (Array.isArray(props.answers) || Array.isArray(props.correctAnswers))) return props;
              if (props.client?.question && typeof props.client.question === 'object') return props.client.question;
              if (props.client?.state?.question && typeof props.client.state.question === 'object') return props.client.state.question;
              if (props.state?.question && typeof props.state.question === 'object') return props.state.question;
              return null;
            }

            for (const el of candidates) {
              let fiberKey = null;
              for (const k in el) {
                if (k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$') || k.startsWith('__reactContainer$')) {
                  fiberKey = k;
                  break;
                }
              }
              if (!fiberKey) continue;
              let curr = el[fiberKey];
              let depth = 0;
              while (curr && depth < 80) {
                const q = checkProps(curr.memoizedProps) || checkProps(curr.stateNode?.props) || checkProps(curr.stateNode?.state);
                if (q) {
                  const stage = curr.stateNode?.state?.stage || curr.memoizedProps?.stage || "";
                  return {
                    questionText: typeof q.question === 'string' ? q.question : (q.text || ""),
                    answers: Array.isArray(q.answers) ? q.answers : [],
                    correctAnswers: Array.isArray(q.correctAnswers) ? q.correctAnswers : (q.correctAnswers ? [q.correctAnswers] : []),
                    qType: q.qType || (Array.isArray(q.answers) && q.answers.length ? "mc" : "typing"),
                    stage,
                    source: "react-fiber"
                  };
                }
                curr = curr.return;
                depth++;
              }
            }
            return null;
          }

          const fiberQ = extractQuestionFromFiber();
          const stage = fiberQ?.stage || "";

          let questionText = fiberQ?.questionText || "";
          let answers = fiberQ?.answers || [];
          let correctAnswers = fiberQ?.correctAnswers || [];
          let questionType = fiberQ?.qType || "mc";

          // 2. Autonomous DOM scanning for visual fallbacks & verification
          const answerSelectors = [
            '[class*="answerContainer"]',
            '[class*="answerWrapper"]',
            '[class*="answersHolder"] > div',
            '[class*="answersWrapper"] > div',
            '[class*="answers"] > div',
            'div[role="button"][class*="answer"]',
            '[class*="styles__answer"]',
            '[class*="answerCard"]',
            '[class*="buttonContainer"]',
            '[class*="answerText"]'
          ];

          let domAnswers = [];
          for (const sel of answerSelectors) {
            const list = [...document.querySelectorAll(sel)].filter(el => {
              if (el.offsetParent === null) return false;
              const r = el.getBoundingClientRect();
              return r.width > 20 && r.height > 15;
            });
            if (list.length >= 2) {
              const topCards = list.filter(el => !list.some(other => other !== el && other.contains(el)));
              const finalEls = topCards.length >= 2 ? topCards : list;
              domAnswers = finalEls.map(el => {
                const textSpan = el.querySelector('[class*="answerText"], span, p') || el;
                return textSpan.innerText?.trim() || "";
              }).filter(Boolean);
              break;
            }
          }

          if (answers.length === 0 && domAnswers.length >= 2) {
            answers = domAnswers;
          }

          // DOM Typing check
          const typingInput = document.querySelector(
            'input[class*="typingInput"], input[placeholder*="Type"], input[placeholder*="Answer"], input[type="text"]:not([placeholder*="Game"]):not([placeholder*="Name"]):not([placeholder*="PIN"])'
          );
          if (typingInput && typingInput.offsetParent !== null) {
            questionType = "typing";
          }

          if (!questionText) {
            const qHeader = document.querySelector('[class*="questionText"], [class*="QuestionText"], [class*="questionHolder"], [class*="headerText"], [class*="questionContainer"]');
            if (qHeader) questionText = qHeader.innerText?.trim() || "";
          }

          const bodyText = document.body ? document.body.innerText : "";
          const pinInputEl = [...document.querySelectorAll('input[placeholder*="game id" i], input[placeholder*="pin" i], input[name="join-code"]')].find(el=>el.getClientRects().length);
          const hasPinInput = Boolean(pinInputEl && pinInputEl.offsetParent !== null);
          const nameInputEl = [...document.querySelectorAll('input[placeholder*="name" i], input[aria-label*="name" i], input[placeholder*="nickname" i]')].find(el=>el.getClientRects().length);
          const hasNameInput = Boolean(nameInputEl && nameInputEl.offsetParent !== null);
          const hasKicked = /you were kicked|you have been removed|kicked from (the )?game|host removed you/i.test(bodyText);
          const hasBlooks = Boolean(document.querySelector('[class*="blookBox"], [class*="blookContainer"], [class*="playerBlook"], [class*="blookWrapper"]'));

          // Specific feedback & transition screen detection (avoids broad word matching that breaks questions)
          const feedbackEl = document.querySelector('[class*="feedbackContainer"], [class*="feedbackWrapper"], [class*="feedback"], [class*="styles__feedback"]');
          const hasFeedbackEl = Boolean(feedbackEl && feedbackEl.offsetParent !== null);
          const isTransitionScreen = domAnswers.length < 2 && !hasPinInput && !hasNameInput && /(press anywhere to continue|click anywhere to (continue|go next)|click anywhere to cast|cast again|click to continue|click anywhere|nice job|correct!|incorrect|wrong answer|you got it|\+\s*\d+\s*(gold|crypto|points|tokens|lbs)|take\s*\d+%|c tier|b tier|a tier|s tier)/i.test(bodyText);
          const hasFeedback = stage === "feedback" || hasFeedbackEl || isTransitionScreen;

          const nextButton = document.querySelector('button[class*="nextButton"], [class*="nextButton"]');
          const hasNext = Boolean(nextButton && nextButton.offsetParent !== null) || /click anywhere to go next|press anywhere to continue/i.test(bodyText);

          // Minigames detection (Chests, Swap, Crypto, Fishing, Brawl, Factory)
          const hasChests = stage === "choice" || stage === "chests" ||
            Boolean(document.querySelector('img[alt^="Chest "], [class*="chest"], [class*="choiceBox"], [class*="treasureChoice"], [class*="styles__chest"]')) ||
            /choose a chest|pick a chest|open a chest/i.test(bodyText);

          const hasChestReward = /(click anywhere to go next|\+\s*\d+\s*gold|take\s*\d+%)/i.test(bodyText);

          const hasSwap = stage === "take" || stage === "swap" ||
            Boolean(document.querySelector('[class*="playerCard"], [class*="swapModal"], [class*="playerChoice"]')) ||
            /swap gold|take gold|choose a player/i.test(bodyText);

          const hasCrypto = stage === "hack" || stage === "password" ||
            Boolean(document.querySelector('[class*="passwordChoice"], [class*="passwordOption"], [class*="guessTile"], [class*="cryptoPlayerCard"]'));

          const hasFish = Boolean(document.querySelector('[class*="castButton"], [class*="reelButton"], [class*="fishingButton"]')) ||
            /click anywhere to cast|fish lost|click to cast again|cast again|c tier|b tier|a tier|s tier|lbs/i.test(bodyText);

          const hasPerk = Boolean(document.querySelector('[class*="upgradeCard"], [class*="perkBox"], [class*="abilityUpgrade"], [class*="serveCustomer"], [class*="restockButton"]'));

          const hasQuestion = Boolean(
            (stage === "question" || stage === "typing" || domAnswers.length >= 2 || answers.length >= 2 || questionType === "typing") &&
            !hasFeedback && !hasChestReward && !hasNext && !hasChests && !hasSwap && !hasCrypto && !hasFish
          );

          const hasGameElements = Boolean(document.querySelector('[class*="gameContainer"], [class*="answersHolder"], [class*="questionHolder"], [class*="headerText"]'));
          const inLobby = !hasQuestion && !hasFeedback && !hasNext && !hasChests && !hasSwap && !hasCrypto && !hasFish && !hasPinInput && !hasNameInput && !hasKicked &&
            (hasBlooks || /waiting for (the )?host|waiting for players|choose a blook/i.test(bodyText));

          return {
            stage,
            questionText,
            answers,
            domAnswers,
            correctAnswers,
            questionType,
            hasPinInput,
            hasNameInput,
            hasKicked,
            hasBlooks,
            inLobby,
            hasQuestion,
            hasGameElements,
            hasFeedback,
            hasNext,
            hasChests,
            hasChestReward,
            hasSwap,
            hasCrypto,
            hasFish,
            hasPerk,
            isTransitionScreen
          };
        });

        // Distinguish an explicit host removal from a lost connection.
        const hostRemoved=state.hasKicked&&!state.hasQuestion;
        if (hostRemoved || state.hasPinInput || state.hasNameInput) {
          instance.connectionLostAt ??= Date.now();
          if (hostRemoved || Date.now()-instance.connectionLostAt>=2500) {
            instance.status="error";
            instance.phase=hostRemoved?"PHASE_REMOVED":"PHASE_DISCONNECTED";
            instance.statusText=hostRemoved?"Removed by the host.":"Connection closed.";
            instance.diagnostic={category:"CONNECTION",code:hostRemoved?"HOST_REMOVED":"CONNECTION_LOST",explanation:instance.statusText,remediation:hostRemoved?["Check with the host before joining again."]:["Check whether the game is still running, then retry."]};
            instance.addEvent(instance.statusText,"error",instance.phase);
            this.emit("state-change",this.getState());
            await instance.context?.close().catch(()=>{});
            instance.context=null;instance.page=null;
          }
          return;
        }
        instance.connectionLostAt=null;

        // 3. In genuine lobby
        if (state.inLobby && instance.status !== "lobby") {
          instance.status = "lobby";
          instance.phase = "PHASE_LOBBY";
          instance.statusText = "In lobby · Standing by for host to start match";
          this.emit("state-change", this.getState());
          return;
        }

        // 4. Feedback / Transition / Next Screen Auto-Dismiss (Continuous Unblocking)
        if ((this.autoAnswerGlobal || instance.forceAnswerOnce) && (state.hasFeedback || state.hasNext || state.hasChestReward || state.isTransitionScreen)) {
          if(await advanceFeedback(instance.page).catch(()=>false)) {
            instance.lastAnswerAttemptKey=null;instance.answerAttemptCount=0;
          }
          return;
        }

        // 5. Minigames Progression
        if (this.autoMinigames && (this.autoAnswerGlobal || instance.forceAnswerOnce)) {
          // Gold Quest Chests
          if (state.hasChests && !state.hasChestReward) {
            await instance.page.evaluate((strategy) => {
              const imgChests = [...document.querySelectorAll('img[alt^="Chest "]')];
              const classChests = [...document.querySelectorAll('[class*="chest"], [class*="choiceBox"], [class*="treasureChoice"], [class*="styles__chest"]')];
              const chests = imgChests.length > 0 ? imgChests : classChests;
              if (chests.length === 0) return;
              
              let targetIdx = 0;
              if (strategy === "esp" || strategy === "best") {
                try {
                  const stateNode = Object.values(document.querySelector('#app > div > div') || {}).find(x => x?.stateNode)?.stateNode;
                  if (stateNode && stateNode.state && stateNode.state.choices) {
                    const choices = stateNode.state.choices;
                    let bestScore = -9999;
                    choices.forEach((c, i) => {
                      let score = 0;
                      if (c.text === "Swap") score = 10000;
                      else if (c.text.includes("Gold")) {
                        if (c.val > 0) score = c.val * (c.text.includes("%") ? 100 : 1);
                        else score = c.val * 1000;
                      }
                      if (score > bestScore) {
                        bestScore = score;
                        targetIdx = i;
                      }
                    });
                  } else {
                    targetIdx = Math.floor(Math.random() * chests.length);
                  }
                } catch { targetIdx = Math.floor(Math.random() * chests.length); }
              } else if (strategy === "chest_1") targetIdx = 0;
              else if (strategy === "chest_2") targetIdx = Math.min(1, chests.length - 1);
              else if (strategy === "chest_3") targetIdx = Math.min(2, chests.length - 1);
              else targetIdx = Math.floor(Math.random() * chests.length);
              
              chests[targetIdx]?.click();
              chests[targetIdx]?.parentElement?.click();
            }, this.chestStrategy).catch(() => {});

            await instance.page.mouse.click(400, 350).catch(() => {});
            instance.lastAnswerAttemptKey = null;
          }

          // Gold Quest Swap / Steal
          if (state.hasSwap) {
            await instance.page.evaluate((policy) => {
              const players = [...document.querySelectorAll('[class*="playerCard"], [class*="playerChoice"], [class*="swapModal"] div[role="button"], [class*="swapModal"] div')];
              if (players.length === 0) return;
              if (policy === "never") {
                const closeBtn = document.querySelector('[class*="closeButton"], [class*="cancel"]');
                if (closeBtn) { closeBtn.click(); return; }
              }
              if (policy === "leader" || players.length === 1) {
                players[0]?.click();
              } else {
                const rand = players[Math.floor(Math.random() * players.length)];
                rand?.click();
              }
            }, this.swapTarget).catch(() => {});
            instance.lastAnswerAttemptKey = null;
          }

          // Crypto Hack
          if (state.hasCrypto) {
            await instance.page.evaluate((cryptoTarget) => {
              const passwordChoices = document.querySelectorAll('[class*="passwordChoice"], [class*="passwordOption"], [class*="guessTile"]');
              if (passwordChoices.length > 0) {
                passwordChoices[0]?.click();
              }
              const victimCards = document.querySelectorAll('[class*="cryptoPlayerCard"], [class*="hackVictim"]');
              if (victimCards.length > 0) {
                if (cryptoTarget === "leader") victimCards[0]?.click();
                else victimCards[Math.floor(Math.random() * victimCards.length)]?.click();
              }
            }, this.cryptoTarget).catch(() => {});
            instance.lastAnswerAttemptKey = null;
          }

          // Fishing Frenzy & Perks
          if (state.hasFish) {
            await instance.page.evaluate(() => {
              const castBtn = document.querySelector('[class*="castButton"], [class*="reelButton"], [class*="fishingButton"]');
              if (castBtn) castBtn.click();
            }).catch(() => {});
            await instance.page.mouse.click(400, 300).catch(() => {});
            await instance.page.keyboard.press("Space").catch(() => {});
            instance.lastAnswerAttemptKey = null;
          }

          if (state.hasPerk) {
            await instance.page.evaluate((brawlStat) => {
              const upgradeCards = document.querySelectorAll('[class*="upgradeCard"], [class*="perkBox"], [class*="abilityUpgrade"]');
              if (upgradeCards.length > 0) {
                let idx = 0;
                if (brawlStat === "defense" && upgradeCards.length > 1) idx = 1;
                else if (brawlStat === "speed" && upgradeCards.length > 2) idx = 2;
                upgradeCards[idx]?.click();
              }
              const restock = document.querySelector('[class*="restockButton"], [class*="serveCustomer"], [class*="feedCustomer"]');
              restock?.click();
            }, this.brawlStat).catch(() => {});
            instance.lastAnswerAttemptKey = null;
          }
        }

        // 6. Active Question Answering with Fleet Simultaneous Synchronization
        if (state.hasQuestion) {
          if (instance.status !== "game") {
            instance.status = "game";
            instance.phase = "PHASE_GAME";
            instance.statusText = "Match active · In Game";
            instance.addEvent("Game match began · Answering questions", "action", "PHASE_GAME");
            this.emit("state-change", this.getState());
          }

          const qData = {
            questionText: state.questionText,
            answers: state.answers.length > 0 ? state.answers : state.domAnswers,
            correctAnswers: state.correctAnswers || [],
            questionType: state.questionType || "mc",
            hasQuestion: true
          };

          const currentQKey = (qData.questionText || "") + "::" + (qData.answers.join("|") || qData.questionType);

          this.latestQuestion={...qData,botName:instance.name,timestamp:Date.now()};
          if (this.autoAnswerGlobal || instance.forceAnswerOnce) {
            const force=Boolean(instance.forceAnswerOnce);instance.forceAnswerOnce=false;
            if (instance.lastAnswerAttemptKey!==currentQKey || Date.now()-instance.lastAnswerAttemptTime>2200) {
              await this.performBotAnswer(instance,qData,1,force);
            }
          }
        }
      } catch (err) {
        // Log minor evaluate errors at debug level to avoid UI error floods
      } finally {
        instance.isLooping = false;
        if (active() && instance.page && !instance.page.isClosed()) {
          const nextDelay = this.pollDelay(instance);
          instance.autoAnswerLoop = setTimeout(step, nextDelay);
        }
      }
    };

    instance.pollNow = step;
    instance.autoAnswerLoop = setTimeout(step, 80+(instance.fleetIndex%10)*25);
  }

  pollDelay(instance) {
    return instance.status==="lobby"?1000:this.instances.size>10?500:300;
  }

  async performBotAnswer(instance,state,attemptCount=1,force=false) {
    if ((!this.autoAnswerGlobal&&!force)||instance.isAnswering||instance.cancelled||!instance.page||instance.page.isClosed())return;
    const page=instance.page,version=instance.runVersion,generation=this.answerGeneration;
    const active=()=>this.answerGeneration===generation&&!instance.cancelled&&instance.runVersion===version&&instance.page===page&&!page.isClosed()&&(this.autoAnswerGlobal||force);
    const questionKey=(state.questionText||"")+"::"+(state.answers||[]).join("|");
    if(instance.lastAnswerAttemptKey===questionKey&&Date.now()-instance.lastAnswerAttemptTime<2200)return;
    instance.isAnswering=true;
    try {
      let delay=0;
      if(this.answerSpeedMode!=="instant") {
        const min=Math.max(0,this.answerDelayMinMs||0),max=Math.max(min,this.answerDelayMaxMs||min);
        delay=min+Math.floor(Math.random()*(max-min+1));
        if(this.adaptiveReadingTime)delay=Math.max(delay,Math.min(2000,(state.questionText||"").length*12));
        if(this.staggerFleet)delay+=(instance.fleetIndex%4)*40;
      }
      if(delay)await new Promise(resolve=>setTimeout(resolve,delay));
      if(!active())return;
      const answers=state.answers||[],correct=state.correctAnswers||[];
      const correctText=correct[0];
      const targetAccuracy=instance.targetAccuracy??this.accuracyRate;
      const wantCorrect=Math.random()*100<targetAccuracy&&!(this.streakBreaker>0&&instance.streak>=this.streakBreaker);
      const incorrect=answers.filter(answer=>!correct.some(item=>String(item).trim().toLowerCase()===String(answer).trim().toLowerCase()));
      const text=state.questionType==="typing"?(wantCorrect&&correctText?String(correctText):"pass"):
        wantCorrect&&correctText?String(correctText):String(incorrect[0]??answers[0]??"");
      if(!text)return;
      const submitted=await submitAnswer(page,state,{text});
      if(!submitted||!active())return;
      instance.lastAnswerAttemptKey=questionKey;instance.lastAnswerAttemptTime=Date.now();
      instance.questionsAnswered++;this.totalQuestionsAnswered++;
      const isCorrect=correct.some(answer=>String(answer).trim().toLowerCase()===submitted.text.trim().toLowerCase());
      if(isCorrect){instance.streak++;this.totalCorrectAnswers++;}else instance.streak=0;
      instance.lastAnswer=submitted.text;
      instance.addEvent(`Answered: "${submitted.text}"`,"action","PHASE_GAME");
      this.emit("state-change",this.getState());
    } finally {instance.isAnswering=false;}
  }

  async shuffleLobbyBlooks() {
    this.log("Shuffling Blooks across all bots in lobby...", "system");
    for (const inst of this.instances.values()) {
      if (inst.status === "lobby") {
        await this.pickLobbyBlook(inst).catch(() => {});
      }
    }
  }

  async forceAnswerAll() {
    for(const instance of this.instances.values()) {
      if(!instance.page||instance.page.isClosed()||instance.status==="error")continue;
      instance.forceAnswerOnce=true;
      if(!instance.isLooping&&!instance.isAnswering&&instance.pollNow){clearTimeout(instance.autoAnswerLoop);instance.pollNow().catch(()=>{});}
    }
  }

  async trimToOneBot() {
    const activeInstances = [...this.instances.values()].filter((inst) => inst.page && !inst.page.isClosed());
    if (activeInstances.length <= 1) {
      return { success: true, count: activeInstances.length };
    }
    const toStop = activeInstances.slice(1);
    for (const inst of toStop) {
      await this.stopBot(inst.id);
    }
    this.log(`Trimmed fleet from ${activeInstances.length} bots to 1 bot (${activeInstances[0].name})`, "action");
    this.emit("state-change", this.getState());
    return { success: true, count: 1, kept: activeInstances[0].name };
  }

  async injectScript(scriptCode, scriptName = "Custom Cheat") {
    const activeInstances = [...this.instances.values()].filter((inst) => inst.page && !inst.page.isClosed());
    if (activeInstances.length === 0) {
      return { success: false, unavailable: true, message: "Unavailable: No active bot running. Please launch 1 bot first." };
    }
    if (activeInstances.length > 1) {
      return {
        success: false,
        unavailable: true,
        message: `Unavailable: Cheats only support 1 bot at a time. You currently have ${activeInstances.length} bots running. Please stop extra bots.`
      };
    }

    const inst = activeInstances[0];
    this.log(`Fleet Injection: Deploying "${scriptName}" to single bot "${inst.name}"`, "action");

    try {
      // Ensure bot name and universal Blooket helper are accessible in page
      await inst.page.evaluate((botName) => {
        window.__botName = botName;

        window.getBlooketGameContext = function() {
          let room = null;
          const all = document.querySelectorAll('*');
          for (const el of all) {
            for (const k of Object.keys(el)) {
              if (k.startsWith('__reactFiber$')) {
                let f = el[k];
                while (f) {
                  if (f.memoizedState) {
                    let s = f.memoizedState;
                    while (s) {
                      if (s.memoizedState && typeof s.memoizedState === 'object' && s.memoizedState.sessionId && s.memoizedState.roomId) {
                        room = s.memoizedState;
                        break;
                      }
                      s = s.next;
                    }
                  }
                  if (room) break;
                  f = f.return;
                }
              }
              if (room) break;
            }
            if (room) break;
          }

          let stateNode = null;
          for (const el of all) {
            for (const k of Object.keys(el)) {
              if (k.startsWith('__reactFiber$') || k.startsWith('__reactInternalInstance$')) {
                let f = el[k];
                let d = 0;
                while (f && d < 60) {
                  const sn = f.stateNode;
                  if (sn && !(sn instanceof HTMLElement) && typeof sn === 'object' && !sn.containerInfo) {
                    if (sn.props?.liveGameController || sn.state?.gold !== undefined || sn.state?.crypto !== undefined || sn.state?.choices) {
                      stateNode = sn;
                      break;
                    }
                  }
                  f = f.return;
                  d++;
                }
              }
              if (stateNode) break;
            }
            if (stateNode) break;
          }

          const myName = window.__botName || (document.querySelector("[class*='nameText'], [class*='headerPlayerNameText']")?.innerText?.trim()) || "Jonah";

          return {
            room,
            stateNode,
            name: myName,
            isColyseus: Boolean(room),
            isFirebase: Boolean(stateNode?.props?.liveGameController)
          };
        };

        window.getStateNode = function() {
          const ctx = window.getBlooketGameContext();
          if (ctx.stateNode) return ctx.stateNode;

          const colyseusPlayers = (ctx.room && ctx.room.state && ctx.room.state.players) ? ctx.room.state.players : null;
          const playerObj = colyseusPlayers
            ? (colyseusPlayers.get ? colyseusPlayers.get(ctx.name) : colyseusPlayers[ctx.name])
            : null;

          const activeScore = playerObj ? playerObj.score : 0;

          return {
            _synthetic: true,
            context: ctx,
            invulnerable: true,
            ghostMode: false,
            player: playerObj || {
              x: 0,
              y: 0,
              health: 999999,
              maxHealth: 999999,
              invulnerable: true,
              score: activeScore
            },
            enemies: [
              { health: 1, maxHealth: 1, die() { this.health = 0; } }
            ],
            gems: [{ x: 0, y: 0 }],
            abilities: {
              dmg: { level: 10, maxLevel: 10 },
              speed: { level: 10, maxLevel: 10 },
              magnet: { level: 10, maxLevel: 10 }
            },
            towers: [{ level: 3 }],
            state: {
              name: ctx.name,
              gold: activeScore,
              gold2: activeScore,
              crypto: activeScore,
              crypto2: activeScore,
              cafeCash: activeScore,
              cash: activeScore,
              tokens: activeScore,
              coins: activeScore,
              fossils: activeScore,
              doubloons: activeScore,
              toys: activeScore,
              weight: activeScore,
              weight2: activeScore,
              progress: 100,
              people: 100,
              materials: 100,
              military: 100,
              strength: 100,
              charisma: 100,
              wisdom: 100,
              health: 999999,
              lives: 999999,
              password: "admin",
              hack: "",
              frenzy: true,
              frenzyTime: 999999,
              lure: 5,
              distraction: false,
              customers: [],
              foods: [
                { name: "Toast", stock: 99, level: 5 },
                { name: "Milk", stock: 99, level: 5 }
              ],
              items: { speed: 5, customers: 5 },
              blooks: [{ name: "Mega Bot", upgradePrice: 0, level: 4 }],
              cards: [{ name: "Hero", strength: 100, charisma: 100, wisdom: 100 }],
              enemy: { health: 1 },
              guest: { name: "King", choices: [{ text: "Yes" }, { text: "No" }] },
              choices: [
                { type: "gold", val: 5000000, text: "5,000,000 Gold" },
                { type: "multiply", val: 3, text: "Triple Gold!" },
                { type: "swap", text: "Swap Gold!" }
              ],
              unlocks: []
            },
            setState(updates, cb) {
              if (!updates) return;
              Object.assign(this.state, updates);
              const newScore = updates.gold ?? updates.gold2 ?? updates.crypto ?? updates.crypto2 ?? updates.cash ?? updates.cafeCash ?? updates.tokens ?? updates.coins ?? updates.fossils ?? updates.doubloons ?? updates.toys ?? updates.weight;
              if (newScore !== undefined) {
                if (playerObj) playerObj.score = newScore;
                document.querySelectorAll('[class*="headerScoreText"], [class*="goldText"], [class*="scoreText"]').forEach(el => {
                  el.innerText = Number(newScore).toLocaleString();
                });
              }
              if (typeof cb === 'function') cb();
            },
            props: {
              client: { name: ctx.name },
              liveGameController: {
                setVal({ path, val }) {
                  if (path && (path.includes('/g') || path.includes('/ca') || path.includes('/cr') || path.includes('/w') || path.includes('/f') || path.includes('/d') || path.includes('/t') || path.includes('/pr'))) {
                    if (playerObj) playerObj.score = val;
                    document.querySelectorAll('[class*="headerScoreText"], [class*="goldText"], [class*="scoreText"]').forEach(el => {
                      el.innerText = Number(val).toLocaleString();
                    });
                  }
                  return true;
                },
                getDatabaseVal(path, cb) {
                  if (typeof cb === 'function') {
                    const mockDb = {};
                    mockDb[ctx.name] = { g: activeScore, cr: activeScore, ca: activeScore, w: activeScore };
                    cb(mockDb);
                  }
                }
              }
            },
            choosePrize(i) {
              const chests = document.querySelectorAll("div[class*='choice'], img[alt^='Chest ']");
              if (chests && chests[i]) chests[i].click();
            },
            takeDamage() {},
            nextLevel() {},
            nextWave() {},
            nextGuest() {}
          };
        };

        window.getStatenode = window.getStateNode;
        window.getClientName = function(sn) {
          return window.__botName || sn?.props?.client?.name || sn?.state?.name || "Jonah";
        };
      }, inst.name).catch(() => {});

      const res = await inst.page.evaluate((code) => {
        try {
          return { ok: true, result: eval(code) };
        } catch (e) {
          return { ok: false, error: e.message };
        }
      }, scriptCode);

      inst.addEvent(`Injected cheat: ${scriptName}`, "action", "SCRIPT_INJECT");
      this.emit("state-change", this.getState());

      if (res.ok && res.result && res.result.error) {
        return {
          success: false,
          error: res.result.error,
          message: `Cheat error: ${res.result.error}`
        };
      }

      return {
        success: res.ok,
        successful: 1,
        total: 1,
        botName: inst.name,
        result: res.result,
        message: res.ok ? `Successfully executed "${scriptName}" on ${inst.name}!` : `Error: ${res.error}`
      };
    } catch (err) {
      return { success: false, error: err.message, message: `Evaluation error: ${err.message}` };
    }
  }

  async stopBot(id) {
    const inst = this.instances.get(id);
    if (!inst) return;
    inst.cancelled = true;
    this.joinScheduler.cancelInactive();
    this.instances.delete(id);
    if (inst.autoAnswerLoop) clearTimeout(inst.autoAnswerLoop);
    try {
      if (inst.page && !inst.page.isClosed()) await inst.page.close().catch(() => {});
      if (inst.context) await inst.context.close().catch(() => {});
    } catch {}
    this.instances.delete(id);
    this.log(`Bot [${inst.name}] disconnected.`, "warn", inst.name);
    this.emit("state-change", this.getState());
  }

  async stopFleet() {
    if (this.stopping) return;
    this.stopping = true;
    this.fleetGeneration++;
    for (const instance of this.instances.values()) { instance.cancelled = true; clearTimeout(instance.autoAnswerLoop); }
    this.joinScheduler.cancelInactive();
    if (this.browserPromise) await this.browserPromise.catch(() => {});
    this.log("Terminating active bot fleet...", "warn");
    for (const inst of this.instances.values()) {
      if (inst.autoAnswerLoop) clearTimeout(inst.autoAnswerLoop);
      try {
        if (inst.page && !inst.page.isClosed()) await inst.page.close().catch(() => {});
        if (inst.context) await inst.context.close().catch(() => {});
      } catch {}
    }
    this.instances.clear();
    if (this.browser) {
      await this.browser.close().catch(() => {});
      this.browser = null;
    }
    this.stopping = false;
    this.log("Session stopped.", "system");
    this.emit("state-change", this.getState());
  }

  updateSettings({
    autoAnswer,
    autoMinigames,
    answerDelay,
    answerSpeedMode,
    answerDelayMinMs,
    answerDelayMaxMs,
    staggerFleet,
    adaptiveReadingTime,
    accuracy,
    scoreSpread,
    wrongAnswerStrategy,
    streakBreaker,
    gameMode,
    chestStrategy,
    swapTarget,
    cryptoTarget,
    cryptoDefense,
    fishingStyle,
    brawlStat,
    humanKeystrokes,
    clickOffsets,
    namingStyle,
    namePattern,
    nameStartNumber,
    blookTheme,
    useFlareSolverr
  }) {
    if (autoAnswer !== undefined) {
      const enabled=Boolean(autoAnswer),changed=enabled!==this.autoAnswerGlobal;
      this.autoAnswerGlobal=enabled;
      if(changed||!enabled)this.answerGeneration++;
      for(const instance of this.instances.values()) {
        if(!enabled)instance.forceAnswerOnce=false;
        if(enabled&&changed&&instance.pollNow&&!instance.isLooping&&!instance.isAnswering){clearTimeout(instance.autoAnswerLoop);instance.pollNow().catch(()=>{});}
      }
    }
    if (autoMinigames !== undefined) this.autoMinigames = Boolean(autoMinigames);
    if (answerDelay !== undefined) this.answerDelayMs = Number(answerDelay);
    if (answerSpeedMode !== undefined) this.answerSpeedMode = String(answerSpeedMode);
    if (answerDelayMinMs !== undefined) this.answerDelayMinMs = Number(answerDelayMinMs);
    if (answerDelayMaxMs !== undefined) this.answerDelayMaxMs = Number(answerDelayMaxMs);
    if (staggerFleet !== undefined) this.staggerFleet = Boolean(staggerFleet);
    if (adaptiveReadingTime !== undefined) this.adaptiveReadingTime = Boolean(adaptiveReadingTime);
    if (accuracy !== undefined) this.accuracyRate = Number(accuracy);
    if (scoreSpread !== undefined) this.scoreSpread = String(scoreSpread);
    if (wrongAnswerStrategy !== undefined) this.wrongAnswerStrategy = String(wrongAnswerStrategy);
    if (streakBreaker !== undefined) this.streakBreaker = Number(streakBreaker);
    if (gameMode !== undefined) this.gameMode = String(gameMode);
    if (chestStrategy !== undefined) this.chestStrategy = String(chestStrategy);
    if (swapTarget !== undefined) this.swapTarget = String(swapTarget);
    if (cryptoTarget !== undefined) this.cryptoTarget = String(cryptoTarget);
    if (cryptoDefense !== undefined) this.cryptoDefense = String(cryptoDefense);
    if (fishingStyle !== undefined) this.fishingStyle = String(fishingStyle);
    if (brawlStat !== undefined) this.brawlStat = String(brawlStat);
    if (humanKeystrokes !== undefined) this.humanKeystrokes = Boolean(humanKeystrokes);
    if (clickOffsets !== undefined) this.clickOffsets = Boolean(clickOffsets);
    if (namingStyle !== undefined) this.namingStyle = namingStyle;
    if (namePattern !== undefined) this.namePattern = String(namePattern);
    if (nameStartNumber !== undefined) this.nameStartNumber = Number(nameStartNumber);
    if (blookTheme !== undefined) this.blookTheme = blookTheme;
    if (useFlareSolverr !== undefined) this.useFlareSolverr = Boolean(useFlareSolverr);
    this.emit("state-change", this.getState());
  }

  getSystemMetrics() {
    const totalMem = os.totalmem();
    const freeMem = os.freemem();
    const usedMem = totalMem - freeMem;
    const procMem = process.memoryUsage();

    const cgroupMemRaw = readCgroupFile("memory.current");
    const cgroupPeakRaw = readCgroupFile("memory.peak");
    const cgroupProcsRaw = readCgroupFile("cgroup.procs");

    const serviceMemBytes = cgroupMemRaw ? Number(cgroupMemRaw) : procMem.rss;
    const servicePeakBytes = cgroupPeakRaw ? Number(cgroupPeakRaw) : procMem.rss;
    const servicePids = cgroupProcsRaw ? cgroupProcsRaw.split("\n").filter(Boolean).length : (this.instances.size + 1);

    const serverUptimeSec = os.uptime();
    const programUptimeSec = (Date.now() - this.serviceStartTime) / 1000;

    return {
      hostname: os.hostname(),
      platform: `${os.type()} ${os.release()} (${os.arch()})`,
      cpuModel: os.cpus()[0]?.model || "Intel Skylake 4-Core",
      cpuCores: os.cpus().length,
      loadAverage: os.loadavg()[0].toFixed(2),
      loadAverage15: os.loadavg()[2].toFixed(2),
      serverUptimeSec: Math.floor(serverUptimeSec),
      serverUptimeFormatted: formatUptime(serverUptimeSec),

      totalMemoryMB: Math.round(totalMem / (1024 * 1024)),
      usedMemoryMB: Math.round(usedMem / (1024 * 1024)),
      freeMemoryMB: Math.round(freeMem / (1024 * 1024)),
      memoryPercent: Math.round((usedMem / totalMem) * 100),

      programUptimeSec: Math.floor(programUptimeSec),
      programUptimeFormatted: formatUptime(programUptimeSec),
      serviceMemoryMB: Math.round(serviceMemBytes / (1024 * 1024)),
      servicePeakMemoryMB: Math.round(servicePeakBytes / (1024 * 1024)),
      servicePids,
      nodeRssMB: Math.round(procMem.rss / (1024 * 1024)),
      nodeHeapMB: Math.round(procMem.heapUsed / (1024 * 1024)),
      activeBotWorkers: this.instances.size
    };
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
      technicalTrace: i.technicalTrace
    }));

    const errorCount = list.filter((i) => i.status === "error").length;
    const lobbyCount = list.filter((i) => i.status === "lobby").length;
    const inGameCount = list.filter((i) => i.status === "game").length;
    const activeBots = list.filter((i) => ["queued", "starting", "navigating", "entering_pin", "entering_name", "lobby", "game"].includes(i.status));
    const accuracy = this.totalQuestionsAnswered > 0
      ? Math.round((this.totalCorrectAnswers / this.totalQuestionsAnswered) * 100)
      : 100;

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
      cryptoDefense: this.cryptoDefense,
      fishingStyle: this.fishingStyle,
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
      metrics: this.getSystemMetrics(),
      latestQuestion: this.latestQuestion,
      logs: this.logHistory.slice(-50)
    };
  }
}
