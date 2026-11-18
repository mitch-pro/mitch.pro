/* Economy: coin tools, multiplier, casino switch and premium. Admins only;
   moderators can send some of these to the admins for approval (SD.post). */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  function audit() {
    var box = h('div', { class: 'sd-stats' });
    SD.api('/api/admin/economy/audit').then(function (d) {
      ui.add(box, [
        ui.stat('Total coins', SD.money(d.total)),
        ui.stat('Top 1% owns', Number(d.topPercent || 0).toFixed(1) + '%'),
        ui.stat('Richest', d.richestUser || 'N/A'),
      ]);
    }).catch(function (e) { box.appendChild(ui.empty(e.message)); });
    var casino = ui.stat('Casino', '...');
    fetch('/api/casino/status', { credentials: 'same-origin', cache: 'no-store' }).then(function (r) { return r.json(); })
      .then(function (d) { casino.replaceWith(ui.stat('Casino', d.enabled ? 'Open' : 'Disabled', d.enabled ? 'good' : 'bad')); })
      .catch(function () { casino.replaceWith(ui.stat('Casino', 'Unknown')); });
    box.appendChild(casino);
    return box;
  }

  function casinoToggle() {
    return ui.form({
      title: 'Casino switch', desc: 'Close or reopen every casino game.', submitLabel: 'Toggle casino', danger: true,
      fields: [], confirm: 'Toggle the casino on or off?',
      submit: function () { return SD.post('/api/admin/casino/toggle', {}); },
      ok: function (d) { return d.enabled ? 'Casino is now open.' : 'Casino disabled.'; }, onDone: function () { SD.show('economy'); },
    });
  }

  function render(root) {
    ui.add(root, [ui.heading('Economy', 'Balances, rewards and the casino.')]);
    if (SD.can('admin')) root.appendChild(audit());
    var coins = ui.grid([
      ui.form({
        title: 'Give coins', desc: 'Add coins to one account.', request: true, submitLabel: 'Give coins',
        fields: [
          { name: 'targetEmail', label: 'Account email or username', required: true },
          { name: 'amount', label: 'Amount', type: 'number', required: true, min: 1 },
          { name: 'reason', label: 'Reason' },
        ],
        submit: function (v) { return SD.post('/api/admin/gift-coins', { targetEmail: v.targetEmail, amount: v.amount, reason: v.reason }); },
        ok: function (d) { return 'Gave ' + d.amount + ' coins. New balance: ' + SD.money(d.newBalance); },
      }),
      ui.form({
        title: 'Set balance', desc: 'Overwrite one account’s balance.', request: true, submitLabel: 'Set balance', danger: true,
        fields: [
          { name: 'targetEmail', label: 'Account email or username', required: true },
          { name: 'amount', label: 'New balance', type: 'number', required: true, min: 0 },
          { name: 'reason', label: 'Reason' },
        ],
        confirm: function (v) { return 'Set ' + v.targetEmail + '’s balance to ' + v.amount + ' coins? This overwrites their current balance.'; },
        submit: function (v) { return SD.post('/api/admin/economy/set-coins', { targetEmail: v.targetEmail, amount: v.amount, reason: v.reason }); },
        ok: function (d) { return 'Balance set: ' + SD.money(d.previousBalance) + ' to ' + SD.money(d.newBalance); },
      }),
      ui.form({
        title: 'Burn coins', desc: 'Remove coins from one account.', request: true, submitLabel: 'Burn', danger: true,
        fields: [{ name: 'email', label: 'Account email', required: true }, { name: 'amount', label: 'Amount', type: 'number', required: true, min: 1 }],
        submit: function (v) { return SD.post('/api/admin/economy/burn', { email: v.email, amount: v.amount }); },
        ok: function (d) { return 'Burned. New balance: ' + SD.money(d.newBalance); },
      }),
      ui.form({
        title: 'Reset balances above a limit', desc: 'One time: every account above the limit is set down to the new value. It does not stop anyone earning past it again.',
        request: true, submitLabel: 'Reset balances', danger: true,
        fields: [
          { name: 'above', label: 'Accounts above', type: 'number', required: true, min: 0 },
          { name: 'setTo', label: 'Set them to', type: 'number', required: true, min: 0 },
          { name: 'reason', label: 'Reason' },
        ],
        confirm: function (v) { return 'Set every account currently above ' + v.above + ' coins down to ' + v.setTo + '? This affects every matching account right now and cannot be undone automatically.'; },
        submit: function (v) { return SD.post('/api/admin/economy/cap-coins', { above: v.above, setTo: v.setTo, reason: v.reason }); },
        ok: function (d) { return 'Reset ' + d.affectedCount + ' account(s) to ' + SD.money(d.newBalance) + '.'; },
      }),
      ui.form({
        title: 'Earnings multiplier', desc: 'Scales coins earned site-wide.', request: true, submitLabel: 'Update multiplier',
        fields: [{ name: 'multiplier', label: 'Multiplier', type: 'select', options: [['1.0', '1.0x (normal)'], ['1.5', '1.5x (bonus)'], ['2.0', '2.0x (double)'], ['5.0', '5.0x (extreme)']] }],
        submit: function (v) { return SD.post('/api/admin/economy/multiplier', { multiplier: v.multiplier }); },
        ok: function (d, v) { return 'Multiplier is now ' + v.multiplier + 'x.'; }, keep: true,
      }),
      casinoToggle(),
    ]);
    root.appendChild(coins);

    var premium = ui.grid([
      ui.form({
        title: 'Grant premium', request: true, submitLabel: 'Grant',
        fields: [{ name: 'targetEmail', label: 'Account email or username', required: true }, { name: 'reason', label: 'Reason' }],
        submit: function (v) { return SD.post('/api/admin/grant-premium', v); },
        ok: function (d) { return 'Premium granted to ' + d.targetEmail + '.'; },
      }),
      ui.form({
        title: 'Revoke premium', request: true, submitLabel: 'Revoke', danger: true,
        fields: [{ name: 'email', label: 'Account email or username', required: true }, { name: 'reason', label: 'Reason' }],
        submit: function (v) { return SD.post('/api/admin/revoke-premium', v); },
        ok: function (d) { return 'Premium revoked for ' + d.targetEmail + '.'; },
      }),
    ]);
    if (premium.children.length) { root.appendChild(ui.heading('Premium')); root.appendChild(premium); }

    var owner = ui.form({
      access: 'owner', title: 'Reset everyone’s coins',
      desc: 'Sets every balance to zero, including staff. Purchased items and lifetime achievements stay. An encrypted recovery snapshot is saved on the server.',
      submitLabel: 'Reset all balances to zero', danger: true,
      fields: [{ name: 'confirmation', label: 'Type RESET ALL COINS to confirm', required: true }],
      submit: function (v) {
        if (v.confirmation !== 'RESET ALL COINS') throw new Error('Type RESET ALL COINS to confirm.');
        return SD.api('/api/admin/owner-accounts', { action: 'reset-coins', confirmation: v.confirmation });
      },
      ok: function (d) { return d.count + ' balances reset to zero. New coins can still be earned.'; },
    });
    if (owner) { root.appendChild(ui.heading('Root')); root.appendChild(ui.grid([owner])); }
  }

  SD.register({ key: 'economy', label: 'Economy', access: 'mod', render: function (root) {
    // Moderators only see this section when at least one tool can be requested.
    render(root);
  } });
})();
