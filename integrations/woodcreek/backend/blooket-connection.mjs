// Blooket's browser join flow; keep page selectors in one place.
export const PLAYER_URL = "https://play.blooket.com/play";
export const PIN_SELECTOR =
  'input[placeholder*="game id" i], input[placeholder*="pin" i], input[aria-label*="game id" i], input[aria-label*="pin" i], input[name="join-code"]';
export const NAME_SELECTOR =
  'input[placeholder*="name" i], input[aria-label*="name" i], input[placeholder*="nickname" i], input[name="nickname"]';
export function normalizePin(value) {
  const pin = String(value ?? "").replace(/\s|-/g, "");
  if (!/^\d{6,9}$/.test(pin))
    throw new Error("Enter a Game PIN with 6–9 digits.");
  return pin;
}
export function visibleField(page, selector) {
  return page.locator(selector).filter({ visible: true }).first();
}
export async function submitJoinField(field) {
  const form = field.locator("xpath=ancestor::form[1]");
  const button = form
    .locator('button[type="submit"], input[type="submit"], button:not([type])')
    .filter({ visible: true })
    .first();
  // Submit exactly once. A second click can hit the next screen's button.
  if ((await button.count()) && (await button.isEnabled()))
    await button.click();
  else await field.press("Enter");
}
export async function readJoinScreen(page) {
  return page.evaluate(
    ({ pinSelector, nameSelector }) => {
      const visible = (el) =>
        Boolean(
          el &&
            el.getClientRects().length &&
            getComputedStyle(el).visibility !== "hidden",
        );
      const hasPin = [...document.querySelectorAll(pinSelector)].some(visible);
      const hasName = [...document.querySelectorAll(nameSelector)].some(
        visible,
      );
      const hasRandom = [
        ...document.querySelectorAll('button, [role="button"]'),
      ].some(
        (el) =>
          visible(el) &&
          (/generate|spin/i.test(el.innerText) ||
            String(el.className).includes("randomButton")),
      );
      const text = document.body?.innerText || "";
      const hasBlooks = [
        ...document.querySelectorAll(
          '[class*="blookBox"], [class*="blookContainer"], [class*="playerBlook"], [class*="blookWrapper"]',
        ),
      ].some(visible);
      const hasGame = [
        ...document.querySelectorAll(
          '[class*="questionText"], [class*="answersHolder"], [class*="questionHolder"], [class*="feedbackContainer"], [class*="choiceBox"]',
        ),
      ].some(visible);
      return {
        hasPin,
        hasName,
        hasRandom,
        hasBlooks,
        hasGame,
        hasLobbyTxt:
          /waiting for (the )?host|you[’']?re in|choose a blook|waiting for players/i.test(
            text,
          ),
        invalidPin:
          hasPin &&
          /invalid game (id|pin|code)|could not find (that|the) game|game (id|code) (not found|does not exist)/i.test(
            text,
          ),
        isTaken:
          (hasName || hasRandom) &&
          /name.*(taken|already in use|choose another|not allowed|inappropriate|invalid)/i.test(
            text,
          ),
        isFull:
          !hasGame &&
          /game (lobby )?is full|maximum (number of )?players/i.test(text),
        isEnded:
          !hasGame &&
          /game (has ended|is over|is no longer active)|game session has already concluded/i.test(
            text,
          ),
      };
    },
    { pinSelector: PIN_SELECTOR, nameSelector: NAME_SELECTOR },
  );
}
