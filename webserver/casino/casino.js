(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const games = {
    flip: { name:'Coin flip', choices:[['heads','Heads'],['tails','Tails']], prompt:'Heads or tails', rules:'Pick a side. Each side has a 50% chance of landing. A winning flip pays 1.95 times your bet.', payout:'1.95× your bet', edge:'2.5%' },
    dice: { name:'Dice', choices:[['under','Under 7'],['over','Over 7']], prompt:'Under or over seven', rules:'Two six-sided dice are rolled. Choose a total under or over seven. A total of seven loses either bet.', payout:'2.3× your bet', edge:'4.17%' },
    roulette: { name:'Roulette', choices:[['red','Red'],['black','Black']], prompt:'Red or black', rules:'Pick red or black on a single-zero wheel. There are 18 of each color. Green zero loses either bet.', payout:'2× your bet', edge:'2.70%' }
  };
  let game = 'flip', busy = false, ready = false, pending = null;
  const fmt = n => Number(n).toLocaleString(undefined,{ maximumFractionDigits:4 });
  const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
  function lock() {
    $('play').disabled = busy || !ready || !!pending;
    document.querySelectorAll('#games button,#choices input,#stake').forEach(el => { el.disabled = busy || !!pending; });
    $('retry').disabled = busy; $('retry').hidden = !pending;
  }
  function status(text, error = false) { $('status').textContent = text; $('status').dataset.error = error; }
  function select(key) {
    if (busy || pending) return;
    game = key; const info = games[game];
    $('game-title').textContent = info.name; $('result').textContent = info.prompt; $('result').removeAttribute('data-win');
    $('result-detail').textContent = 'Choose a side to play.';
    for (const id of ['rules','payout','edge']) $(id).textContent = info[id];
    $('choices').innerHTML = '<legend>Your choice</legend>' + info.choices.map(([value,label],i) => `<label class="choice"><input type="radio" name="choice" value="${value}" ${i===0?'checked':''} required>${label}</label>`).join('');
    document.querySelectorAll('#games button').forEach(btn => btn.setAttribute('aria-pressed', String(btn.dataset.game === key)));
  }
  async function refresh() {
    const response = await fetch('/api/casino/state', { credentials:'same-origin',cache:'no-store' });
    const data = await response.json(); if (!response.ok) throw new Error(data.error || 'Could not load your balance.');
    $('balance').textContent = fmt(data.balance); window.dispatchEvent(new Event('mitch:coins-refresh')); ready = true;
    $('history').innerHTML = data.rounds.map(round => `<tr><td>${esc(new Date(round.ts).toLocaleTimeString([],{hour:'numeric',minute:'2-digit'}))}</td><td>${esc(games[round.game]?.name || round.game)}</td><td>${esc(round.choice)}</td><td>${esc(round.outcome)}</td><td>${fmt(round.stake)}</td><td class="net-${round.net>0?'positive':'negative'}">${round.net>0?'+':''}${fmt(round.net)}</td></tr>`).join('') || '<tr><td colspan="6">No rounds played yet.</td></tr>';
    lock();
  }
  async function play() {
    if (busy || !pending) return;
    busy = true; lock(); status('Settling your round…');
    try {
      const response = await fetch('/api/casino/play', {method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json','X-Mitch-Requested-With':'1'},body:JSON.stringify(pending)});
      const data = await response.json();
      if (!response.ok) { if (response.status >= 400 && response.status < 500 && response.status !== 429) pending = null; throw new Error(data.error || 'Could not confirm this round. Check it before placing another bet.'); }
      const round = data.round; pending = null;
      $('result').textContent = round.outcome; $('result').dataset.win = round.net > 0;
      $('result-detail').textContent = `${round.net>0?'+':''}${fmt(round.net)} coins · ${fmt(round.payout)} returned`;
      $('balance').textContent = fmt(round.balance); status('Round complete.');
      await refresh().catch(() => status('Round settled. Refresh to update your history.'));
    } catch (error) { status(error.message, true); }
    finally { busy = false; lock(); }
  }
  $('bet-form').addEventListener('submit', event => {
    event.preventDefault(); if (busy || !ready || pending) return;
    pending = {id:crypto.randomUUID(),game,choice:document.querySelector('[name=choice]:checked').value,stake:Number($('stake').value)};
    play();
  });
  $('retry').addEventListener('click', play);
  $('refresh').addEventListener('click', async () => { if (busy) return; try { await refresh(); status(pending ? 'Check your pending round before playing again.' : 'Balance updated.'); } catch (error) { status(error.message,true); } });
  $('games').addEventListener('click', event => { const button = event.target.closest('[data-game]'); if (button) select(button.dataset.game); });
  select('flip'); refresh().then(() => status('')).catch(error => status(error.message,true));
})();
