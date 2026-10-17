(() => {
  const panel = document.getElementById('owner-account-tools');
  if (!panel) return;
  const status = panel.querySelector('[role=status]');
  const list = panel.querySelector('.owner-account-list');
  const selected = new Set();
  let accounts = [], busy = false;
  const search = panel.querySelector('[type=search]');
  const confirmation = panel.querySelector('[data-confirmation]');
  const count = panel.querySelector('[data-count]');
  function render() {
    list.replaceChildren();
    accounts.filter(account => [account.email,account.username,account.displayName].join(' ').toLowerCase().includes(search.value.trim().toLowerCase())).forEach(account => {
      const label = document.createElement('label'); const input = document.createElement('input'); input.type = 'checkbox';
      input.checked = selected.has(account.email); input.disabled = account.protected || busy;
      input.addEventListener('change', () => { input.checked ? selected.add(account.email) : selected.delete(account.email); update(); });
      label.append(input,document.createTextNode((account.displayName || account.username || account.email) + ' · ' + account.email + (account.protected ? ' · Protected owner' : ''))); list.append(label);
    }); update();
  }
  function update() { count.textContent = `${selected.size} selected. To remove them, type REMOVE ${selected.size} REGISTRATIONS below.`; }
  async function call(body) {
    if (typeof postAdmin !== 'function') throw new Error('Reload the owner panel and unlock it first.');
    return postAdmin('/api/admin/owner-accounts',body);
  }
  async function run(task) {
    if (busy) return; busy = true;
    panel.querySelectorAll('button').forEach(button => button.disabled = true);
    try { await task(); } catch(error) { status.textContent = error.message; }
    finally { busy = false; panel.querySelectorAll('button').forEach(button => button.disabled = false); render(); }
  }
  panel.querySelector('[data-load]').onclick = () => run(async () => {
    const response = typeof adminFetch === 'function'
      ? await adminFetch('/api/admin/accounts')
      : await fetch('/api/admin/accounts', {
          credentials: 'same-origin',
          headers: typeof getAdminHeaders === 'function'
            ? getAdminHeaders()
            : { 'X-Mitch-Requested-With': '1', 'X-Admin-Passphrase': sessionStorage.getItem('admin_passphrase') || '' }
        });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw new Error(data.error || 'Could not load accounts.');
    accounts = data.accounts || [];
    selected.clear();
    status.textContent = `${accounts.length} accounts. Choose accounts below.`;
  });
  panel.querySelector('[data-all]').onclick = () => { accounts.forEach(account => { if (!account.protected) selected.add(account.email); }); render(); };
  panel.querySelector('[data-none]').onclick = () => { selected.clear(); render(); };
  search.oninput = render;
  panel.querySelector('[data-remove]').onclick = () => run(async () => {
    if (!selected.size) throw new Error('Select at least one account.');
    const expected = `REMOVE ${selected.size} REGISTRATIONS`;
    if (confirmation.value !== expected) throw new Error(`Type ${expected} to confirm.`);
    const data = await call({action:'remove-registrations',emails:[...selected],confirmation:confirmation.value});
    accounts = accounts.filter(account => !selected.has(account.email)); selected.clear(); confirmation.value = '';
    status.textContent = `${data.count} sign-in registrations removed. These email addresses can register again. Content was kept.`;
  });
  panel.querySelector('[data-reset]').onclick = () => run(async () => {
    const typed = panel.querySelector('[data-coin-confirmation]');
    if (typed.value !== 'RESET ALL COINS') throw new Error('Type RESET ALL COINS in the coin confirmation field.');
    const data = await call({action:'reset-coins',confirmation:typed.value}); typed.value = '';
    status.textContent = `${data.count} coin balances reset to zero. New coins can still be earned.`;
  });
  fetch('/api/me',{credentials:'same-origin'}).then(r=>r.json()).then(me=>{ panel.hidden = !me.isOwner; }).catch(()=>{});
})();
