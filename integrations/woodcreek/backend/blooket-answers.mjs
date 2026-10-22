import { GAME_CONTROLS } from "./blooket-controls.mjs";
import {
  readGameScreen,
  inspectGameDocument,
  inspectOptions,
} from "./blooket-screen.mjs";
import { submitJoinField } from "./blooket-connection.mjs";
export {
  QUESTION_SELECTOR,
  TYPING_SELECTOR,
  ANSWER_SELECTORS,
} from "./blooket-screen.mjs";
const clean = (value) =>
  String(value ?? "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();

/** Re-observe after the configured delay. A question key identifies an occurrence,
 * not just its wording; the browser checks it again before dispatching a click.
 */
export async function submitAnswer(
  page,
  observed,
  choice,
  active = () => true,
) {
  if (!active()) return null;
  const current = await readGameScreen(page);
  if (!current.hasQuestion || !active()) return null;
  if (
    observed.questionKey
      ? observed.questionKey !== current.questionKey
      : clean(observed.questionText) !== clean(current.questionText) ||
        JSON.stringify((observed.answers || []).map(clean)) !==
          JSON.stringify(current.answers.map(clean))
  )
    return null;
  if (current.questionType === "typing") {
    const input = page.locator(
      `[data-tylerium-control="${current.inputTarget}"]`,
    );
    if (!active() || !(await input.isEnabled())) return null;
    await input.fill(choice.humanKeystrokes ? "" : choice.text);
    if (choice.humanKeystrokes)
      for (const char of choice.text) {
        if (!active()) return null;
        await input.pressSequentially(char, { delay: 40 });
      }
    const ready = await readGameScreen(page);
    if (!active() || ready.questionKey !== current.questionKey) return null;
    await submitJoinField(input);
    return { text: choice.text, questionKey: current.questionKey };
  }
  const index = current.answers.findIndex(
    (answer) => clean(answer) === clean(choice.text),
  );
  if (index < 0 || !active()) return null;
  return page.evaluate(inspectGameDocument, {
    ...inspectOptions(),
    intent: {
      questionKey: current.questionKey,
      index,
      text: choice.text,
    },
  });
}

export async function advanceFeedback(page, active = () => true) {
  if (!active()) return false;
  const state = await readGameScreen(page);
  if (state.hasQuestion || !(state.hasFeedback || state.hasNext)) return false;
  for (const selector of GAME_CONTROLS.feedback) {
    const control = page.locator(selector).filter({ visible: true }).first();
    if ((await control.count()) && active()) {
      await control.click({ timeout: 1500 });
      return true;
    }
  }
  const control = page
    .getByText(
      /^(?:click|press) anywhere to (?:continue|go next|cast)|^(?:continue|go next|claim|collect)$/i,
    )
    .filter({ visible: true })
    .first();
  if ((await control.count()) && active()) {
    await control.click({ timeout: 1500 });
    return true;
  }
  return false;
}

export function chooseAnswer(state, settings, instance, random = Math.random) {
  const answers = state.answers || [],
    correct = state.correctAnswers || [];
  const correctText = correct.find(
    (text) =>
      state.questionType === "typing" ||
      answers.some((answer) => clean(answer) === clean(text)),
  );
  if (correctText === undefined) {
    if (state.questionType === "typing" || !answers.length) return null;
    return {
      text: String(answers[Math.floor(random() * answers.length)]),
      known: false,
    };
  }
  const wantCorrect =
    random() * 100 < instance.targetAccuracy &&
    !(settings.streakBreaker > 0 && instance.streak >= settings.streakBreaker);
  if (wantCorrect) return { text: String(correctText), known: true };
  if (state.questionType === "typing") return { text: "__skip__", known: true };
  const incorrect = answers.filter(
    (answer) => !correct.some((text) => clean(text) === clean(answer)),
  );
  if (!incorrect.length) return { text: String(correctText), known: true };
  let text = incorrect[0];
  if (settings.wrongAnswerStrategy === "random_guess")
    text = incorrect[Math.floor(random() * incorrect.length)];
  else if (settings.wrongAnswerStrategy === "smart_miss")
    text = [...incorrect].sort(
      (a, b) =>
        Math.abs(String(a).length - String(correctText).length) -
        Math.abs(String(b).length - String(correctText).length),
    )[0];
  return { text: String(text), known: true };
}
