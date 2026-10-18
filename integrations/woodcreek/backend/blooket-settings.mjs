export const MAX_BOTS = 50;
export const DEFAULT_SETTINGS = Object.freeze({
  answerSpeedMode: "human",
  answerDelayMinMs: 1200,
  answerDelayMaxMs: 2800,
  staggerFleet: false,
  adaptiveReadingTime: false,
  accuracyRate: 95,
  scoreSpread: "uniform",
  wrongAnswerStrategy: "random_guess",
  streakBreaker: 0,
  gameMode: "gold_quest",
  chestStrategy: "random",
  swapTarget: "leader",
  cryptoTarget: "leader",
  brawlStat: "damage",
  humanKeystrokes: false,
  clickOffsets: false,
  autoMinigames: true,
  useFlareSolverr: true,
  namingStyle: "real",
  namePattern: "Player_{n}",
  nameStartNumber: 1,
  blookTheme: "random",
});
const enums = {
  answerSpeedMode: ["instant", "fast", "human", "casual", "custom"],
  scoreSpread: ["uniform", "bell_curve", "tiered"],
  wrongAnswerStrategy: ["smart_miss", "random_guess", "first_wrong"],
  gameMode: [
    "gold_quest",
    "crypto_hack",
    "fishing_frenzy",
    "monster_brawl",
    "cafe_factory",
    "tower_defense",
  ],
  chestStrategy: ["random", "esp", "best", "chest_1", "chest_2", "chest_3"],
  swapTarget: ["leader", "random", "never"],
  cryptoTarget: ["leader", "random"],
  brawlStat: ["damage", "defense", "speed"],
  namingStyle: ["real", "initials", "woodcreek", "casual", "pattern"],
  blookTheme: ["random", "wolf", "megabot", "dragon", "wizard"],
};
const numbers = {
  accuracyRate: [50, 100],
  answerDelayMinMs: [0, 15000],
  answerDelayMaxMs: [0, 15000],
  streakBreaker: [0, 1000],
  nameStartNumber: [1, 9999],
};
const booleans = [
  "autoAnswer",
  "autoMinigames",
  "staggerFleet",
  "adaptiveReadingTime",
  "humanKeystrokes",
  "clickOffsets",
  "useFlareSolverr",
];
export function boundedNumber(value, name, min, max) {
  if (
    (typeof value !== "number" && typeof value !== "string") ||
    String(value).trim() === "" ||
    !Number.isFinite(Number(value))
  )
    throw new Error(`${name} must be a number.`);
  return Math.min(max, Math.max(min, Math.round(Number(value))));
}

// Validate the whole patch before applying any of it. Launches use the same rules.
export function normalizeSettings(input, current = DEFAULT_SETTINGS) {
  if (!input || typeof input !== "object" || Array.isArray(input))
    throw new Error("Invalid game settings.");
  const patch = {};
  for (const name of booleans)
    if (input[name] !== undefined) {
      if (typeof input[name] !== "boolean")
        throw new Error(`${name} must be true or false.`);
      patch[name] = input[name];
    }
  for (const [name, allowed] of Object.entries(enums))
    if (input[name] !== undefined) {
      if (!allowed.includes(input[name])) throw new Error(`Invalid ${name}.`);
      patch[name] =
        name === "chestStrategy" && ["esp", "best"].includes(input[name])
          ? "random"
          : input[name];
    }
  for (const [name, [min, max]] of Object.entries(numbers)) {
    const value =
      name === "accuracyRate"
        ? (input.accuracy ?? input.accuracyRate)
        : input[name];
    if (value !== undefined) patch[name] = boundedNumber(value, name, min, max);
  }
  if (input.answerDelay !== undefined) {
    const delay = boundedNumber(input.answerDelay, "Answer delay", 0, 15000);
    if (input.answerDelayMinMs === undefined) patch.answerDelayMinMs = delay;
    if (input.answerDelayMaxMs === undefined) patch.answerDelayMaxMs = delay;
  }
  const min = patch.answerDelayMinMs ?? current.answerDelayMinMs;
  const max = patch.answerDelayMaxMs ?? current.answerDelayMaxMs;
  if (min > max) {
    if (
      input.answerDelayMinMs !== undefined &&
      input.answerDelayMaxMs === undefined
    )
      patch.answerDelayMaxMs = min;
    else if (
      input.answerDelayMaxMs !== undefined &&
      input.answerDelayMinMs === undefined
    )
      patch.answerDelayMinMs = max;
    else
      throw new Error(
        "Minimum answer delay must not exceed maximum answer delay.",
      );
  }
  if (input.namePattern !== undefined) {
    if (typeof input.namePattern !== "string" || !input.namePattern.trim())
      throw new Error("Enter a name pattern.");
    patch.namePattern = input.namePattern.trim().slice(0, 64);
  }
  return patch;
}

export function targetAccuracy(settings, index) {
  const offset =
    settings.scoreSpread === "bell_curve"
      ? (((index - 1) % 5) - 2) * 3
      : settings.scoreSpread === "tiered"
        ? index === 1
          ? 4
          : index >= 4
            ? -8
            : 0
        : 0;
  return Math.max(50, Math.min(100, settings.accuracyRate + offset));
}
