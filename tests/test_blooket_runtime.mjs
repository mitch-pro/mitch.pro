// Browser fixtures only: never join a game or contact Blooket.
import assert from "node:assert/strict";
import { chromium } from "@playwright/test";
import { readGameScreen } from "../integrations/woodcreek/backend/blooket-screen.mjs";
import { executeCommand } from "../integrations/woodcreek/backend/blooket-commands.mjs";
import { joinGame } from "../integrations/woodcreek/backend/blooket-join.mjs";
import { advanceFeedback } from "../integrations/woodcreek/backend/blooket-answers.mjs";
import { JoinScheduler } from "../integrations/woodcreek/backend/join-scheduler.mjs";
import { BlooketInstanceManager } from "../integrations/woodcreek/backend/blooket-manager.mjs";

const browser = await chromium.launch();
const manager = new BlooketInstanceManager();
const page = await browser.newPage();
await page.route("**/*", (route) => route.abort());
async function question({ hooks = false, bank = false } = {}) {
  await page.setContent(
    '<main id="root"><div class="styles__questionText_ab">What is 2 + 2?</div><div class="styles__answersHolder_ab"><button class="styles__answerButton_ab"><span>3</span></button><button class="styles__answerButton_ab"><span>4</span></button></div></main>',
  );
  await page.evaluate(
    ({ hooks, bank }) => {
      const data = {
        question: "<b>What is 2 + 2?</b>",
        answers: ["3", "4"],
        correctAnswers: ["4"],
      };
      document.querySelector("[class*=questionText]").__reactFiber$fixture =
        hooks
          ? {
              memoizedState: { memoizedState: { question: data }, next: null },
              return: null,
            }
          : {
              memoizedProps: bank ? { questions: [data] } : { question: data },
              return: null,
            };
      window.clicks = 0;
      document.querySelectorAll("button").forEach(
        (button) =>
          (button.onclick = () => {
            window.clicks++;
            window.chosen = button.textContent;
          }),
      );
    },
    { hooks, bank },
  );
  return readGameScreen(page);
}
try {
  for (const source of [{ hooks: true }, { bank: true }, {}]) {
    const state = await question(source);
    assert(state.hasQuestion);
    assert.deepEqual(
      state.correctAnswers,
      ["4"],
      "Read the exact visible question from hooks, banks and HTML-rich props",
    );
  }
  const automatic = await executeCommand(manager, {
    op: "inject-script",
    fileName: "global/autoAnswer.min.js",
    scriptName: "Auto Answer",
  });
  assert(
    automatic.success && manager.autoAnswerGlobal,
    "Legacy auto-answer scripts must select the shared runner without reading or evaluating a script",
  );
  manager.updateSettings({ autoAnswer: false });
  const bot = manager.createBotInstance("fixture", "Player", "cat", "123456");
  bot.page = page;
  bot.status = "game";
  manager.updateSettings({
    autoAnswer: true,
    accuracy: 100,
    answerSpeedMode: "instant",
  });
  let state = await question();
  await manager.performBotAnswer(bot, state);
  await manager.performBotAnswer(bot, await readGameScreen(page));
  assert.equal(
    await page.evaluate(() => clicks),
    1,
    "A persistent screen is submitted only once",
  );
  const previousKey = state.questionKey;
  state = await question();
  assert.notEqual(
    state.questionKey,
    previousKey,
    "A new occurrence of the same question has a new identity",
  );
  await manager.performBotAnswer(bot, state);
  assert.equal(
    await page.evaluate(() => clicks),
    1,
    "An identical consecutive question remains answerable",
  );
  const stale = state;
  await question();
  assert.equal(
    await manager.performBotAnswer(bot, stale),
    false,
    "A delayed answer must not click a replaced question",
  );
  assert.equal(await page.evaluate(() => clicks), 0);
  state = await readGameScreen(page);
  await page.evaluate(() => {
    const overlay = document.createElement("div");
    overlay.id = "occlusion";
    overlay.style = "position:fixed;inset:0;background:#222;z-index:9999";
    document.body.append(overlay);
  });
  assert.equal(
    await manager.performBotAnswer(bot, state),
    false,
    "Do not click a question behind an unrecognized overlay",
  );
  assert.equal(await page.evaluate(() => clicks), 0);
  await page.evaluate(() => document.getElementById("occlusion").remove());
  state = await readGameScreen(page);
  manager.updateSettings({
    answerSpeedMode: "custom",
    answerDelayMinMs: 15000,
    answerDelayMaxMs: 15000,
  });
  const start = Date.now(),
    pending = manager.performBotAnswer(bot, state);
  await new Promise((resolve) => setTimeout(resolve, 30));
  manager.updateSettings({ autoAnswer: false });
  await pending;
  assert(
    Date.now() - start < 1000,
    "Pausing interrupts the delay instead of blocking polling for 15 seconds",
  );
  assert.equal(await page.evaluate(() => clicks), 0);
  await page.setContent(
    '<div class="questionText">Old question</div><div class="answersHolder"><button>A</button><button>B</button></div><div class="feedbackContainer">Correct. Continue</div>',
  );
  state = await readGameScreen(page);
  assert(
    state.hasFeedback && !state.hasQuestion,
    "A visible feedback overlay takes precedence over retained question nodes",
  );
  await page.setContent(
    '<div onclick="window.advances++">Click anywhere to continue</div>',
  );
  await page.evaluate(() => {
    window.advances = 0;
  });
  state = await readGameScreen(page);
  assert(state.hasNext, "A non-button continuation prompt must not stall");
  await advanceFeedback(page);
  assert.equal(await page.evaluate(() => advances), 1);
  await page.evaluate(() => {
    window.advances = 0;
  });
  manager.updateSettings({ autoAnswer: true, answerSpeedMode: "instant" });
  manager.startBotLoop(bot);
  await bot.pollNow();
  await bot.pollNow();
  assert.equal(
    await page.evaluate(() => advances),
    1,
    "A persistent transition cannot generate repeated clicks",
  );
  await manager.stopBot(bot.id);

  const joinPage = await browser.newPage();
  await joinPage.route("**/*", (route) => route.abort());
  await joinPage.setContent(`<form id="pin"><input placeholder="Game ID"><button>Continue</button></form><form id="name" hidden><input placeholder="Nickname"><button>Join</button></form><script>
    window.submissions=0;
    document.getElementById('pin').onsubmit=event=>{event.preventDefault();document.getElementById('pin').hidden=true;document.getElementById('name').hidden=false};
    document.getElementById('name').onsubmit=event=>{event.preventDefault();window.submissions++};
  </script>`);
  const connection = { pin: "123456", name: "Player" };
  await assert.rejects(
    joinGame(joinPage, connection, {
      active: () => true,
      phase: () => {},
      freshName: () => "NewPlayer",
      timeoutMs: 200,
    }),
    /did not confirm/,
  );
  assert.equal(
    await joinPage.evaluate(() => submissions),
    1,
    "An uncertain join cannot resubmit the nickname and replace another connection",
  );
  assert(connection.joinSubmitted);
  const retry = manager.createBotInstance(
    "uncertain",
    "Player",
    "cat",
    "123456",
  );
  manager.joinScheduler = new JoinScheduler({
    spacingMs: 0,
    capacity: async () => true,
  });
  let attempts = 0;
  manager.executeBotJoin = async (instance) => {
    attempts++;
    instance.joinSubmitted = true;
    throw new Error("Join not confirmed.");
  };
  await manager.runBotLifecycle(retry);
  assert.equal(
    attempts,
    1,
    "Do not start another browser join after an unconfirmed submission",
  );
  await joinPage.close();
  console.log(
    "PASS: DOM variants, React hooks/banks, rich text, repeated questions, stale screens, interruptible answering, transition controls and single-submission joins. No external requests.",
  );
} finally {
  await manager.stopFleet();
  await browser.close();
}
