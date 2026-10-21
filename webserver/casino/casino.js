(() => {
  const format = new Intl.NumberFormat(undefined, { maximumFractionDigits: 4 });
  const balanceEl = document.getElementById('casino-balance');
  const disabledNote = document.getElementById('casino-disabled-note');
  let casinoEnabled = true;

  // Deliberately plain `fetch`, never fetchWithRetry: a bet is not
  // idempotent, so auto-retrying a request whose response got lost after
  // the server already settled it would risk the client thinking a bet
  // never landed and the player trying again. A dropped response here
  // just shows an error; the balance refresh after it reveals the truth.
  async function postCasino(url, body) {
    const res = await fetch(url, {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body || {}),
    });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(describeError(data.error) || 'Something went wrong.');
    return data;
  }

  function describeError(code) {
    switch (code) {
      case 'casino_disabled': return 'The casino is temporarily disabled.';
      case 'unauthorized': return 'Sign in to play.';
      case 'invalid_bet': return 'Enter a bet between 1 and 5000.';
      case 'insufficient_funds': return 'You don’t have enough coins for that bet.';
      case 'game_in_progress': return 'Finish your current hand first.';
      case 'no_active_game': return 'Start a new hand first.';
      default: return '';
    }
  }

  function setBalance(n) {
    if (Number.isFinite(Number(n))) balanceEl.textContent = format.format(Number(n));
  }

  async function loadBalance() {
    try {
      const res = await fetch('/api/me/coins', { credentials: 'same-origin', cache: 'no-store' });
      const data = await res.json();
      if (data.authenticated === false) { balanceEl.textContent = 'Sign in to play'; return; }
      setBalance(data.coins);
    } catch { balanceEl.textContent = '—'; }
  }

  async function loadStatus() {
    try {
      const res = await fetch('/api/casino/status', { credentials: 'same-origin', cache: 'no-store' });
      const data = await res.json();
      casinoEnabled = !!data.enabled;
      disabledNote.hidden = casinoEnabled;
      document.querySelectorAll('.casino-bet-form button, .casino-actions button').forEach(b => { b.disabled = !casinoEnabled; });
    } catch { /* leave default-enabled UI; the bet attempt itself will surface any real error */ }
  }

  function setResult(el, text, cls) {
    el.textContent = text;
    el.className = 'casino-result' + (cls ? ' ' + cls : '');
  }

  // ---- Tabs ----
  document.querySelectorAll('.casino-tab').forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('.casino-tab').forEach(t => { t.classList.remove('is-active'); t.setAttribute('aria-selected', 'false'); });
      tab.classList.add('is-active');
      tab.setAttribute('aria-selected', 'true');
      document.querySelectorAll('.casino-panel').forEach(p => { p.classList.remove('is-active'); p.hidden = true; });
      const panel = document.getElementById('panel-' + tab.dataset.tab);
      panel.hidden = false;
      panel.classList.add('is-active');
    });
  });

  // ---- Coinflip ----
  const coinflipForm = document.getElementById('coinflip-form');
  const coinflipResult = document.getElementById('coinflip-result');
  let coinflipChoice = 'heads';
  coinflipForm.querySelectorAll('.casino-choice').forEach(btn => {
    btn.addEventListener('click', () => {
      coinflipForm.querySelectorAll('.casino-choice').forEach(b => b.classList.remove('is-active'));
      btn.classList.add('is-active');
      coinflipChoice = btn.dataset.choice;
    });
  });
  coinflipForm.addEventListener('submit', async e => {
    e.preventDefault();
    const bet = Number(document.getElementById('coinflip-bet').value);
    setResult(coinflipResult, 'Flipping…');
    try {
      const d = await postCasino('/api/casino/coinflip', { bet, choice: coinflipChoice });
      setBalance(d.balance);
      setResult(
        coinflipResult,
        d.won ? `You won ${format.format(d.payout)} coins!` : 'No luck this time.',
        d.won ? 'is-win' : 'is-loss',
      );
    } catch (err) { setResult(coinflipResult, err.message, 'is-err'); }
  });

  // ---- Dice ----
  const diceForm = document.getElementById('dice-form');
  const diceResult = document.getElementById('dice-result');
  const diceTarget = document.getElementById('dice-target');
  const diceTargetValue = document.getElementById('dice-target-value');
  const diceOdds = document.getElementById('dice-odds');
  let diceDirection = 'under';
  function updateDiceOdds() {
    const target = Number(diceTarget.value);
    diceTargetValue.textContent = target;
    const p = (diceDirection === 'under' ? (target - 1) : (100 - target)) / 100;
    const clamped = Math.min(Math.max(p, 0.01), 0.99);
    const multiplier = 0.96 / clamped;
    diceOdds.textContent = `Win chance ~${Math.round(clamped * 100)}% → pays ${multiplier.toFixed(2)}×`;
  }
  diceTarget.addEventListener('input', updateDiceOdds);
  diceForm.querySelectorAll('.casino-choice').forEach(btn => {
    btn.addEventListener('click', () => {
      diceForm.querySelectorAll('.casino-choice').forEach(b => b.classList.remove('is-active'));
      btn.classList.add('is-active');
      diceDirection = btn.dataset.direction;
      updateDiceOdds();
    });
  });
  updateDiceOdds();
  diceForm.addEventListener('submit', async e => {
    e.preventDefault();
    const bet = Number(document.getElementById('dice-bet').value);
    const target = Number(diceTarget.value);
    setResult(diceResult, 'Rolling…');
    try {
      const d = await postCasino('/api/casino/dice', { bet, target, direction: diceDirection });
      setBalance(d.balance);
      setResult(
        diceResult,
        `Rolled ${d.roll}. ` + (d.won ? `You won ${format.format(d.payout)} coins!` : 'No luck this time.'),
        d.won ? 'is-win' : 'is-loss',
      );
    } catch (err) { setResult(diceResult, err.message, 'is-err'); }
  });

  // ---- Slots ----
  const slotsForm = document.getElementById('slots-form');
  const slotsResult = document.getElementById('slots-result');
  const slotsReels = document.getElementById('slots-reels');
  const SLOT_EMOJI = { cherry: '🍒', lemon: '🍋', bell: '🔔', clover: '🍀', star: '⭐', seven: '7️⃣' };
  slotsForm.addEventListener('submit', async e => {
    e.preventDefault();
    const bet = Number(document.getElementById('slots-bet').value);
    setResult(slotsResult, 'Spinning…');
    try {
      const d = await postCasino('/api/casino/slots', { bet });
      setBalance(d.balance);
      slotsReels.innerHTML = d.reels.map(s => `<span>${SLOT_EMOJI[s] || '?'}</span>`).join('');
      setResult(
        slotsResult,
        d.won ? `You won ${format.format(d.payout)} coins!` : 'No luck this time.',
        d.won ? 'is-win' : 'is-loss',
      );
    } catch (err) { setResult(slotsResult, err.message, 'is-err'); }
  });

  // ---- Shared card rendering ----
  const RED_SUITS = new Set(['H', 'D']);
  function renderCard(card) {
    const span = document.createElement('span');
    span.className = 'casino-card ' + (RED_SUITS.has(card.suit) ? 'is-red' : 'is-black');
    const suitGlyph = { S: '♠', H: '♥', D: '♦', C: '♣' }[card.suit] || card.suit;
    span.textContent = card.rank + suitGlyph;
    return span;
  }

  // ---- Blackjack ----
  const bjBetForm = document.getElementById('blackjack-bet-form');
  const bjHands = document.getElementById('blackjack-hands');
  const bjDealerCards = document.getElementById('blackjack-dealer-cards');
  const bjPlayerCards = document.getElementById('blackjack-player-cards');
  const bjPlayerValue = document.getElementById('blackjack-player-value');
  const bjActions = document.getElementById('blackjack-actions');
  const bjResult = document.getElementById('blackjack-result');
  const bjHitBtn = document.getElementById('blackjack-hit');
  const bjStandBtn = document.getElementById('blackjack-stand');

  // Display-only mirror of the server's ace-reduction scoring (the server
  // is always the one that actually settles money) so the end-of-round
  // view can show both final hands' totals.
  function handValue(cards) {
    let total = 0, aces = 0;
    for (const c of cards) {
      if (c.rank === 'A') { total += 11; aces++; }
      else if (['J', 'Q', 'K'].includes(c.rank)) total += 10;
      else total += Number(c.rank);
    }
    while (total > 21 && aces > 0) { total -= 10; aces--; }
    return total;
  }

  function renderBlackjack(player, dealerCards, value) {
    bjPlayerCards.replaceChildren(...player.map(renderCard));
    bjDealerCards.replaceChildren(...dealerCards.map(renderCard));
    bjPlayerValue.textContent = value;
    bjHands.hidden = false;
  }

  function endBlackjackRound(d) {
    renderBlackjack(d.player, d.dealer, handValue(d.player));
    bjActions.hidden = true;
    bjBetForm.querySelector('button').disabled = false;
    const labels = {
      player_blackjack: 'Blackjack! You won',
      player_wins: 'You won',
      dealer_bust: 'Dealer busts — you won',
      player_bust: 'You busted.',
      dealer_wins: 'Dealer wins.',
      push: 'Push — bet returned.',
    };
    const cls = d.result === 'push' ? '' : (d.payout > d.bet ? 'is-win' : 'is-loss');
    setResult(
      bjResult,
      `${labels[d.result] || d.result}` + (d.payout > 0 && d.result !== 'push' ? ` ${format.format(d.payout)} coins!` : ''),
      cls,
    );
  }

  bjBetForm.addEventListener('submit', async e => {
    e.preventDefault();
    const bet = Number(document.getElementById('blackjack-bet').value);
    setResult(bjResult, 'Dealing…');
    bjBetForm.querySelector('button').disabled = true;
    try {
      const d = await postCasino('/api/casino/blackjack/start', { bet });
      setBalance(d.balance ?? NaN);
      if (d.resolved) {
        d.bet = bet;
        endBlackjackRound(d);
      } else {
        renderBlackjack(d.player, [d.dealerUpCard], d.value);
        bjActions.hidden = false;
        setResult(bjResult, '');
      }
    } catch (err) {
      setResult(bjResult, err.message, 'is-err');
      bjBetForm.querySelector('button').disabled = false;
    }
  });

  bjHitBtn.addEventListener('click', async () => {
    try {
      const d = await postCasino('/api/casino/blackjack/hit', {});
      if (d.resolved) {
        setBalance(d.balance);
        const bet = Number(document.getElementById('blackjack-bet').value);
        d.bet = bet;
        endBlackjackRound(d);
      } else {
        bjPlayerCards.replaceChildren(...d.player.map(renderCard));
        bjPlayerValue.textContent = d.value;
      }
    } catch (err) { setResult(bjResult, err.message, 'is-err'); }
  });

  bjStandBtn.addEventListener('click', async () => {
    try {
      const d = await postCasino('/api/casino/blackjack/stand', {});
      setBalance(d.balance);
      const bet = Number(document.getElementById('blackjack-bet').value);
      d.bet = bet;
      endBlackjackRound(d);
    } catch (err) { setResult(bjResult, err.message, 'is-err'); }
  });

  // ---- Poker ----
  const pokerBetForm = document.getElementById('poker-bet-form');
  const pokerHandEl = document.getElementById('poker-hand');
  const pokerActions = document.getElementById('poker-actions');
  const pokerResult = document.getElementById('poker-result');
  const pokerDrawBtn = document.getElementById('poker-draw');
  let pokerHold = [false, false, false, false, false];

  function renderPokerHand(hand, interactive) {
    pokerHandEl.innerHTML = '';
    hand.forEach((card, i) => {
      const el = renderCard(card);
      const tag = document.createElement('small');
      tag.textContent = pokerHold[i] ? 'held' : '';
      el.appendChild(tag);
      el.classList.toggle('is-held', pokerHold[i]);
      if (interactive) {
        el.addEventListener('click', () => {
          pokerHold[i] = !pokerHold[i];
          renderPokerHand(hand, true);
        });
      }
      pokerHandEl.appendChild(el);
    });
    pokerHandEl.hidden = false;
  }

  const POKER_RANK_LABELS = {
    royal_flush: 'Royal Flush', straight_flush: 'Straight Flush', four_of_a_kind: 'Four of a Kind',
    full_house: 'Full House', flush: 'Flush', straight: 'Straight', three_of_a_kind: 'Three of a Kind',
    two_pair: 'Two Pair', jacks_or_better: 'Jacks or Better', high_card: 'No Pair',
  };

  pokerBetForm.addEventListener('submit', async e => {
    e.preventDefault();
    const bet = Number(document.getElementById('poker-bet').value);
    setResult(pokerResult, 'Dealing…');
    pokerBetForm.querySelector('button').disabled = true;
    try {
      const d = await postCasino('/api/casino/poker/start', { bet });
      pokerHold = [false, false, false, false, false];
      renderPokerHand(d.hand, true);
      pokerActions.hidden = false;
      setResult(pokerResult, 'Tap cards to hold them, then draw.');
    } catch (err) {
      setResult(pokerResult, err.message, 'is-err');
      pokerBetForm.querySelector('button').disabled = false;
    }
  });

  pokerDrawBtn.addEventListener('click', async () => {
    try {
      const d = await postCasino('/api/casino/poker/draw', { hold: pokerHold });
      setBalance(d.balance);
      renderPokerHand(d.hand, false);
      pokerActions.hidden = true;
      pokerBetForm.querySelector('button').disabled = false;
      const label = POKER_RANK_LABELS[d.rank] || d.rank;
      setResult(
        pokerResult,
        d.payout > 0 ? `${label} — ${format.format(d.payout)} coins!` : `${label}. No payout.`,
        d.payout > 0 ? 'is-win' : 'is-loss',
      );
    } catch (err) { setResult(pokerResult, err.message, 'is-err'); }
  });

  // Recovers an in-progress Blackjack/Poker hand after a reload — the bet
  // is already placed server-side and there's no other way back in,
  // since starting a new hand is refused while one is active.
  async function resumeBlackjack() {
    try {
      const res = await fetch('/api/casino/blackjack/state', { credentials: 'same-origin', cache: 'no-store' });
      const d = await res.json();
      if (!d.inProgress) return;
      document.getElementById('blackjack-bet').value = d.bet;
      bjBetForm.querySelector('button').disabled = true;
      renderBlackjack(d.player, [d.dealerUpCard], d.value);
      bjActions.hidden = false;
    } catch { /* no recoverable hand */ }
  }

  async function resumePoker() {
    try {
      const res = await fetch('/api/casino/poker/state', { credentials: 'same-origin', cache: 'no-store' });
      const d = await res.json();
      if (!d.inProgress) return;
      document.getElementById('poker-bet').value = d.bet;
      pokerBetForm.querySelector('button').disabled = true;
      pokerHold = [false, false, false, false, false];
      renderPokerHand(d.hand, true);
      pokerActions.hidden = false;
      setResult(pokerResult, 'Tap cards to hold them, then draw.');
    } catch { /* no recoverable hand */ }
  }

  loadBalance();
  loadStatus();
  resumeBlackjack();
  resumePoker();
  document.addEventListener('visibilitychange', () => { if (!document.hidden) { loadBalance(); loadStatus(); } });
})();
