import { PIN_SELECTOR, NAME_SELECTOR } from "./blooket-connection.mjs";

import { GAME_CONTROLS } from "./blooket-controls.mjs";
export const QUESTION_SELECTOR = GAME_CONTROLS.question;
export const TYPING_SELECTOR = GAME_CONTROLS.typing;
export const ANSWER_SELECTORS = GAME_CONTROLS.answers;

/** Runs in the player document. Only the visible question leaves the browser;
 * bounded React inspection supports class components and hooks without writing
 * to game state. Node identities distinguish repeated question occurrences.
 * The optional click is checked and dispatched in this same JavaScript task,
 * so a delayed command cannot select an answer on a replacement screen.
 */
export function inspectGameDocument({
  pinSelector,
  nameSelector,
  selectors,
  intent,
} = {}) {
  const visible = (element) =>
    Boolean(
      element?.getClientRects().length &&
        !element.closest('[hidden], [aria-hidden="true"]') &&
        getComputedStyle(element).visibility !== "hidden",
    );
  const all = (selector) =>
    [...document.querySelectorAll(selector)].filter(visible);
  const text = (value) => {
    if (value && typeof value === "object")
      value = value.text ?? value.answer ?? value.html ?? value.label ?? "";
    value = String(value ?? "");
    if (!/[<&]/.test(value)) return value.replace(/\s+/g, " ").trim();
    const template = document.createElement("template");
    template.innerHTML = String(value ?? "");
    return (template.content.textContent || "").replace(/\s+/g, " ").trim();
  };
  const clean = (value) => text(value).toLowerCase();
  const label = (element) =>
    text(element.innerText) ||
    element.getAttribute("aria-label") ||
    [...element.querySelectorAll("img")]
      .map((image) => image.alt || image.getAttribute("src") || "")
      .join(" ");
  const tracker = (window.__tyleriumScreen ||= {
    seed: Date.now().toString(36) + Math.random().toString(36).slice(2),
    ids: new WeakMap(),
    next: 0,
  });
  const id = (element) => {
    if (!tracker.ids.has(element)) tracker.ids.set(element, ++tracker.next);
    return tracker.ids.get(element);
  };
  const tag = (element) => {
    const token = tracker.seed + "-" + id(element);
    element.setAttribute("data-tylerium-control", token);
    return token;
  };
  const hasPinInput =
    all(pinSelector || 'input[placeholder*="game id" i]').length > 0;
  const hasNameInput =
    all(nameSelector || 'input[placeholder*="nickname" i]').length > 0;
  const headings = all(selectors.question);
  const heading =
    headings.find(
      (node) =>
        !headings.some((other) => other !== node && node.contains(other)),
    ) || headings[0];
  const input = all(selectors.typing)[0];
  let cards = [];
  for (const selector of selectors.answers) {
    const nodes = all(selector);
    const leaves = nodes.filter(
      (node) => !nodes.some((other) => other !== node && node.contains(other)),
    );
    if (leaves.length >= 2 && leaves.length <= 8) {
      cards = leaves;
      break;
    }
  }
  const domAnswers = cards.map(label);
  const questionText = heading ? label(heading) : "";
  const normalizedAnswers = domAnswers.map(clean).sort();
  const questions = [],
    banks = new Set(),
    seen = new Set();
  let stage = "",
    occurrence = "";
  const collect = (value, depth = 0) => {
    if (!value || typeof value !== "object" || seen.has(value) || depth > 4)
      return;
    seen.add(value);
    if (Array.isArray(value)) {
      banks.add(value);
      return;
    }
    if (typeof value.stage === "string") stage ||= value.stage;
    for (const key of [
      "questionIndex",
      "currentQuestionIndex",
      "questionNumber",
      "qIndex",
    ])
      if (typeof value[key] === "number") occurrence ||= key + ":" + value[key];
    if (
      (typeof value.question === "string" || typeof value.text === "string") &&
      (Array.isArray(value.answers) ||
        value.correctAnswers != null ||
        value.correctAnswer != null)
    )
      questions.push(value);
    for (const key of [
      "question",
      "currentQuestion",
      "client",
      "state",
      "current",
    ])
      collect(value[key], depth + 1);
    if (Array.isArray(value.questions)) banks.add(value.questions);
  };
  const roots = new Set();
  for (const start of [
    heading,
    input,
    ...cards.slice(0, 2),
    document.querySelector("#app"),
    document.querySelector("#root"),
  ]) {
    for (
      let element = start, depth = 0;
      element && depth < 12;
      element = element.parentElement, depth++
    )
      roots.add(element);
  }
  const fibers = new Set();
  for (const element of roots) {
    for (const key of Object.keys(element).filter((key) =>
      /^__react(Fiber|InternalInstance|Container)\$/.test(key),
    )) {
      let fiber = element[key]?.current || element[key];
      for (
        let depth = 0;
        fiber && depth < 80 && !fibers.has(fiber);
        depth++, fiber = fiber.return
      ) {
        fibers.add(fiber);
        for (const value of [
          fiber.memoizedProps,
          fiber.stateNode?.props,
          fiber.stateNode?.state,
        ])
          collect(value);
        for (
          let hook = fiber.memoizedState, n = 0;
          hook && n < 30;
          hook = hook.next, n++
        )
          collect(hook.memoizedState);
      }
    }
  }
  const matches = (question) => {
    if (!question || typeof question !== "object") return false;
    if (
      !questionText ||
      clean(question.question || question.text) !== clean(questionText)
    )
      return false;
    if (!cards.length) return true;
    const answers = Array.isArray(question.answers)
      ? question.answers.map(clean).sort()
      : [];
    return (
      answers.length === normalizedAnswers.length &&
      answers.every((answer, index) => answer === normalizedAnswers[index])
    );
  };
  let matching = questions.filter(matches);
  if (
    !matching.some(
      (question) =>
        question.correctAnswers?.length || question.correctAnswer != null,
    )
  ) {
    matching = [...banks]
      .flatMap((bank) => bank.slice(0, 5000))
      .filter(matches);
  }
  const answerKeys = matching.map((question) => {
    const raw = question.correctAnswers ?? question.correctAnswer ?? [];
    return (Array.isArray(raw) ? raw : [raw]).map(text);
  });
  const correctAnswers =
    answerKeys.length &&
    answerKeys.every(
      (key) =>
        JSON.stringify(key.map(clean).sort()) ===
        JSON.stringify(answerKeys[0].map(clean).sort()),
    )
      ? answerKeys[0]
      : [];
  const feedback = all(selectors.feedback.join(","));
  const next = all('button, [role="button"], div, p, span').find((element) =>
    /^(?:continue|go next|next question|claim|collect|(?:click|press) anywhere to (?:continue|go next|cast))$/i.test(
      label(element),
    ),
  );
  const hasFeedback =
    feedback.length > 0 || Boolean(next && stage === "feedback");
  const terminalUi =
    all('[class*="gameOver"], [class*="finalStanding"], [class*="finalResult"]')
      .length > 0;
  const removalNotice = all(
    '[role="alert"], [role="dialog"], [class*="kicked"], [class*="removed"]',
  ).some(
    (element) =>
      !heading?.contains(element) &&
      /you were kicked|you have been removed|host removed you/i.test(
        label(element),
      ),
  );
  const hasQuestion =
    !hasPinInput &&
    !hasNameInput &&
    !hasFeedback &&
    !terminalUi &&
    !removalNotice &&
    Boolean(questionText && (cards.length >= 2 || input));
  const hasNext = !hasQuestion && Boolean(next);
  const has = (selector) => all(selector).length > 0;
  const bodyText = document.body?.innerText || "";
  const hasKicked =
    removalNotice ||
    (!hasQuestion &&
      /you were kicked|you have been removed|kicked from (the )?game|host removed you/i.test(
        bodyText,
      ));
  const ended =
    !hasQuestion &&
    !hasPinInput &&
    !hasNameInput &&
    (has(
      '[class*="gameOver"], [class*="finalStanding"], [class*="finalResult"]',
    ) ||
      /^(?:the )?game (?:has ended|is over|ended)[.!]?$/im.test(bodyText));
  const hasBlooks = has(
    '[class*="blookBox"], [class*="blookContainer"], [class*="playerBlook"], [class*="blookWrapper"]',
  );
  const hasChests = !hasQuestion && has(selectors.chests.join(","));
  const hasSwap = !hasQuestion && has(selectors.players.join(","));
  const hasCrypto =
    !hasQuestion &&
    has([...selectors.passwords, ...selectors.cryptoPlayers].join(","));
  const hasFish = !hasQuestion && has(selectors.fishing.join(","));
  const hasPerk =
    !hasQuestion && has([...selectors.upgrades, ...selectors.cafe].join(","));
  const inLobby =
    !hasQuestion &&
    !hasFeedback &&
    !hasNext &&
    !hasChests &&
    !hasSwap &&
    !hasCrypto &&
    !hasFish &&
    !hasPerk &&
    !hasPinInput &&
    !hasNameInput &&
    !hasKicked &&
    !ended &&
    (hasBlooks ||
      /waiting for (the )?host|waiting for players|choose a blook/i.test(
        bodyText,
      ));
  const questionKey = hasQuestion
    ? [
        tracker.seed,
        occurrence,
        clean(questionText),
        ...cards.map((element) => id(element) + ":" + clean(label(element))),
        input ? id(input) : "",
      ].join("|")
    : null;
  const result = {
    stage,
    questionText,
    questionKey,
    answers: domAnswers,
    domAnswers,
    correctAnswers,
    questionType: input ? "typing" : "mc",
    hasQuestion,
    hasPinInput,
    hasNameInput,
    hasKicked,
    hasBlooks,
    inLobby,
    ended,
    hasFeedback,
    hasNext,
    hasChests,
    hasSwap,
    hasCrypto,
    hasFish,
    hasPerk,
    hasChestReward: false,
    isTransitionScreen: hasFeedback || hasNext,
    screenKey: [
      tracker.seed,
      ...[
        ...new Set([
          ...all(
            Object.entries(selectors)
              .filter(
                ([key]) => !["question", "typing", "answers"].includes(key),
              )
              .flatMap(([, value]) => value)
              .join(","),
          ),
          ...(next ? [next] : []),
        ]),
      ].map(
        (element) =>
          id(element) +
          ":" +
          label(element) +
          ":" +
          element.className +
          ":" +
          element.getAttribute("aria-disabled"),
      ),
    ].join("|"),
    answerTargets: cards.map(tag),
    inputTarget: input ? tag(input) : null,
  };
  if (intent) {
    const target = cards[intent.index];
    if (
      !hasQuestion ||
      questionKey !== intent.questionKey ||
      !target ||
      clean(label(target)) !== clean(intent.text) ||
      target.matches(':disabled, [aria-disabled="true"]')
    )
      return null;
    target.scrollIntoView({
      block: "center",
      inline: "nearest",
      behavior: "instant",
    });
    const box = target.getBoundingClientRect();
    const hit = document.elementFromPoint(
      box.left + box.width / 2,
      box.top + box.height / 2,
    );
    if (!hit || !(target === hit || target.contains(hit))) return null;
    const submitted = { text: label(target), questionKey };
    target.click();
    return submitted;
  }
  return result;
}
export const inspectOptions = () => ({
  pinSelector: PIN_SELECTOR,
  nameSelector: NAME_SELECTOR,
  selectors: GAME_CONTROLS,
});
export const readGameScreen = (page) =>
  page.evaluate(inspectGameDocument, inspectOptions());
