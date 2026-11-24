/* Site: public access, domains and mirrors, reserved usernames, tester mode and
   admin passphrases. Admins only; some cards need developer or owner access. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  function maintenance() {
    var card = ui.card('Public access', 'Close the public site while keeping staff access so you can reopen it.', [], '');
    var state = h('p', { class: 'sd-desc', text: 'Checking...' });
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    var btn = ui.button('...', 'danger', function () {});
    btn.disabled = true;
    var active = false;
    function paint() { state.textContent = active ? 'The public site is closed for maintenance.' : 'The public site is open.'; btn.textContent = active ? 'Reopen the site' : 'Close for maintenance'; btn.className = active ? 'primary' : 'danger'; btn.disabled = false; }
    SD.api('/api/admin/maintenance-status').then(function (d) { active = !!d.active; paint(); }).catch(function (e) { state.textContent = e.message; });
    btn.addEventListener('click', async function () {
      if (!active && !(await SD.confirm('Close the site', 'Close the public site for maintenance? Staff access stays available so you can reopen it.', { danger: true, ok: 'Close site' }))) return;
      btn.disabled = true;
      try { var d = await SD.post('/api/admin/maintenance-toggle', { active: !active }); active = !!d.active; paint(); status.textContent = ''; }
      catch (e) { status.textContent = e.message; status.className = 'sd-form-status err'; btn.disabled = false; }
    });
    card.appendChild(state); card.appendChild(h('div', { class: 'sd-actions' }, [btn])); card.appendChild(status);
    return card;
  }

  function testerMode() {
    if (!SD.can('dev')) return null;
    var card = ui.card('Tester mode', 'Turns the beta tester tools on for your own account.', []);
    var box = h('input', { type: 'checkbox' }), status = h('p', { class: 'sd-form-status', role: 'status' });
    SD.api('/api/admin/tester-self').then(function (d) { box.checked = !!d.tester; }).catch(function (e) { status.textContent = e.message; });
    box.addEventListener('change', async function () {
      var want = box.checked;
      try { await SD.api('/api/admin/tester-self', { active: want }); status.textContent = want ? 'Tester mode on.' : 'Tester mode off.'; status.className = 'sd-form-status ok'; }
      catch (e) { box.checked = !want; status.textContent = e.message; status.className = 'sd-form-status err'; }
    });
    card.appendChild(h('label', { class: 'sd-check' }, [box, 'Tester mode'])); card.appendChild(status);
    return card;
  }

  // Loads a image from each domain; a real image means it is reachable from this browser.
  function probe(origin) {
    return new Promise(function (resolve) {
      var img = new Image(), t = setTimeout(function () { img.src = ''; resolve('timed out'); }, 8000);
      img.onload = function () { clearTimeout(t); resolve('loads'); };
      img.onerror = function () { clearTimeout(t); resolve('blocked'); };
      img.src = origin + '/favicon.ico?check=' + Date.now();
    });
  }

  async function domains(root) {
    var d = await SD.api('/api/admin/backup-domains');
    var post = function (body) { return SD.post('/api/admin/backup-domains', body); };
    var bare = function (o) { return String(o).replace(/^https?:\/\//, ''); };
    root.appendChild(ui.heading('Domains', 'Primary: ' + bare(d.primary) + ' · secondary: ' + bare(d.alternate)));
    var checks = h('div');
    var all = [d.primary, d.alternate].concat((d.domains || []).map(function (r) { return r.origin; }));
    var cards = [
      ui.list({
        access: 'admin', title: 'Backup domains', desc: 'Visitors fall back to these when the main domains are blocked.', empty: 'No backup domains. Visitors fall back to the secondary domain.',
        load: async function () { return (await SD.api('/api/admin/backup-domains')).domains || []; },
        row: function (r) { return { label: bare(r.origin), tag: r.active ? 'active' : '' }; },
        actions: function (r) {
          var a = [];
          if (!r.active) a.push({ label: 'Make active', run: function () { return post({ action: 'activate', origin: r.origin }); } });
          a.push({ label: 'Remove', danger: true, confirm: 'Remove ' + bare(r.origin) + '?', run: function () { return post({ action: 'remove', origin: r.origin }); } });
          return a;
        },
        add: { submitLabel: 'Add domain', fields: [{ name: 'origin', label: 'Domain', placeholder: 'https://example.com', required: true }], submit: function (v) { return post({ action: 'add', origin: v.origin }); } },
      }),
      ui.list({
        access: 'admin', title: 'Removed domains', desc: 'Kept so they aren’t added back by mistake.', empty: 'Empty.',
        load: async function () { return ((await SD.api('/api/admin/backup-domains')).removed || []).map(function (o) { return { origin: o }; }); },
        row: function (r) { return { label: bare(r.origin) }; },
        actions: function (r) { return [{ label: 'Forget', run: function () { return post({ action: 'forget', origin: r.origin }); } }]; },
      }),
      ui.card('Check domains', 'Tests which domains load from this browser (a filter’s block page counts as blocked).', [
        h('div', { class: 'sd-actions' }, [ui.button('Check now', '', async function (b) {
          b.disabled = true; checks.textContent = 'Checking...';
          var seen = {}, list = all.filter(function (o) { try { o = new URL(o).origin; } catch (e) { return false; } if (seen[o]) return false; seen[o] = 1; return true; });
          var res = await Promise.all(list.map(async function (o) { return [o, await probe(o)]; }));
          checks.textContent = '';
          checks.appendChild(ui.table(['Domain', 'Result'], res.map(function (r) { return [bare(r[0]), ui.pill(r[1], r[1] === 'loads' ? 'ok' : 'warn')]; })));
          b.disabled = false;
        })]), checks,
      ]),
      ui.form({
        access: 'owner', title: 'Secondary domain', submitLabel: 'Save', keep: true,
        fields: [{ name: 'url', label: 'Domain', value: d.alternate || '', required: true }],
        submit: function (v) { return SD.post('/api/admin/site-alternate', { url: v.url }); }, ok: function () { return 'Secondary domain saved.'; },
      }),
      ui.list({
        access: 'owner', title: 'Redirects', desc: 'Send visitors on a domain to another.', empty: 'No domains.',
        load: async function () {
          var x = await SD.api('/api/admin/backup-domains'), on = {}; (x.redirecting || []).forEach(function (hn) { on[String(hn).toLowerCase()] = true; });
          return [x.primary, x.alternate].concat((x.domains || []).map(function (r) { return r.origin; })).filter(Boolean).map(function (o) { var hn = new URL(o).hostname; return { origin: o, host: hn, on: !!on[hn.toLowerCase()] }; });
        },
        row: function (r) { return { label: r.host, tag: r.on ? 'redirecting' : '' }; },
        actions: function (r) { return [{ label: r.on ? 'Stop redirecting' : 'Start redirecting', run: function () { return post({ action: r.on ? 'redirect_off' : 'redirect_on', origin: r.origin }); } }]; },
      }),
      ui.list({
        access: 'admin', title: 'School mirrors', desc: 'rjuhsd.school mirror hosts.', empty: 'No mirrors.',
        load: async function () { return (await SD.api('/api/admin/school-mirrors')).mirrors || []; },
        row: function (m) { return { label: m.host }; },
        actions: function (m) { return [{ label: 'Remove', danger: true, confirm: 'Remove ' + m.host + '?', run: function () { return SD.post('/api/admin/school-mirrors', { action: 'remove', origin: m.host }); } }]; },
        add: { submitLabel: 'Add mirror', fields: [{ name: 'origin', label: 'Host', required: true }], submit: function (v) { return SD.post('/api/admin/school-mirrors', { action: 'add', origin: v.origin }); } },
      }),
    ];
    root.appendChild(ui.grid(cards));
  }

  async function render(root) {
    root.appendChild(ui.grid([maintenance(), testerMode(),
      ui.list({
        access: 'owner', title: 'Reserved usernames', desc: 'Names only the listed account (or nobody) can claim.', empty: 'No reserved usernames.',
        load: async function () { return (await SD.api('/api/admin/reserved-usernames')).reserved || []; },
        row: function (r) { return { label: r.username, sub: r.holderEmail ? 'only ' + r.holderEmail : 'nobody' }; },
        actions: function (r) { return [{ label: 'Remove', danger: true, confirm: 'Let anyone claim "' + r.username + '" again?', run: function () { return SD.post('/api/admin/reserved-usernames', { username: r.username, active: false }); } }]; },
        add: { submitLabel: 'Reserve', fields: [{ name: 'username', label: 'Username', required: true }, { name: 'email', label: 'Only this account (optional)', type: 'email' }], submit: function (v) { return SD.post('/api/admin/reserved-usernames', { username: v.username, email: v.email, active: true }); } },
      }),
    ]));
    if (SD.can('admin')) await domains(root);

    var pass = ui.grid([
      ui.form({
        access: 'admin', title: 'Change my admin passphrase', desc: 'Forgot it? Use the reset page, which emails a one-time code to your account.', submitLabel: 'Change passphrase',
        fields: [{ name: 'newPassphrase', label: 'New passphrase', type: 'password', required: true, autocomplete: 'new-password', hint: 'At least 4 characters.' }],
        submit: async function (v) {
          if (v.newPassphrase.length < 4) throw new Error('The passphrase must be at least 4 characters.');
          await SD.api('/api/admin/change-passphrase', { newPassphrase: v.newPassphrase });
          window.MitchAdminAuth.remember(v.newPassphrase);
          return 'Passphrase changed.';
        },
      }),
      ui.form({
        access: 'admin', title: 'Reset another admin’s passphrase', submitLabel: 'Reset passphrase', danger: true,
        fields: [{ name: 'targetEmail', label: 'Admin email', type: 'email', required: true }, { name: 'newPassphrase', label: 'New passphrase', type: 'password', required: true, autocomplete: 'new-password' }],
        confirm: function (v) { return 'Reset the passphrase for ' + v.targetEmail + '?'; },
        submit: async function (v) {
          if (v.newPassphrase.length < 4) throw new Error('The passphrase must be at least 4 characters.');
          await SD.api('/api/admin/reset-other-passphrase', v);
          return 'Passphrase for ' + v.targetEmail + ' reset.';
        },
      }),
      SD.can('admin') ? ui.card('Forgot your passphrase?', '', [h('a', { class: 'sd-link', href: '/staff-dashboard/passphrase-reset/', text: 'Reset it by email' })]) : null,
    ]);
    if (pass.children.length) { root.appendChild(ui.heading('Admin passphrase')); root.appendChild(pass); }
  }

  SD.register({ key: 'site', render: render });
})();
