import {
  PIN_SELECTOR,
  NAME_SELECTOR,
  visibleField,
  submitJoinField,
} from "./blooket-connection.mjs";
export const QUESTION_SELECTOR =
  '[class*="questionText"], [class*="QuestionText"], [class*="questionHolder"], [class*="questionContainer"]';
export const TYPING_SELECTOR =
  'input[class*="typingInput"], input[placeholder*="type" i], input[placeholder*="answer" i]';
export const ANSWER_SELECTORS = [
  '[class*="answerContainer"]',
  '[class*="answerWrapper"]',
  '[class*="answersHolder"] > div',
  '[class*="answersHolder"] > button',
  '[class*="answersWrapper"] > div',
  '[class*="answerCard"]',
  '[role="button"][class*="answer"]',
];
const clean = (value) =>
  String(value ?? "")
    .replace(/\s+/g, " ")
    .trim()
    .toLowerCase();

// Submit through the visible game control once; never click the next screen blindly.
export async function submitAnswer(page, state, choice, active = () => true) {
  if (!active()) return null;
  if (
    (await visibleField(page, PIN_SELECTOR).count()) ||
    (await visibleField(page, NAME_SELECTOR).count())
  )
    return null;
  const heading = page
    .locator(QUESTION_SELECTOR)
    .filter({ visible: true })
    .first();
  if (await heading.count()) {
    const current = clean(await heading.innerText());
    if (current && state.questionText && current !== clean(state.questionText))
      return null;
  }
  if (state.questionType === "typing") {
    const input = page
      .locator(TYPING_SELECTOR)
      .filter({ visible: true })
      .first();
    if (!(await input.count())) return null;
    if (!active()) return null;
    await input.fill(choice.humanKeystrokes ? "" : choice.text);
    if (choice.humanKeystrokes)
      for (const char of choice.text) {
        if (!active()) return null;
        await input.pressSequentially(char, {
          delay: 40 + Math.floor(Math.random() * 50),
        });
      }
    if (!active()) return null;
    await submitJoinField(input);
    return { text: choice.text };
  }
  for (const selector of ANSWER_SELECTORS) {
    const cards = page.locator(selector).filter({ visible: true });
    const count = await cards.count();
    if (count < 2) continue;
    const texts = await cards.allInnerTexts();
    const index = texts.findIndex((text) => clean(text) === clean(choice.text));
    if (index < 0) continue;
    if (!active()) return null;
    const card = cards.nth(index),
      box = choice.clickOffsets ? await card.boundingBox() : null;
    if (!active()) return null;
    await card.click({
      timeout: 2000,
      ...(box
        ? {
            position: {
              x: box.width * (0.3 + Math.random() * 0.4),
              y: box.height * (0.3 + Math.random() * 0.4),
            },
          }
        : {}),
    });
    return { text: texts[index].trim() };
  }
  return null;
}

export async function advanceFeedback(page, active = () => true) {
  if (!active()) return false;
  for (const selector of [
    '[class*="nextButton"]',
    '[class*="feedbackContainer"]',
    '[class*="feedbackWrapper"]',
    '[class*="feedback"]',
  ]) {
    const control = page.locator(selector).filter({ visible: true }).first();
    if ((await control.count()) && active()) {
      await control.click({ timeout: 1500 });
      return true;
    }
  }
  const prompt = page
    .getByText(
      /^(?:click|press) anywhere (?:to (?:continue|go next)|to cast)|^(?:continue|go next|claim|collect)$/i,
    )
    .filter({ visible: true })
    .first();
  if ((await prompt.count()) && active()) {
    await prompt.click({ timeout: 1500 });
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
