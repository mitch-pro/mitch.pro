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
    return click(
      [
        'img[alt^="Chest "]',
        '[class*="choiceBox"]',
        '[class*="treasureChoice"]',
        '[class*="chestContainer"]',
      ],
      index,
    );
  }
  if (state.hasCrypto) {
    if (
      await click(
        [
          '[class*="passwordChoice"]',
          '[class*="passwordOption"]',
          '[class*="guessTile"]',
        ],
        "random",
      )
    )
      return true;
    return click(
      ['[class*="cryptoPlayerCard"]', '[class*="hackVictim"]'],
      settings.cryptoTarget === "random" ? "random" : 0,
    );
  }
  if (state.hasSwap) {
    if (settings.swapTarget === "never")
      return click(['[class*="closeButton"]', '[class*="cancelButton"]']);
    return click(
      ['[class*="playerCard"]', '[class*="playerChoice"]'],
      settings.swapTarget === "random" ? "random" : 0,
    );
  }
  if (state.hasFish)
    return click([
      '[class*="castButton"]',
      '[class*="reelButton"]',
      '[class*="fishingButton"]',
    ]);
  if (state.hasPerk) {
    const index = { defense: 1, speed: 2 }[settings.brawlStat] ?? 0;
    if (
      await click(
        [
          '[class*="upgradeCard"]',
          '[class*="perkBox"]',
          '[class*="abilityUpgrade"]',
        ],
        index,
      )
    )
      return true;
    return click([
      '[class*="restockButton"]',
      '[class*="serveCustomer"]',
      '[class*="feedCustomer"]',
    ]);
  }
  return false;
}
