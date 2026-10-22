import {
  PIN_SELECTOR,
  NAME_SELECTOR,
  visibleField,
  submitJoinField,
  readJoinScreen,
} from "./blooket-connection.mjs";

/** Each join form is submitted once. Only an explicit nickname rejection permits
 * another submission; an uncertain response never silently replaces a player.
 */
export async function joinGame(
  page,
  instance,
  { active, phase, freshName, timeoutMs = 14000 } = {},
) {
  const checkActive = () => {
    if (!active()) throw new Error("Connection cancelled.");
  };
  const rejectTerminal = (state) => {
    if (state.isFull)
      throw new Error("Game lobby is full (maximum player capacity reached).");
    if (state.isEnded) throw new Error("Game session has already concluded.");
    if (state.invalidPin)
      throw new Error("Invalid Game PIN: Blooket could not find that game.");
  };
  async function waitFor(read, message) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      checkActive();
      const state = await readJoinScreen(page);
      rejectTerminal(state);
      const result = read(state);
      if (result) return result;
      await page.waitForTimeout(100);
    }
    throw new Error(message);
  }
  phase("PHASE_PIN_INPUT", "entering_pin", "Submitting Game PIN.");
  const pinInput = visibleField(page, PIN_SELECTOR);
  await pinInput.waitFor({ state: "visible", timeout: 20000 });
  checkActive();
  await pinInput.fill(instance.pin);
  checkActive();
  await submitJoinField(pinInput);
  const nameScreen = await waitFor(
    (state) => (state.hasName || state.hasRandom ? state : null),
    "The nickname screen did not load. Check the PIN and try again.",
  );
  let rejectedBefore = false;
  for (let attempt = 0; attempt < 4; attempt++) {
    checkActive();
    phase("PHASE_NAME_INPUT", "entering_name", `Joining as ${instance.name}.`);
    if (nameScreen.hasRandom && !nameScreen.hasName) {
      const generate = page
        .getByRole("button", { name: /generate|spin/i })
        .filter({ visible: true })
        .first();
      await generate.click({ timeout: 2000 });
      const join = page
        .getByRole("button", { name: /^(join|let.s go|play)$/i })
        .filter({ visible: true })
        .first();
      await join.waitFor({ state: "visible", timeout: 5000 });
      checkActive();
      instance.joinSubmitted = true;
      await join.click();
    } else {
      const input = visibleField(page, NAME_SELECTOR);
      await input.fill(instance.name);
      checkActive();
      instance.joinSubmitted = true;
      await submitJoinField(input);
    }
    let sawClear = !rejectedBefore;
    const outcome = await waitFor((state) => {
      if (
        !state.hasPin &&
        !state.hasName &&
        (state.hasLobbyTxt ||
          state.hasBlooks ||
          state.hasGame ||
          /\/lobby(?:\/|$)/.test(new URL(page.url()).pathname))
      )
        return { joined: true, hasGame: state.hasGame };
      if (!state.isTaken) sawClear = true;
      if (state.isTaken && sawClear) return { rejected: true };
      return null;
    }, "Blooket did not confirm the join. Check the game before retrying.");
    if (outcome.joined) return outcome;
    rejectedBefore = true;
    instance.name = freshName();
  }
  throw new Error("Nickname rejected. Choose another name and retry.");
}
