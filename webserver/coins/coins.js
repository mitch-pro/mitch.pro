(() => {
  const balance = document.getElementById('coin-balance');
  const status = document.getElementById('coin-status');
  const message = document.getElementById('daily-message');
  const button = document.getElementById('daily-claim');
  const format = new Intl.NumberFormat(undefined, {maximumFractionDigits:2});
  async function wallet() {
    try {
      const response = await fetch('/api/me/coins', {credentials:'same-origin',cache:'no-store'});
      if (!response.ok) throw new Error('Wallet unavailable');
      const data = await response.json();
      if (data.authenticated === false) {
        balance.textContent = 'Sign in'; status.textContent = 'Your coins are saved to your account.';
        const link = document.createElement('a'); link.href = '/enroll/?next=%2Fcoins%2F'; link.textContent = 'Sign in to MitchCoins'; status.append(' ',link); return;
      }
      if (!Number.isFinite(Number(data.coins))) throw new Error('Wallet unavailable');
      balance.textContent = format.format(Number(data.coins)); status.textContent = 'MitchCoins available';
    } catch { balance.textContent = '—'; status.textContent = 'Couldn’t load your balance. Refresh to try again.'; }
  }
  async function daily() {
    try {
      const response = await fetch('/api/daily-login/state', {credentials:'same-origin',cache:'no-store'});
      if (!response.ok) { message.textContent = 'Sign in to collect daily rewards.'; return; }
      const data = await response.json(); message.textContent = data.message || 'Come back each day to collect a reward.';
      button.disabled = !data.canClaim; button.textContent = data.canClaim ? 'Claim ' + format.format(data.nextReward) + ' coins' : 'Claimed today';
    } catch { message.textContent = 'Daily rewards are unavailable right now.'; }
  }
  button.addEventListener('click', async () => {
    button.disabled = true;
    try {
      const response = await fetch('/api/daily-login/claim', {method:'POST',credentials:'same-origin'});
      const data = await response.json(); if (!response.ok || !data.success) throw new Error(data.error || 'Couldn’t claim your reward.');
      await Promise.all([wallet(),daily()]);
    } catch(error) { message.textContent = error.message; button.disabled = false; }
  });
  wallet(); daily();
  document.addEventListener('visibilitychange',() => {if (!document.hidden) {wallet();daily();}});
})();
