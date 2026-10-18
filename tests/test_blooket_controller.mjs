import assert from "node:assert/strict";
import { chromium } from "@playwright/test";
import { mkdtemp, mkdir, writeFile, symlink, rm } from "node:fs/promises";
import { BlooketInstanceManager } from "../integrations/woodcreek/backend/blooket-manager.mjs";
import { normalizeSettings } from "../integrations/woodcreek/backend/blooket-settings.mjs";
import { readJoinScreen } from "../integrations/woodcreek/backend/blooket-connection.mjs";
import { readGameScreen } from "../integrations/woodcreek/backend/blooket-screen.mjs";
import { playMinigame } from "../integrations/woodcreek/backend/blooket-minigames.mjs";
import {
  executeCommand,
  HTTP_COMMANDS,
  readCheatScript,
} from "../integrations/woodcreek/backend/blooket-commands.mjs";
import { JoinScheduler } from "../integrations/woodcreek/backend/join-scheduler.mjs";
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const browser = await chromium.launch({ headless: true });
const temp = await mkdtemp("/tmp/blooket-controller-");
const managers = [];
function manager() {
  const instance = new BlooketInstanceManager();
  managers.push(instance);
  return instance;
}
async function until(check, message) {
  for (let i = 0; i < 80; i++) {
    if (await check()) return;
    await pause(20);
  }
  throw new Error(message);
}
try {
  const settings = manager();
  assert.equal(
    settings.getState().accuracyPct,
    null,
    "Unmeasured accuracy must not be fabricated",
  );
  const existing = settings.createBotInstance(
    "existing",
    "Alex",
    "cat",
    "123456",
  );
  existing.status = "lobby";
  settings.updateSettings({
    accuracy: 100,
    scoreSpread: "uniform",
    streakBreaker: 0,
  });
  assert.equal(
    existing.targetAccuracy,
    100,
    "Live accuracy changes must affect existing bots",
  );
  for (const patch of [
    { autoAnswer: "false" },
    { accuracy: "bad" },
    { answerDelayMinMs: Infinity },
    { gameMode: "unknown" },
    { autoAnswer: true, accuracy: NaN },
  ])
    assert.throws(() => settings.updateSettings(patch));
  assert.equal(
    settings.autoAnswerGlobal,
    false,
    "An invalid patch cannot partially enable answering",
  );
  settings.updateSettings({
    answerSpeedMode: "instant",
    answerDelayMinMs: 0,
    answerDelayMaxMs: 0,
  });
  assert.equal(settings.answerDelayMs, 0, "Zero delay is a valid setting");
  assert.deepEqual(normalizeSettings({ answerDelay: 800 }), {
    answerDelayMinMs: 800,
    answerDelayMaxMs: 800,
  });
  assert.throws(
    () => normalizeSettings({ answerDelayMinMs: 1500, answerDelayMaxMs: 100 }),
    /Minimum/,
  );
  for (const options of [
    { count: 0 },
    { count: 51 },
    { count: NaN },
    { names: "Alex" },
    { names: [{}] },
    { botBlooks: [null] },
  ])
    assert.throws(() => settings.validateStart({ pin: "123456", ...options }));
  const launches = manager();
  launches.ensureBrowser = async () => {};
  launches.runBotLifecycle = async (instance) => {
    instance.status = "lobby";
  };
  launches.updateSettings({
    accuracy: 88,
    answerSpeedMode: "custom",
    answerDelayMinMs: 700,
    answerDelayMaxMs: 900,
    namingStyle: "pattern",
    namePattern: "Guest_{n}",
    nameStartNumber: 4,
    useFlareSolverr: false,
  });
  await launches.startFleet({
    pin: "123456",
    count: 2,
    names: ["", "Blake"],
    botBlooks: ["cat", "wolf"],
  });
  assert.equal(launches.accuracyRate, 88);
  assert.equal(
    launches.answerDelayMinMs,
    700,
    "Joining must preserve omitted settings",
  );
  assert.deepEqual(
    [...launches.instances.values()].map((instance) => [
      instance.name,
      instance.blook,
    ]),
    [
      ["Guest_4", "cat"],
      ["Blake", "wolf"],
    ],
    "Blank roster slots keep avatar alignment and use the selected naming style",
  );
  await launches.startFleet({ pin: "123456", count: 1 });
  assert.equal(
    new Set([...launches.instances.values()].map((instance) => instance.name))
      .size,
    3,
    "Additional batches keep names unique",
  );
  await launches.trimToOneBot();
  assert.equal(
    launches.instances.size,
    1,
    "Trimming must stop queued connections too",
  );
  // A retry reserves capacity before asynchronous browser cleanup.
  const retries = manager();
  for (let i = 0; i < 49; i++) {
    const instance = retries.createBotInstance(
      String(i),
      "Bot" + i,
      "cat",
      "123456",
    );
    instance.status = "lobby";
  }
  const first = retries.createBotInstance("first", "First", "cat", "123456");
  first.status = "error";
  const second = retries.createBotInstance("second", "Second", "cat", "123456");
  second.status = "error";
  let release;
  first.context = {
    close: () => new Promise((resolve) => (release = resolve)),
  };
  retries.runBotLifecycle = async () => {};
  const pendingRetry = retries.retryBot("first");
  await assert.rejects(retries.retryBot("second"), /50 bots/);
  release();
  await pendingRetry;
  first.context = null;
  const page = await browser.newPage();
  await page.route("**/*", (route) => route.abort());
  await page.setContent(
    '<input placeholder="Nickname" hidden><div class="blookBox" hidden></div><div class="feedbackContainer" hidden></div><div class="questionText">Which fish weighs 3 lbs? Choose a chest. Incorrect?</div><div class="answersHolder"><button>3</button><button>4</button></div>',
  );
  let state = await readGameScreen(page);
  assert(state.hasQuestion);
  assert(
    !state.hasFish &&
      !state.hasFeedback &&
      !state.inLobby &&
      !state.hasNameInput,
    "Question wording and hidden leftovers must not become game actions",
  );
  await page.evaluate(() => {
    document.querySelector(".questionText").__reactFiber$fixture = {
      memoizedProps: {
        question: {
          question: document.querySelector(".questionText").innerText,
          answers: ["3", "4"],
          correctAnswers: ["4"],
        },
      },
      return: null,
    };
    window.clicks = 0;
    document
      .querySelectorAll("button")
      .forEach((button) => (button.onclick = () => window.clicks++));
  });
  state = await readGameScreen(page);
  assert.deepEqual(state.correctAnswers, ["4"]);
  const answering = manager();
  const bot = answering.createBotInstance("answer", "Alex", "cat", "123456");
  bot.page = page;
  bot.status = "game";
  answering.updateSettings({
    autoAnswer: true,
    accuracy: 100,
    scoreSpread: "uniform",
    answerSpeedMode: "instant",
  });
  await answering.performBotAnswer(bot, state);
  assert.equal(await page.evaluate(() => clicks), 1);
  bot.lastAnswerAttemptTime = Date.now() - 5000;
  await answering.performBotAnswer(bot, state);
  assert.equal(
    await page.evaluate(() => clicks),
    1,
    "A question cannot be resubmitted simply because time elapsed",
  );
  assert.equal(answering.getState().accuracyPct, 100);
  await page.setContent(
    '<div class="questionText">Unknown key</div><div class="answersHolder"><button>one</button><button>two</button></div>',
  );
  state = await readGameScreen(page);
  await answering.performBotAnswer(bot, state);
  assert.equal(
    answering.totalKnownAnswers,
    1,
    "Unknown keys do not count as correct or incorrect",
  );
  await page.setContent(
    '<div class="questionText">Type a word</div><input placeholder="Type your answer">',
  );
  state = await readGameScreen(page);
  assert.equal(state.questionType, "typing");
  assert.equal(
    await answering.performBotAnswer(bot, state),
    false,
    "Do not invent a typing answer when no key is available",
  );
  // A pending poll cannot overlap another poll, even after a long answer delay.
  answering.updateSettings({ autoAnswer: false });
  const pollContext = await browser.newContext(),
    pollPage = await pollContext.newPage();
  await pollPage.setContent(
    '<div class="questionText">The game is over: true or false?</div><div class="answersHolder"><button>True</button><button>False</button></div>',
  );
  assert.equal(
    (await readJoinScreen(pollPage)).isEnded,
    false,
    "Question wording cannot end a join",
  );
  const evaluate = pollPage.evaluate.bind(pollPage);
  let reading = 0,
    peak = 0;
  pollPage.evaluate = async (...args) => {
    reading++;
    peak = Math.max(peak, reading);
    try {
      await pause(70);
      return await evaluate(...args);
    } finally {
      reading--;
    }
  };
  const polling = answering.createBotInstance(
    "polling",
    "Casey",
    "cat",
    "123456",
  );
  polling.status = "game";
  polling.page = pollPage;
  polling.context = pollContext;
  answering.startBotLoop(polling);
  await Promise.all([polling.pollNow(), polling.pollNow()]);
  assert.equal(peak, 1, "Only one screen read may run per bot");
  await answering.stopBot(polling.id);
  // One chest action, even when it immediately turns into a question.
  await page.setContent(
    '<div class="choiceBox"><img alt="Chest 1"></div><div class="choiceBox"><img alt="Chest 2"></div>',
  );
  await page.evaluate(() => {
    window.actions = 0;
    document.querySelectorAll(".choiceBox").forEach(
      (box) =>
        (box.onclick = () => {
          window.actions++;
          document.body.innerHTML =
            '<div class="questionText">Next</div><div class="answersHolder"><button>A</button><button>B</button></div>';
          document
            .querySelectorAll("button")
            .forEach((button) => (button.onclick = () => window.actions++));
        }),
    );
  });
  state = await readGameScreen(page);
  assert(state.hasChests);
  await playMinigame(page, state, { chestStrategy: "chest_2" });
  assert.equal(await page.evaluate(() => actions), 1);
  assert.equal(
    await playMinigame(page, await readGameScreen(page), {}, () => false),
    false,
  );
  await page.setContent('<div class="questionText">Lobby</div>');
  const scriptResult = await answering.injectScript(
    "getStateNode().setState({gold:999})",
    "Unavailable fixture",
  );
  assert.equal(scriptResult.success, false);
  assert.equal(
    scriptResult.successful,
    0,
    "Unavailable scripts cannot report successful fake state writes",
  );
  const asyncFailure = await answering.injectScript(
    'Promise.reject(new Error("Async failure"))',
  );
  assert.equal(asyncFailure.success, false);
  const resultFailure = await answering.injectScript(
    '({success:false,message:"Not available"})',
  );
  assert.equal(resultFailure.success, false);
  // Ended games release their context, and closed pages no longer look online.
  const endedContext = await browser.newContext(),
    endedPage = await endedContext.newPage();
  await endedPage.setContent("<h1>Game ended.</h1>");
  const ended = answering.createBotInstance("ended", "Pat", "cat", "123456");
  ended.status = "game";
  ended.page = endedPage;
  ended.context = endedContext;
  answering.startBotLoop(ended);
  await ended.pollNow();
  assert.equal(ended.status, "ended");
  assert.equal(ended.context, null);
  assert(endedPage.isClosed());
  const closed = answering.createBotInstance(
    "closed",
    "Robin",
    "cat",
    "123456",
  );
  closed.status = "game";
  closed.page = await browser.newPage();
  await closed.page.close();
  answering.startBotLoop(closed);
  await closed.pollNow();
  assert.equal(closed.status, "error");
  // A recovering server still spaces launches rather than admitting a burst.
  let capacity = false;
  const starts = [];
  const scheduler = new JoinScheduler({
    capacity: async () => capacity,
    spacingMs: 60,
  });
  const jobs = [
    scheduler.run(() => starts.push(Date.now())),
    scheduler.run(() => starts.push(Date.now())),
  ];
  await pause(30);
  capacity = true;
  await Promise.all(jobs);
  assert(starts[1] - starts[0] >= 55);
  await mkdir(temp + "/cheats");
  await writeFile(temp + "/cheats/good.js", "42");
  await writeFile(temp + "/private.js", "secret");
  await symlink(temp + "/private.js", temp + "/cheats/link.js");
  assert.equal(await readCheatScript(temp, "good.js"), "42");
  for (const name of ["../private.js", "/etc/passwd", "link.js"])
    await assert.rejects(readCheatScript(temp, name));
  for (const route of ["/api/start", "/api/launch", "/api/fleet/launch"])
    assert.equal(HTTP_COMMANDS[route], "launch");
  await executeCommand(settings, { op: "settings", accuracy: 92 });
  assert.equal(existing.targetAccuracy, 92);
  await assert.rejects(
    executeCommand(settings, { op: "retry-bot", id: "missing" }),
    /existing bot/,
  );
  await assert.rejects(executeCommand(settings, { op: "unknown" }), /Unknown/);
  settings.latestQuestion = { questionText: "old" };
  settings.autoAnswerGlobal = true;
  settings.totalQuestionsAnswered = 10;
  const stops = [settings.stopFleet(), settings.stopFleet()];
  assert.equal(
    stops[0],
    stops[1],
    "Concurrent stop commands must await the same cleanup",
  );
  await Promise.all(stops);
  assert.equal(settings.autoAnswerGlobal, false);
  assert.equal(settings.latestQuestion, null);
  assert.equal(settings.totalQuestionsAnswered, 0);
  console.log(
    "PASS: settings validation and persistence, live accuracy, roster alignment, retry capacity races, question detection, one answer/action per screen, honest accuracy and script failures, ended/closed page cleanup, scheduler spacing, script confinement, command aliases, and shared stop completion. Simulated pages only.",
  );
} finally {
  await Promise.all(managers.map((manager) => manager.stopFleet()));
  await browser.close();
  await rm(temp, { recursive: true, force: true });
}
