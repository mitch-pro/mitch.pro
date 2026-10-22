/** DOM controls shared by observation and interaction. Keep selectors here so
 * detecting a new screen and acting on it cannot drift into separate adapters.
 */
export const GAME_CONTROLS = Object.freeze({
  question:
    '[class*="questionText"], [class*="QuestionText"], [class*="questionHolder"], [class*="questionContainer"], [data-testid="question"]',
  typing:
    'input[class*="typingInput"], input[placeholder*="type" i], input[placeholder*="answer" i], [data-testid="typing-answer"]',
  answers: [
    'button[class*="answer"], [role="button"][class*="answer"]',
    '[class*="answerContainer"]',
    '[class*="answerWrapper"]',
    '[class*="answersHolder"] > button, [class*="answersHolder"] > div',
    '[class*="answersWrapper"] > div',
    '[class*="answerCard"]',
    '[class*="styles__answer"]',
  ],
  feedback: [
    '[class*="nextButton"]',
    '[class*="feedbackContainer"]',
    '[class*="feedbackWrapper"]',
    '[class*="styles__feedback"]',
  ],
  chests: [
    'img[alt^="Chest "]',
    '[class*="choiceBox"]',
    '[class*="treasureChoice"]',
    '[class*="chestContainer"]',
  ],
  passwords: [
    '[class*="passwordChoice"]',
    '[class*="passwordOption"]',
    '[class*="guessTile"]',
  ],
  cryptoPlayers: ['[class*="cryptoPlayerCard"]', '[class*="hackVictim"]'],
  players: ['[class*="playerCard"]', '[class*="playerChoice"]'],
  fishing: [
    '[class*="castButton"]',
    '[class*="reelButton"]',
    '[class*="fishingButton"]',
  ],
  upgrades: [
    '[class*="upgradeCard"]',
    '[class*="perkBox"]',
    '[class*="abilityUpgrade"]',
  ],
  cafe: [
    '[class*="restockButton"]',
    '[class*="serveCustomer"]',
    '[class*="feedCustomer"]',
  ],
  cancel: ['[class*="closeButton"]', '[class*="cancelButton"]'],
});
