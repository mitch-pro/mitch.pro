import { PIN_SELECTOR, NAME_SELECTOR } from "./blooket-connection.mjs";
import {
  ANSWER_SELECTORS,
  QUESTION_SELECTOR,
  TYPING_SELECTOR,
} from "./blooket-answers.mjs";

// Read each bot's current screen once. Visible question controls take precedence
// over words in a question that happen to resemble lobby or minigame prompts.
export async function readGameScreen(page) {
  return page.evaluate(
    ({
      pinSelector,
      nameSelector,
      answerSelectors,
      questionSelector,
      typingSelector,
    }) => {
      const visible = (element) =>
        Boolean(
          element?.getClientRects().length &&
            getComputedStyle(element).visibility !== "hidden",
        );
      const all = (selector) =>
        [...document.querySelectorAll(selector)].filter(visible);
      const has = (selector) => all(selector).length > 0;
      const bodyText = document.body?.innerText || "";
      const heading = all(questionSelector)[0];
      let fiberQuestion = null,
        stage = "";
      for (const element of [
        heading,
        ...all('[class*="answersHolder"], #app, #root'),
      ].filter(Boolean)) {
        const key = Object.keys(element).find((key) =>
          /^__react(Fiber|InternalInstance|Container)\$/.test(key),
        );
        let fiber = key && element[key];
        for (
          let depth = 0;
          fiber && depth < 80;
          depth++, fiber = fiber.return
        ) {
          const state = fiber.stateNode?.state;
          stage ||= state?.stage || fiber.memoizedProps?.stage || "";
          for (const props of [
            fiber.memoizedProps,
            fiber.stateNode?.props,
            state,
          ]) {
            if (!props || typeof props !== "object") continue;
            const question =
              typeof props.question === "object"
                ? props.question
                : typeof props.question === "string" &&
                    (props.answers || props.correctAnswers)
                  ? props
                  : props.client?.question || props.client?.state?.question;
            if (question && typeof question === "object") {
              fiberQuestion = question;
              break;
            }
          }
          if (fiberQuestion) break;
        }
        if (fiberQuestion) break;
      }
      let domAnswers = [];
      for (const selector of answerSelectors) {
        const cards = all(selector);
        if (cards.length >= 2) {
          domAnswers = cards
            .map((card) => card.innerText.trim())
            .filter(Boolean);
          if (domAnswers.length >= 2) break;
        }
      }
      const hasPinInput = has(pinSelector),
        hasNameInput = has(nameSelector);
      const typing = has(typingSelector);
      const questionText =
        heading?.innerText.trim() ||
        fiberQuestion?.question ||
        fiberQuestion?.text ||
        "";
      const visibleQuestion =
        !hasPinInput &&
        !hasNameInput &&
        Boolean((domAnswers.length >= 2 || typing) && questionText);
      const answers =
        domAnswers.length >= 2
          ? domAnswers
          : Array.isArray(fiberQuestion?.answers)
            ? fiberQuestion.answers.map(String)
            : [];
      const normalize = (value) =>
        String(value || "")
          .replace(/\s+/g, " ")
          .trim()
          .toLowerCase();
      const fiberText = fiberQuestion?.question || fiberQuestion?.text || "";
      const sameQuestion =
        !heading || normalize(fiberText) === normalize(heading.innerText);
      const correct = sameQuestion ? fiberQuestion?.correctAnswers : null;
      const correctAnswers = (
        Array.isArray(correct) ? correct : correct == null ? [] : [correct]
      ).map(String);
      const hasFeedback =
        !visibleQuestion &&
        (stage === "feedback" ||
          has(
            '[class*="feedbackContainer"], [class*="feedbackWrapper"], [class*="styles__feedback"]',
          ));
      const hasNext =
        !visibleQuestion &&
        (has('[class*="nextButton"]') ||
          /^(click|press) anywhere to (go next|continue)$/im.test(bodyText));
      const hasChests =
        !visibleQuestion &&
        has(
          'img[alt^="Chest "], [class*="choiceBox"], [class*="treasureChoice"], [class*="chestContainer"]',
        );
      const hasSwap =
        !visibleQuestion &&
        has('[class*="playerCard"], [class*="playerChoice"]');
      const hasCrypto =
        !visibleQuestion &&
        has(
          '[class*="passwordChoice"], [class*="passwordOption"], [class*="guessTile"], [class*="cryptoPlayerCard"], [class*="hackVictim"]',
        );
      const hasFish =
        !visibleQuestion &&
        has(
          '[class*="castButton"], [class*="reelButton"], [class*="fishingButton"]',
        );
      const hasPerk =
        !visibleQuestion &&
        has(
          '[class*="upgradeCard"], [class*="perkBox"], [class*="abilityUpgrade"], [class*="serveCustomer"], [class*="restockButton"]',
        );
      const hasKicked =
        !visibleQuestion &&
        /you were kicked|you have been removed|kicked from (the )?game|host removed you/i.test(
          bodyText,
        );
      const ended =
        !visibleQuestion &&
        !hasPinInput &&
        !hasNameInput &&
        (has(
          '[class*="gameOver"], [class*="finalStanding"], [class*="finalResult"]',
        ) ||
          /^(?:the )?game (?:has ended|is over|ended)[.!]?$/im.test(bodyText));
      const hasBlooks = has(
        '[class*="blookBox"], [class*="blookContainer"], [class*="playerBlook"], [class*="blookWrapper"]',
      );
      const inLobby =
        !visibleQuestion &&
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
      return {
        stage,
        questionText,
        answers,
        domAnswers,
        correctAnswers,
        questionType: typing ? "typing" : "mc",
        hasQuestion: visibleQuestion,
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
      };
    },
    {
      pinSelector: PIN_SELECTOR,
      nameSelector: NAME_SELECTOR,
      answerSelectors: ANSWER_SELECTORS,
      questionSelector: QUESTION_SELECTOR,
      typingSelector: TYPING_SELECTOR,
    },
  );
}
