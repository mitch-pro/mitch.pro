/* Staff dashboard core: shared helpers and UI builders used by every section.
   Sections register themselves with SD.register({key, label, access, render}).
   Only sections and tools the viewer can use are ever built, so nobody sees
   controls they could not use. See docs/staff-dashboard.md. */
(function () {
  'use strict';
  var SD = window.SD = { role: {}, sections: [] };

  // ---- DOM helpers -----------------------------------------------------------
  function h(tag, attrs, kids) {
    var el = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      var v = attrs[k];
      if (v === false || v == null) return;
      if (k === 'text') el.textContent = v;
      else if (k === 'class') el.className = v;
      else if (k === 'onclick') el.addEventListener('click', v);
      else el.setAttribute(k, v === true ? '' : v);
    });
    (kids || []).forEach(function (c) { if (c) el.appendChild(typeof c === 'string' ? document.createTextNode(c) : c); });
    return el;
  }
  SD.h = h;
  SD.$ = function (id) { return document.getElementById(id); };
  SD.say = function (text, bad) { var s = SD.$('sd-status'); if (!s) return; s.className = bad ? 'err' : ''; s.textContent = text || ''; };
  SD.ago = function (ms) {
    if (!ms) return '';
    var m = Math.max(0, Math.round((Date.now() - ms) / 60000));
    if (m < 1) return 'just now';
    if (m < 60) return m + ' min ago';
    if (m < 2880) return Math.round(m / 60) + ' h ago';
    return Math.round(m / 1440) + ' days ago';
  };
  SD.time = function (ms) { return ms ? new Date(ms).toLocaleString() : ''; };
  SD.money = function (v) { var n = Number(v); return isFinite(n) ? n.toLocaleString(undefined, { maximumFractionDigits: 2 }) : '0'; };

  // ---- roles -----------------------------------------------------------------
  // access levels: mod (moderator or above), admin, dev (owner or developer), owner.
  SD.can = function (level) {
    var r = SD.role;
    switch (level || 'admin') {
      case 'any': return !!r.isStaff;
      case 'mod': return !!(r.isAdmin || r.isModerator);
      case 'admin': return !!r.isAdmin;
      case 'dev': return !!(r.isOwner || r.isCoOwner);
      case 'owner': return !!r.isOwner;
      default: return false;
    }
  };
  // Moderators (not admins) can ask admins to approve some actions.
  SD.approvalMode = function () { return !!(SD.role.isModerator && !SD.role.isAdmin); };

  // ---- dialogs ---------------------------------------------------------------
  function dialog(build) {
    return new Promise(function (resolve) {
      var d = h('dialog', { class: 'sd-dialog' });
      var done = false;
      function close(v) { if (done) return; done = true; d.close(); d.remove(); resolve(v); }
      d.addEventListener('cancel', function (e) { e.preventDefault(); close(null); });
      build(d, close);
      document.body.appendChild(d);
      d.showModal();
      var f = d.querySelector('input,textarea,button.primary,button.danger');
      if (f) f.focus();
    });
  }
  SD.confirm = function (title, message, opts) {
    opts = opts || {};
    return dialog(function (d, close) {
      d.appendChild(h('h2', { text: title }));
      if (message) d.appendChild(h('p', { class: 'sd-dialog-text', text: message }));
      d.appendChild(h('div', { class: 'sd-row' }, [
        h('button', { type: 'button', text: 'Cancel', onclick: function () { close(false); } }),
        h('button', { type: 'button', class: opts.danger ? 'danger' : 'primary', text: opts.ok || 'Confirm', onclick: function () { close(true); } }),
      ]));
    }).then(function (v) { return !!v; });
  };
  // Resolves to the entered text (possibly empty), or null when cancelled.
  SD.prompt = function (title, okLabel, opts) {
    opts = opts || {};
    return dialog(function (d, close) {
      var input = h(opts.multiline === false ? 'input' : 'textarea', { class: 'sd-input', rows: 3, maxlength: opts.max || 500, 'aria-label': title, type: opts.password ? 'password' : false, autocomplete: 'off' });
      var form = h('form', null, [
        h('h2', { text: title }),
        opts.message ? h('p', { class: 'sd-dialog-text', text: opts.message }) : null,
        input,
        h('div', { class: 'sd-row' }, [
          h('button', { type: 'button', text: 'Cancel', onclick: function () { close(null); } }),
          h('button', { type: 'submit', class: opts.danger ? 'danger' : 'primary', text: okLabel || 'Save' }),
        ]),
      ]);
      form.addEventListener('submit', function (e) { e.preventDefault(); close(input.value.trim()); });
      d.appendChild(form);
    });
  };
  SD.alert = function (title, message) {
    return dialog(function (d, close) {
      d.appendChild(h('h2', { text: title }));
      d.appendChild(h('p', { class: 'sd-dialog-text', text: message || '' }));
      d.appendChild(h('div', { class: 'sd-row' }, [h('button', { type: 'button', class: 'primary', text: 'OK', onclick: function () { close(true); } })]));
    });
  };

  // ---- API -------------------------------------------------------------------
  function headers(extra) {
    var out = Object.assign({ 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, extra || {});
    var pass = sessionStorage.getItem('admin_passphrase');
    if (pass) out['X-Admin-Passphrase'] = pass;
    return out;
  }
  // Admins unlock with their passphrase (kept in session storage, like every
  // admin tool). The dialog repeats until it is right or the admin cancels.
  function askPassphrase() {
    return new Promise(function (resolve, reject) {
      dialog(function (d, close) {
        var input = h('input', { type: 'password', class: 'sd-input', autocomplete: 'current-password', 'aria-label': 'Admin passphrase', required: true });
        var err = h('p', { class: 'sd-err', role: 'status' });
        var form = h('form', null, [
          h('h2', { text: 'Admin passphrase' }), input, err,
          h('div', { class: 'sd-row' }, [
            h('button', { type: 'button', text: 'Cancel', onclick: function () { close(null); } }),
            h('button', { type: 'submit', class: 'primary', text: 'Unlock' }),
          ]),
        ]);
        form.addEventListener('submit', async function (e) {
          e.preventDefault();
          var pass = input.value.trim();
          if (!pass) return;
          try {
            var res = await fetch('/api/admin/passphrase-status', { method: 'POST', credentials: 'same-origin', headers: headers({ 'X-Admin-Passphrase': pass }), body: JSON.stringify({ passphrase: pass }) });
            var data = await res.json().catch(function () { return {}; });
            if (res.ok && data.ok) { sessionStorage.setItem('admin_passphrase', pass); close(true); return; }
          } catch (x) { /* fall through */ }
          sessionStorage.removeItem('admin_passphrase');
          err.textContent = 'Incorrect admin passphrase.'; input.select();
        });
        d.appendChild(form);
      }).then(function (ok) { return ok ? resolve() : reject(new Error('Admin passphrase required.')); });
    });
  }
  // GET (no body) or POST (body). Throws Error(message) on any failure.
  SD.api = async function (path, body, method) {
    function send() {
      return fetch(path, Object.assign({ credentials: 'same-origin', cache: 'no-store', headers: headers() }, body !== undefined ? { method: method || 'POST', body: JSON.stringify(body) } : {}));
    }
    var res = await send();
    var d = await res.json().catch(function () { return {}; });
    if (res.status === 403 && d.error === 'invalid_passphrase') {
      sessionStorage.removeItem('admin_passphrase');
      await askPassphrase();
      res = await send();
      d = await res.json().catch(function () { return {}; });
    }
    if (res.status === 403 && d.error === 'passphrase_not_configured') throw new Error('Set your admin passphrase first (Site section).');
    if (!res.ok || d.ok === false || (d.error && !d.ok)) throw new Error(d.message || d.error || 'Request failed (' + res.status + ').');
    return d;
  };

  // Actions a moderator may ask an admin to approve, by endpoint.
  var REQUESTABLE = {
    '/api/admin/grant-premium': 'grant_premium', '/api/admin/revoke-premium': 'revoke_premium',
    '/api/admin/gift-coins': 'gift_coins', '/api/admin/economy/burn': 'burn_coins',
    '/api/admin/economy/set-coins': 'set_coins', '/api/admin/economy/cap-coins': 'cap_coins',
    '/api/admin/economy/multiplier': 'economy_multiplier', '/api/admin/broadcast': 'broadcast',
    '/api/admin/content/featured': 'content_featured', '/api/admin/moderator-panel': 'moderator_panel',
    '/api/admin/moderators': 'moderator_role', '/api/admin/admins': 'admin_role', '/api/admin/testers': 'tester_role',
  };
  // Moderators may do these directly.
  var DIRECT = [
    '/api/admin/ban-account', '/api/admin/unban-account', '/api/admin/send-notification',
    '/api/admin/unsend-notification', '/api/admin/vm-ban', '/api/admin/byo-os-ban',
    '/api/admin/chat-reports/resolve', '/api/admin/profile-reports/resolve',
    '/api/admin/matrix/outside-accounts', '/api/admin/matrix/purge-user',
  ];
  SD.canRequest = function (url) { return !!REQUESTABLE[url]; };
  // POST that turns into an approval request for moderators when needed.
  // Resolves to the server's answer; `.pending` is set for a request.
  SD.post = async function (url, body) {
    if (SD.approvalMode() && DIRECT.indexOf(url) < 0 && url.indexOf('/api/matrix/') !== 0) {
      var action = REQUESTABLE[url];
      if (!action) throw new Error('That action needs an administrator.');
      var d = await SD.api('/api/moderator/action-request', { action: action, targetUrl: url, payload: body || {} });
      return { ok: true, pending: true, request: d.request };
    }
    return SD.api(url, body || {});
  };
  SD.pendingText = function (d) {
    return 'Sent to the admins for approval' + (d && d.request && d.request.id ? ' (#' + String(d.request.id).slice(0, 6) + ')' : '') + '.';
  };

  // ---- UI builders -----------------------------------------------------------
  var ui = SD.ui = {};

  // A settings-style card: title and description on the left, content on the right.
  ui.card = function (title, desc, kids, cls) {
    var info = h('div', { class: 'sd-card-info' }, [title ? h('h3', { text: title }) : null, desc ? h('p', { class: 'sd-desc', text: desc }) : null]);
    return h('section', { class: 'sd-card' + (cls ? ' ' + cls : '') }, [info, ui.add(h('div', { class: 'sd-card-body' }), kids || [])]);
  };
  // Appends the non-null elements to a parent.
  ui.add = function (parent, els) { (els || []).forEach(function (e) { if (e) parent.appendChild(e); }); return parent; };
  ui.grid = function (kids) { return ui.add(h('div', { class: 'sd-grid' }), kids); };
  ui.heading = function (text, sub) {
    return h('div', { class: 'sd-section-head' }, [h('h2', { text: text }), sub ? h('p', { class: 'sd-desc', text: sub }) : null]);
  };
  ui.stat = function (label, value, tone) {
    return h('div', { class: 'sd-stat' + (tone ? ' ' + tone : '') }, [h('span', { class: 'sd-stat-label', text: label }), h('strong', { text: value == null ? '-' : String(value) })]);
  };
  ui.pill = function (text, tone) { return h('span', { class: 'sd-flag' + (tone ? ' ' + tone : ''), text: text }); };
  ui.empty = function (text) { return h('div', { class: 'sd-empty', text: text }); };
  ui.pre = function (text) { return h('pre', { class: 'sd-log', text: text }); };
  ui.button = function (label, cls, fn) {
    var b = h('button', { type: 'button', class: cls || '', text: label });
    b.addEventListener('click', function () { fn(b); });
    return b;
  };
  ui.table = function (headers_, rows, emptyText) {
    if (!rows.length) return ui.empty(emptyText || 'Nothing to show.');
    var thead = h('thead', null, [h('tr', null, headers_.map(function (t) { return h('th', { text: t }); }))]);
    var tbody = h('tbody', null, rows.map(function (r) {
      return h('tr', null, r.map(function (c) { return h('td', null, [c instanceof Node ? c : document.createTextNode(c == null ? '' : String(c))]); }));
    }));
    return h('div', { class: 'sd-table-wrap' }, [h('table', { class: 'sd-table' }, [thead, tbody])]);
  };

  // Field spec: {name, label, type: text|email|number|password|textarea|select|checkbox,
  //   options: [[value, label]], placeholder, required, value, min, max, maxlength, hint}
  function field(f) {
    var id = 'sd-f-' + Math.random().toString(36).slice(2, 8);
    var input;
    if (f.type === 'textarea') input = h('textarea', { rows: f.rows || 3 });
    else if (f.type === 'select') input = h('select', null, (f.options || []).map(function (o) { return h('option', { value: o[0], text: o[1] }); }));
    else input = h('input', { type: f.type || 'text' });
    input.id = id; input.name = f.name;
    input.classList.add('sd-input');
    ['placeholder', 'min', 'max', 'maxlength', 'step'].forEach(function (k) { if (f[k] != null) input.setAttribute(k, f[k]); });
    if (f.required) input.required = true;
    if (f.autocomplete !== undefined) input.setAttribute('autocomplete', f.autocomplete);
    if (f.value != null) { if (f.type === 'checkbox') input.checked = !!f.value; else input.value = f.value; }
    var wide = f.type === 'textarea' || f.wide;
    if (f.type === 'checkbox') {
      input.classList.remove('sd-input');
      return { el: h('label', { class: 'sd-check', for: id }, [input, f.label]), input: input };
    }
    return { el: h('label', { class: 'sd-field' + (wide ? ' wide' : ''), for: id }, [h('span', { text: f.label }), input, f.hint ? h('small', { text: f.hint }) : null]), input: input };
  }
  function readValues(fields) {
    var v = {};
    fields.forEach(function (f) {
      var i = f.input;
      v[f.spec.name] = f.spec.type === 'checkbox' ? i.checked : f.spec.type === 'number' ? (i.value === '' ? '' : Number(i.value)) : i.value.trim();
    });
    return v;
  }

  // Form card. spec: {title, desc, access, request, fields, submit(values), submitLabel,
  //   danger, confirm: string | fn(values) -> string, keep: true to keep the values after success, onDone}
  // `submit` returns the server's answer (or a string); the form shows the result.
  ui.form = function (spec) {
    if (!SD.can(spec.access || 'admin') && !(spec.request && SD.approvalMode())) return null;
    var fields = (spec.fields || []).map(function (s) { var f = field(s); f.spec = s; return f; });
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    var submitBtn = h('button', { type: 'submit', class: spec.danger ? 'danger' : 'primary', text: spec.submitLabel || 'Save' });
    var needsApproval = SD.approvalMode() && !SD.can(spec.access || 'admin');
    var form = h('form', { class: 'sd-card' + (spec.danger ? ' danger' : ''), autocomplete: 'off' }, [
      h('div', { class: 'sd-card-info' }, [
        spec.title ? h('h3', { text: spec.title }) : null,
        spec.desc ? h('p', { class: 'sd-desc', text: spec.desc }) : null,
        needsApproval ? h('p', { class: 'sd-desc', text: 'Moderators send this to the admins for approval.' }) : null,
      ]),
      ui.add(h('div', { class: 'sd-card-body sd-fields' }), fields.map(function (f) { return f.el; }).concat([h('div', { class: 'sd-actions' }, [submitBtn]), status])),
    ]);
    function show(text, bad) { status.className = 'sd-form-status' + (bad ? ' err' : text ? ' ok' : ''); status.textContent = text || ''; }
    form.addEventListener('submit', async function (e) {
      e.preventDefault();
      var values = readValues(fields);
      var msg = typeof spec.confirm === 'function' ? spec.confirm(values) : spec.confirm;
      if (msg && !(await SD.confirm(spec.title || 'Confirm', msg, { danger: spec.danger, ok: spec.submitLabel }))) return;
      submitBtn.disabled = true; show('Working...');
      try {
        var d = await spec.submit(values, form);
        var text = typeof d === 'string' ? d : (d && d.pending) ? SD.pendingText(d) : (spec.ok ? spec.ok(d, values) : 'Done.');
        show(text);
        if (!spec.keep) form.reset();
        if (spec.onDone) spec.onDone(d, values);
      } catch (err) { show(err.message || 'Something went wrong.', true); }
      submitBtn.disabled = false;
    });
    return form;
  };

  // List card with optional per-row actions and an add form.
  // spec: {title, desc, access, load() -> items, row(item) -> {label, sub, tag}, actions(item) -> [{label, danger, confirm, run(item)}],
  //   add: form spec (its submit should add; the list reloads afterwards), empty}
  ui.list = function (spec) {
    if (!SD.can(spec.access || 'admin')) return null;
    var body = h('div', { class: 'sd-rows' });
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    var card = ui.card(spec.title, spec.desc, [body, status]);
    var cardBody = card.querySelector('.sd-card-body');
    function show(text, bad) { status.className = 'sd-form-status' + (bad ? ' err' : text ? ' ok' : ''); status.textContent = text || ''; }
    async function reload() {
      try {
        var items = await spec.load();
        body.textContent = '';
        if (!items.length) body.appendChild(ui.empty(spec.empty || 'None.'));
        items.forEach(function (it) {
          var r = spec.row(it);
          var acts = (spec.actions ? spec.actions(it) : []).map(function (a) {
            return ui.button(a.label, a.danger ? 'danger' : '', async function (b) {
              if (a.confirm && !(await SD.confirm(a.label, typeof a.confirm === 'function' ? a.confirm(it) : a.confirm, { danger: a.danger }))) return;
              b.disabled = true;
              try { var d = await a.run(it); show(d && d.pending ? SD.pendingText(d) : (a.done || '')); await reload(); if (spec.onChange) spec.onChange(); }
              catch (err) { show(err.message || 'Something went wrong.', true); b.disabled = false; }
            });
          });
          body.appendChild(h('div', { class: 'sd-row-item' }, [
            h('div', { class: 'sd-row-main' }, [h('strong', { text: r.label }), r.tag ? ui.pill(r.tag) : null, r.sub ? h('div', { class: 'sd-meta', text: r.sub }) : null]),
            h('div', { class: 'sd-actions' }, acts),
          ]));
        });
      } catch (err) { body.textContent = ''; show(err.message || 'Could not load.', true); }
    }
    if (spec.add) {
      var add = Object.assign({}, spec.add, { title: spec.add.title || '', access: spec.add.access || spec.access, onDone: function () { reload(); if (spec.onChange) spec.onChange(); } });
      var f = ui.form(add);
      if (f) { f.classList.add('inline'); cardBody.appendChild(f); }
    }
    card.reload = reload;
    reload();
    return card;
  };

  // ---- pages & navigation ----------------------------------------------------
  // Every dashboard page is its own URL with its own module; the sidebar links to
  // all of them. `level` is an SD.can level ('tester' = beta tester).
  var NAV = SD.nav = [
    { group: 'Dashboard', items: [
      { key: 'overview', label: 'Overview', href: '/staff-dashboard/', level: 'any', desc: 'Everything waiting on staff.' },
      { key: 'moderation', label: 'Moderation', href: '/staff-dashboard/moderation/', level: 'mod', desc: 'Look up accounts, ban or restore them, and approve chat access.' },
      { key: 'matrix', label: 'Matrix chat', href: '/staff-dashboard/matrix/', level: 'mod', desc: 'Controls for the official chat rooms. Reports are on the overview.' },
      { key: 'people', label: 'People', href: '/staff-dashboard/people/', level: 'mod', desc: 'Notifications, staff roles and accounts.' },
      { key: 'economy', label: 'Economy', href: '/staff-dashboard/economy/', level: 'mod', desc: 'Balances, rewards, premium and the casino.' },
      { key: 'shop', label: 'Shop', href: '/staff-dashboard/shop/', level: 'admin', desc: 'Edit item names, prices and descriptions. Saved overrides replace the server defaults.' },
      { key: 'vm-requests', label: 'Computer requests', href: '/staff-dashboard/vm-requests/', level: 'admin', desc: 'Requests for Linux containers and dedicated VMs.' },
      { key: 'logs', label: 'Logs', href: '/staff-dashboard/logs/', level: 'admin', desc: 'Server load, admin activity and audit logs.' },
      { key: 'site', label: 'Site', href: '/staff-dashboard/site/', level: 'admin', desc: 'Public access, domains, usernames and admin passphrases.' },
    ] },
    { group: 'Tools', items: [
      { href: '/moderator/support/', label: 'Support tickets', level: 'mod' },
      { href: '/moderator/feedback/', label: 'Feedback', level: 'mod' },
      { href: '/moderator/teachers/', label: 'Teacher ratings', level: 'mod' },
      { href: '/staff-dashboard/special-schedule/', label: 'Bell schedule', level: 'mod' },
      { href: '/staff-dashboard/bans/', label: 'Account bans', level: 'admin' },
      { href: '/staff-dashboard/vms/', label: 'Computers', level: 'admin' },
      { href: '/staff-dashboard/email/', label: 'Email', level: 'admin' },
      { href: '/staff-dashboard/updates/', label: 'Updates', level: 'admin' },
      { href: '/staff-dashboard/companion/', label: 'Companions', level: 'admin' },
      { href: '/matrix/', label: 'Matrix client', level: 'mod', blank: true },
      { href: '/tester/', label: 'Tester hub', level: 'tester' },
      { href: '/vms/desktop/?container=owner', label: 'Container 500', level: 'owner' },
    ] },
  ];
  function allowed(level) { return level === 'tester' ? !!SD.role.isTester : SD.can(level); }
  SD.navItem = function (key) {
    for (var g = 0; g < NAV.length; g++) for (var i = 0; i < NAV[g].items.length; i++) if (NAV[g].items[i].key === key) return NAV[g].items[i];
    return null;
  };

  SD.sections = {};
  // A page registers its renderer; label, description and access come from NAV.
  SD.register = function (s) { SD.sections[s.key] = s; };
  SD.current = null;

  function buildSidebar() {
    var side = SD.$('sd-side'); side.textContent = '';
    NAV.forEach(function (g) {
      var items = g.items.filter(function (it) { return allowed(it.level); });
      if (!items.length) return;
      side.appendChild(h('div', { class: 'sd-side-group' }, [h('p', { class: 'sd-side-title', text: g.group })].concat(items.map(function (it) {
        return h('a', { href: it.href, id: it.key ? 'sd-nav-' + it.key : false, 'aria-current': it.key && it.key === SD.current ? 'page' : false, target: it.blank ? '_blank' : false }, [it.label]);
      }))));
    });
  }
  // Shows a count next to a sidebar entry (used for the unresolved total).
  SD.badge = function (key, n) {
    var a = SD.$('sd-nav-' + key); if (!a) return;
    var old = a.querySelector('.sd-count'); if (old) old.remove();
    if (n) a.appendChild(h('span', { class: 'sd-count hot', text: String(n) }));
  };

  // Renders (or re-renders) the page's own section.
  SD.show = async function () {
    var s = SD.sections[SD.current], root = SD.$('sd-view');
    root.textContent = ''; SD.say('');
    try { await s.render(root); } catch (err) { SD.say(err.message || 'Could not load this page.', true); }
  };

  // Loads the viewer's roles. Resolves to false when the viewer is signed out or not staff.
  SD.loadRole = async function () {
    var res = await fetch('/api/staff-hub/status', { credentials: 'same-origin', cache: 'no-store' });
    if (res.status === 401) { location.href = '/enroll/?next=' + encodeURIComponent(location.pathname); return false; }
    if (!res.ok) throw new Error('Could not load staff status.');
    SD.role = await res.json();
    try {
      var me = await (await fetch('/api/me', { credentials: 'same-origin', cache: 'no-store' })).json();
      SD.role.isCoOwner = !!me.isCoOwner;
      SD.role.isOwner = SD.role.isOwner || !!me.isOwner;
    } catch (e) { /* roles from the status call are enough */ }
    return !!SD.role.isStaff;
  };

  // Starts the page named by <body data-section="...">.
  SD.start = async function () {
    SD.current = document.body.dataset.section || 'overview';
    var item = SD.navItem(SD.current) || {};
    SD.$('sd-title').textContent = item.label || 'Staff dashboard';
    SD.$('sd-sub').textContent = item.desc || '';
    document.title = (item.label || 'Staff dashboard') + ' — mitch.pro';
    try { if (!(await SD.loadRole())) { SD.say('Staff access required.', true); return; } }
    catch (err) { SD.say(err.message || 'Could not load staff status.', true); return; }
    var r = SD.role;
    SD.$('sd-role').textContent = r.isOwner ? 'Root' : r.isCoOwner ? 'Developer' : r.isAdmin ? 'Admin' : r.isModerator ? 'Moderator' : 'Tester';
    buildSidebar();
    SD.$('sd-refresh').addEventListener('click', function () { SD.show(); });
    if (!allowed(item.level || 'any') || !SD.sections[SD.current]) { SD.say('You do not have access to this page.', true); return; }
    // Unresolved total in the sidebar on every page; the overview keeps it fresh itself.
    if (SD.can('mod') && SD.current !== 'overview') {
      SD.api('/api/admin/staff-dashboard/summary').then(function (d) { SD.badge('overview', d.total); }).catch(function () {});
    }
    // Shortcuts admins add for moderators (optional).
    if (SD.can('mod')) {
      SD.api('/api/admin/moderator-panel').then(function (d) {
        var links = (d.links || []).filter(function (l) { return l && /^(\/|https?:\/\/)/i.test(String(l.href || '').trim()); });
        if (!links.length) return;
        var tools = SD.$('sd-side').querySelectorAll('.sd-side-group');
        var box = tools[tools.length - 1];
        links.forEach(function (l) { box.appendChild(h('a', { href: l.href }, [l.label])); });
      }).catch(function () {});
    }
    SD.show();
  };
})();
