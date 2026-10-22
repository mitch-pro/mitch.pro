import { setTimeout as pause } from "node:timers/promises";
import { readGameScreen } from "./blooket-screen.mjs";
import {
  submitAnswer,
  advanceFeedback,
  chooseAnswer,
} from "./blooket-answers.mjs";
import { playMinigame } from "./blooket-minigames.mjs";

export function answerStatus(manager, instance, message) {
  if (instance.answerStatus === message) return;
  instance.answerStatus = message;
  manager.emit("state-change", manager.getState());
}
const fallbackKey = (state) =>
  state.questionText + "::" + (state.answers || []).join("|");

/** One cancellable operation per connection. Pausing cancels a delay immediately;
 * a successful click is recorded even if a pause arrives just after dispatch.
 */
export async function performBotAnswer(
  manager,
  instance,
  state,
  { force = false } = {},
) {
  if (
    (!manager.autoAnswerGlobal && !force) ||
    instance.isAnswering ||
    instance.cancelled ||
    !instance.page ||
    instance.page.isClosed()
  )
    return false;
  const page = instance.page,
    version = instance.runVersion,
    generation = manager.answerGeneration;
  const key = state.questionKey || fallbackKey(state);
  if (instance.lastAnswerAttemptKey === key) return false;
  const controller = new AbortController();
  instance.answerAbort = controller;
  instance.isAnswering = true;
  const active = () =>
    !controller.signal.aborted &&
    manager.answerGeneration === generation &&
    !instance.cancelled &&
    instance.runVersion === version &&
    instance.page === page &&
    !page.isClosed() &&
    (manager.autoAnswerGlobal || force);
  try {
    let delay = 0;
    if (manager.answerSpeedMode !== "instant") {
      const min = manager.answerDelayMinMs,
        max = Math.max(min, manager.answerDelayMaxMs);
      delay = min + Math.floor(Math.random() * (max - min + 1));
      if (manager.adaptiveReadingTime)
        delay = Math.max(delay, Math.min(2000, state.questionText.length * 12));
      if (manager.staggerFleet) delay += (instance.fleetIndex % 4) * 40;
    }
    answerStatus(manager, instance, delay ? "Waiting to answer" : "Answering");
    if (delay) await pause(delay, undefined, { signal: controller.signal });
    if (!active()) return false;
    const choice = chooseAnswer(state, manager, instance);
    if (!choice) {
      answerStatus(manager, instance, "Waiting for an available answer");
      return false;
    }
    const submitted = await submitAnswer(
      page,
      state,
      { ...choice, humanKeystrokes: manager.humanKeystrokes },
      active,
    );
    if (!submitted) {
      if (active())
        answerStatus(manager, instance, "Waiting for the current question");
      return false;
    }
    instance.lastAnswerAttemptKey = key;
    instance.lastAnswerAttemptTime = Date.now();
    instance.questionsAnswered++;
    manager.totalQuestionsAnswered++;
    if (choice.known) {
      instance.knownAnswers++;
      manager.totalKnownAnswers++;
      const correct = state.correctAnswers.some(
        (answer) =>
          String(answer).trim().toLowerCase() ===
          submitted.text.trim().toLowerCase(),
      );
      if (correct) {
        instance.streak++;
        instance.correctAnswers++;
        manager.totalCorrectAnswers++;
      } else instance.streak = 0;
    } else instance.streak = 0;
    instance.lastAnswer = submitted.text;
    instance.addEvent(`Answered: "${submitted.text}"`, "action", "PHASE_GAME");
    answerStatus(
      manager,
      instance,
      manager.autoAnswerGlobal || force
        ? "Waiting for the next screen"
        : "Paused",
    );
    return true;
  } catch (error) {
    if (error.name === "AbortError" || !active()) return false;
    throw error;
  } finally {
    if (instance.answerAbort === controller) instance.answerAbort = null;
    if (instance.runVersion === version) instance.isAnswering = false;
  }
}

/** A serialized state machine replaces independent question/feedback daemons.
 * Every exit releases the poll lock and schedules exactly one subsequent read.
 * Terminal screens close only their own context; host removals never auto-rejoin.
 */
export function startBotLoop(manager, instance) {
  const version = instance.runVersion,
    token = Symbol("poll");
  instance.loopToken = token;
  clearTimeout(instance.pollTimer);
  const active = () =>
    !instance.cancelled &&
    instance.runVersion === version &&
    instance.loopToken === token &&
    manager.instances.get(instance.id) === instance;
  const enabled = () =>
    active() && (manager.autoAnswerGlobal || instance.forceAnswerOnce);
  const release = async () => {
    const context = instance.context;
    instance.context = null;
    instance.page = null;
    await context?.close().catch(() => {});
  };
  async function step() {
    if (!active() || instance.pollRunning || instance.isAnswering) return;
    clearTimeout(instance.pollTimer);
    instance.pollRunning = true;
    let failed = false;
    try {
      if (!instance.page || instance.page.isClosed()) {
        if (!["error", "ended"].includes(instance.status))
          manager.handleBotError(
            instance,
            new Error("Browser page closed. Retry to reconnect."),
            instance.startTime,
          );
        await release();
        return;
      }
      const state = await readGameScreen(instance.page);
      if (!active()) return;
      if (state.ended) {
        instance.status = "ended";
        instance.phase = "PHASE_ENDED";
        instance.statusText = "Game ended.";
        instance.forceAnswerOnce = false;
        answerStatus(manager, instance, "Finished");
        await release();
        manager.emit("state-change", manager.getState());
        return;
      }
      if (state.hasKicked || state.hasPinInput || state.hasNameInput) {
        instance.connectionLostAt ??= Date.now();
        if (state.hasKicked || Date.now() - instance.connectionLostAt >= 2500) {
          instance.status = "error";
          instance.phase = state.hasKicked
            ? "PHASE_REMOVED"
            : "PHASE_DISCONNECTED";
          instance.statusText = state.hasKicked
            ? "Removed by the host."
            : "Connection closed.";
          instance.diagnostic = {
            category: "CONNECTION",
            code: state.hasKicked ? "HOST_REMOVED" : "CONNECTION_LOST",
            explanation: instance.statusText,
            remediation: [
              state.hasKicked
                ? "Check with the host before joining again."
                : "Check whether the game is still running, then retry.",
            ],
          };
          instance.addEvent(instance.statusText, "error", instance.phase);
          await release();
          manager.emit("state-change", manager.getState());
        }
        return;
      }
      instance.connectionLostAt = null;
      if (state.inLobby) {
        instance.status = "lobby";
        instance.phase = "PHASE_LOBBY";
        instance.statusText = "Waiting for the host.";
        answerStatus(
          manager,
          instance,
          manager.autoAnswerGlobal ? "Ready when the game starts" : "Paused",
        );
        return;
      }
      if (state.hasQuestion) {
        instance.status = "game";
        instance.phase = "PHASE_GAME";
        instance.statusText = "Game connected.";
        manager.latestQuestion = {
          questionText: state.questionText,
          answers: state.answers,
          correctAnswers: state.correctAnswers,
          questionType: state.questionType,
          botName: instance.name,
          timestamp: Date.now(),
        };
        instance.lastActionKey = null;
        if (enabled()) {
          if (
            await performBotAnswer(manager, instance, state, {
              force: Boolean(instance.forceAnswerOnce),
            })
          )
            instance.forceAnswerOnce = false;
        } else answerStatus(manager, instance, "Paused");
        return;
      }
      const generation = manager.answerGeneration;
      const actionEnabled = () =>
        enabled() && generation === manager.answerGeneration;
      if (
        state.hasFeedback ||
        state.hasNext ||
        state.hasChests ||
        state.hasSwap ||
        state.hasCrypto ||
        state.hasFish ||
        state.hasPerk
      ) {
        instance.lastAnswerAttemptKey = null;
        if (!enabled()) {
          answerStatus(manager, instance, "Paused");
          return;
        }
        const actionKey = state.screenKey;
        if (actionKey && instance.lastActionKey === actionKey) {
          answerStatus(manager, instance, "Waiting for the game to advance");
          return;
        }
        let acted = false;
        if (state.hasFeedback || state.hasNext)
          acted = await advanceFeedback(instance.page, actionEnabled);
        else if (manager.autoMinigames)
          acted = await playMinigame(
            instance.page,
            state,
            manager,
            actionEnabled,
          );
        if (acted) instance.lastActionKey = actionKey;
        answerStatus(
          manager,
          instance,
          acted ? "Waiting for the next screen" : "Waiting for a game action",
        );
        return;
      }
      answerStatus(
        manager,
        instance,
        enabled() ? "Waiting for game controls" : "Paused",
      );
    } catch (error) {
      failed = true;
      if (active()) {
        instance.pollFailures = (instance.pollFailures || 0) + 1;
        answerStatus(
          manager,
          instance,
          "Could not read or operate this screen",
        );
        if (instance.pollFailures === 3)
          manager.log(
            `Game control failed: ${error.message}`,
            "warn",
            instance.name,
          );
      }
    } finally {
      if (!failed) instance.pollFailures = 0;
      if (instance.loopToken === token) instance.pollRunning = false;
      if (active() && instance.page && !instance.page.isClosed())
        instance.pollTimer = setTimeout(step, manager.pollDelay(instance));
    }
  }
  instance.pollNow = step;
  instance.pollTimer = setTimeout(step, 80 + (instance.fleetIndex % 10) * 25);
}
