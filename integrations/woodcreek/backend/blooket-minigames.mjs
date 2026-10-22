import { GAME_CONTROLS } from "./blooket-controls.mjs";
// One visible action per poll. Never follow a click with a coordinate or key burst.
export async function playMinigame(page, state, settings, active = () => true) {
  if (!active() || state.hasQuestion) return false;
  async function click(selectors, index = 0) {
    for (const selector of selectors) {
      const controls = page.locator(selector).filter({ visible: true });
      const count = await controls.count();
      if (!count) continue;
      const chosen =
        index === "random"
          ? Math.floor(Math.random() * count)
          : Math.min(index, count - 1);
      if (!active()) return false;
      const control = controls.nth(chosen);
      if (!(await control.isEnabled()) || !active()) return false;
      await control.click({ timeout: 1500 });
      return true;
    }
    return false;
  }
  if (state.hasChests) {
    const index =
      { chest_1: 0, chest_2: 1, chest_3: 2 }[settings.chestStrategy] ??
      "random";
    return click(GAME_CONTROLS.chests, index);
  }
  if (state.hasCrypto) {
    if (await click(GAME_CONTROLS.passwords, "random")) return true;
    return click(
      GAME_CONTROLS.cryptoPlayers,
      settings.cryptoTarget === "random" ? "random" : 0,
    );
  }
  if (state.hasSwap) {
    if (settings.swapTarget === "never") return click(GAME_CONTROLS.cancel);
    return click(
      GAME_CONTROLS.players,
      settings.swapTarget === "random" ? "random" : 0,
    );
  }
  if (state.hasFish) return click(GAME_CONTROLS.fishing);
  if (state.hasPerk) {
    const index = { defense: 1, speed: 2 }[settings.brawlStat] ?? 0;
    if (await click(GAME_CONTROLS.upgrades, index)) return true;
    return click(GAME_CONTROLS.cafe);
  }
  return false;
}
