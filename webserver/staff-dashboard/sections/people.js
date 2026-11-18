/* People: staff roles, testers, blog contributors, notifications and (owner only)
   sign-in registrations. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  // Roster of emails with add/remove, for endpoints shaped {email, active}.
  function roleList(o) {
    return ui.list({
      access: o.access, title: o.title, desc: o.desc, empty: o.empty,
      load: async function () { return o.items(await SD.api(o.url)); },
      row: function (it) { return { label: it.email, tag: it.tag }; },
      actions: function (it) {
        return [{
          label: 'Remove', danger: true, confirm: 'Remove ' + it.email + ' from ' + o.noun + '?',
          run: function () { return SD.post(o.url, Object.assign({ email: it.email, active: false }, o.extra ? o.extra(it) : {})); },
        }];
      },
      add: {
        submitLabel: 'Add', confirm: o.confirmAdd,
        fields: [{ name: 'email', label: 'Email', type: 'email', required: true }].concat(o.addFields || []),
        submit: function (v) { return SD.post(o.url, Object.assign({ email: v.email, active: true }, o.addExtra ? o.addExtra(v) : {})); },
      },
    });
  }
  function emails(list, tag) { return (list || []).map(function (e) { return { email: e, tag: tag }; }); }

  // Moderator shortcut links, one "label | /path" per line.
  function shortcuts() {
    var f = ui.form({
      title: 'Moderator shortcuts', desc: 'Extra links shown on every moderator’s dashboard. One per line: label | /path',
      submitLabel: 'Save shortcuts', keep: true, request: true,
      fields: [{ name: 'links', label: 'Links', type: 'textarea', rows: 5, placeholder: 'Label | /path' }],
      submit: function (v) {
        var links = v.links.split('\n').map(function (line) { var p = line.split('|'); return { label: (p[0] || '').trim(), href: (p[1] || '').trim() }; })
          .filter(function (l) { return l.label || l.href; });
        return SD.post('/api/admin/moderator-panel', { links: links });
      },
      ok: function (d) { return 'Saved ' + (d.links || []).length + ' link(s).'; },
    });
    if (f && SD.can('admin')) {
      SD.api('/api/admin/moderator-panel').then(function (d) {
        f.querySelector('textarea').value = (d.links || []).map(function (l) { return (l.label || '') + ' | ' + (l.href || ''); }).join('\n');
      }).catch(function () {});
    }
    return f;
  }

  // Owner: sign-in registrations and the special role.
  function registrations() {
    if (!SD.can('owner')) return null;
    var accounts = [], selected = {};
    var list = h('div', { class: 'sd-rows' });
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    var search = h('input', { type: 'search', class: 'sd-input', placeholder: 'Search names, usernames or email', 'aria-label': 'Search accounts' });
    var count = h('p', { class: 'sd-desc', text: 'Load the accounts to begin.' });
    function say(t, bad) { status.className = 'sd-form-status' + (bad ? ' err' : t ? ' ok' : ''); status.textContent = t || ''; }
    function chosen() { return Object.keys(selected).filter(function (k) { return selected[k]; }); }
    function render() {
      list.textContent = '';
      var q = search.value.trim().toLowerCase();
      accounts.filter(function (a) { return [a.email, a.username, a.displayName].join(' ').toLowerCase().indexOf(q) >= 0; }).slice(0, 300).forEach(function (a) {
        var box = h('input', { type: 'checkbox', disabled: !!a.protected });
        box.checked = !!selected[a.email];
        box.addEventListener('change', function () { selected[a.email] = box.checked; count.textContent = chosen().length + ' selected.'; });
        list.appendChild(h('label', { class: 'sd-row-item' }, [box, h('span', { class: 'sd-row-main', text: (a.displayName || a.username || a.email) + ' · ' + a.email + (a.protected ? ' · protected' : '') + (a.scrotum ? ' · special role' : '') })]));
      });
    }
    search.addEventListener('input', render);
    async function call(body) { return SD.api('/api/admin/owner-accounts', body); }
    var confirmInput = h('input', { class: 'sd-input', placeholder: 'REMOVE 0 REGISTRATIONS', 'aria-label': 'Confirmation' });
    return ui.card('Accounts', 'Remove sign-in registrations so those addresses can sign up again. This revokes sessions, passwords, passkeys and recovery links. Profiles, posts and chat history are kept. Owner accounts are protected.', [
      h('div', { class: 'sd-actions' }, [
        ui.button('Load accounts', '', async function (b) {
          b.disabled = true;
          try { accounts = (await SD.api('/api/admin/accounts')).accounts || []; selected = {}; say(accounts.length + ' accounts.'); count.textContent = '0 selected.'; render(); }
          catch (e) { say(e.message, true); }
          b.disabled = false;
        }),
      ]),
      search, count, list,
      h('label', { class: 'sd-field' }, [h('span', { text: 'To remove registrations, type REMOVE <number selected> REGISTRATIONS' }), confirmInput]),
      h('div', { class: 'sd-actions' }, [
        ui.button('Remove selected registrations', 'danger', async function () {
          var emails = chosen();
          if (!emails.length) return say('Select at least one account.', true);
          var expected = 'REMOVE ' + emails.length + ' REGISTRATIONS';
          if (confirmInput.value !== expected) return say('Type ' + expected + ' to confirm.', true);
          try {
            var d = await call({ action: 'remove-registrations', emails: emails, confirmation: confirmInput.value });
            accounts = accounts.filter(function (a) { return !selected[a.email]; }); selected = {}; confirmInput.value = '';
            say(d.count + ' registrations removed. Those addresses can register again.'); render();
          } catch (e) { say(e.message, true); }
        }),
        ui.button('Give special role', '', function () { role(true); }),
        ui.button('Remove special role', '', function () { role(false); }),
      ]),
      status,
    ], 'wide');
    async function role(enabled) {
      var c = chosen();
      if (c.length !== 1) return say('Select exactly one account.', true);
      try {
        await call({ action: 'set-scrotum', email: c[0], enabled: enabled });
        var a = accounts.filter(function (x) { return x.email === c[0]; })[0];
        if (a) { a.scrotum = enabled; }
        say(enabled ? 'Role given. The account’s display name is now forced to MITCHS-SCROTUM.' : 'Role removed.'); render();
      } catch (e) { say(e.message, true); }
    }
  }

  function render(root) {
    ui.add(root, [ui.heading('Notifications')]);
    ui.add(root, [ui.grid([
      ui.form({
        access: 'mod', title: 'Send a notification', desc: 'To one account or everyone.', submitLabel: 'Send',
        fields: [
          { name: 'allUsers', label: 'Send to all users', type: 'checkbox' },
          { name: 'targetEmail', label: 'Account email', type: 'email', placeholder: 'user@email.com' },
          { name: 'title', label: 'Title', maxlength: 80 },
          { name: 'message', label: 'Message', type: 'textarea', maxlength: 1000, required: true },
        ],
        confirm: function (v) { return v.allUsers ? 'Send this to every user?' : ''; },
        submit: function (v) {
          if (!v.allUsers && !v.targetEmail) throw new Error('Enter an email or choose all users.');
          return SD.post('/api/admin/send-notification', v);
        },
        ok: function (d, v) { return v.allUsers ? 'Sent to ' + (d.count || 0) + ' users.' : 'Sent to ' + d.targetEmail + '.'; },
      }),
      ui.form({
        title: 'Emergency popup', desc: 'A popup shown to everyone currently on the site.', request: true, submitLabel: 'Broadcast', danger: true,
        fields: [{ name: 'msg', label: 'Message', required: true }],
        confirm: 'Show this popup to every active user?',
        submit: function (v) { return SD.post('/api/admin/broadcast', { msg: v.msg }); },
        ok: function () { return 'Broadcast sent.'; },
      }),
    ])]);

    var roles = ui.grid([
      roleList({ access: 'admin', title: 'Moderators', noun: 'moderators', url: '/api/admin/moderators', empty: 'No moderators.', items: function (d) { return emails(d.moderators); } }),
      roleList({ access: 'dev', title: 'Administrators', desc: 'Full control of the site.', noun: 'administrators', url: '/api/admin/admins', empty: 'No additional administrators.',
        confirmAdd: 'This gives broad control of the website immediately. Continue?', items: function (d) { return emails(d.admins); } }),
      roleList({ access: 'owner', title: 'Developers and Root (in an LXC)', noun: 'developers', url: '/api/admin/co-owners', empty: 'None configured.',
        items: function (d) {
          var lxc = {}; (d.rootLxc || []).forEach(function (e) { lxc[String(e).toLowerCase()] = true; });
          return (d.coOwners || []).map(function (e) { return { email: e, lxc: !!lxc[String(e).toLowerCase()], tag: lxc[String(e).toLowerCase()] ? 'Root (in an LXC)' : 'Developer' }; });
        },
        extra: function (it) { return { rootLxc: it.lxc }; },
        addFields: [{ name: 'rootLxc', label: 'Root (in an LXC) instead of Developer', type: 'checkbox' }],
        addExtra: function (v) { return { rootLxc: v.rootLxc }; },
        confirmAdd: 'This gives site-wide administrative access immediately. Continue?' }),
      roleList({ access: 'admin', title: 'Beta testers', noun: 'testers', url: '/api/admin/testers', empty: 'No testers.', items: function (d) { return emails(d.testers); } }),
      roleList({ access: 'admin', title: 'Blog contributors', noun: 'blog contributors', url: '/api/admin/blog-contributors', empty: 'No contributors.', items: function (d) { return emails(d.contributors); } }),
      shortcuts(),
    ]);
    if (roles.children.length) { root.appendChild(ui.heading('Roles')); root.appendChild(roles); }
    var reg = registrations();
    if (reg) { root.appendChild(ui.heading('Accounts')); root.appendChild(ui.grid([reg])); }
  }

  SD.register({ key: 'people', render: render });
})();
