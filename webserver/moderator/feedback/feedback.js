(function () {
  'use strict';
  var BASE = '/api/admin/feedback';
  var $ = function (id) { return document.getElementById(id); };
  var state = { filter: 'new', isAdmin: false, counts: { new: 0, handled: 0 } };
  var KINDS = { feedback: 'Feedback', broken_game: 'Broken', add_page: 'Add a page or game', general: 'Idea' };

  function headers(extra) {
    var h = Object.assign({ 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, extra || {});
    var pass = sessionStorage.getItem('admin_passphrase');
    if (pass) h['X-Admin-Passphrase'] = pass;
    return h;
  }

  function say(text, bad) {
    var s = $('status');
    s.className = 'fb-status' + (bad ? ' err' : '');
    s.textContent = text || '';
  }

  // Admins unlock with their passphrase; moderators never see this.
  function askPassphrase() {
    var dialog = $('pass-dialog'), form = $('pass-form'), input = $('pass-input'), st = $('pass-status');
    return new Promise(function (resolve, reject) {
      st.textContent = ''; input.value = '';
      $('pass-cancel').onclick = function () { dialog.close(); reject(new Error('Admin passphrase required.')); };
      form.onsubmit = async function (e) {
        e.preventDefault();
        var pass = input.value.trim();
        if (!pass) return;
        try {
          var res = await fetch('/api/admin/passphrase-status', { method: 'POST', credentials: 'same-origin', headers: headers({ 'X-Admin-Passphrase': pass }), body: JSON.stringify({ passphrase: pass }) });
          var d = await res.json().catch(function () { return {}; });
          if (res.ok && d.ok) { sessionStorage.setItem('admin_passphrase', pass); dialog.close(); resolve(); return; }
        } catch (err) { /* fall through */ }
        sessionStorage.removeItem('admin_passphrase');
        st.textContent = 'Incorrect admin passphrase.'; input.select();
      };
      dialog.showModal(); input.focus();
    });
  }

  async function api(path, body) {
    function send() {
      return fetch(BASE + path, Object.assign({ credentials: 'same-origin', cache: 'no-store', headers: headers() }, body ? { method: 'POST', body: JSON.stringify(body) } : {}));
    }
    var res = await send();
    var data = await res.json().catch(function () { return {}; });
    if (res.status === 403 && data.error === 'invalid_passphrase') {
      sessionStorage.removeItem('admin_passphrase');
      await askPassphrase();
      res = await send();
      data = await res.json().catch(function () { return {}; });
    }
    if (res.status === 403 && data.error === 'passphrase_not_configured') throw new Error('Set your admin passphrase on the admin page first.');
    if (!res.ok) throw new Error(data.error || 'Request failed (' + res.status + ').');
    return data;
  }

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  function renderTabs() {
    var tabs = $('tabs');
    tabs.replaceChildren();
    [['new', 'New'], ['handled', 'Handled'], ['all', 'All']].forEach(function (t) {
      var n = t[0] === 'all' ? state.counts.new + state.counts.handled : state.counts[t[0]];
      var b = el('button', '', t[1] + ' (' + n + ')');
      b.type = 'button';
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(state.filter === t[0]));
      b.onclick = function () { state.filter = t[0]; load(); };
      tabs.append(b);
    });
  }

  function card(entry) {
    var c = el('article', 'fb-card' + (entry.status === 'handled' ? ' is-handled' : ''));
    var meta = el('div', 'fb-meta');
    meta.append(el('span', 'fb-kind', KINDS[entry.type] || entry.type || 'Feedback'));
    meta.append(el('span', '', new Date(entry.createdAt).toLocaleString()));
    meta.append(el('span', '', entry.account || 'Not signed in'));
    if (entry.status === 'handled' && entry.handledBy) meta.append(el('span', '', 'Handled by ' + entry.handledBy));
    c.append(meta, el('p', 'fb-text', entry.text));
    var actions = el('div', 'fb-actions');
    var toggle = el('button', 'fb-btn' + (entry.status === 'new' ? ' primary' : ''), entry.status === 'new' ? 'Mark handled' : 'Reopen');
    toggle.type = 'button';
    toggle.onclick = function () { act('/status', { id: entry.id, status: entry.status === 'new' ? 'handled' : 'new' }, toggle); };
    actions.append(toggle);
    if (state.isAdmin) {
      var del = el('button', 'fb-btn', 'Delete');
      del.type = 'button';
      del.onclick = function () { if (confirm('Delete this feedback?')) act('/delete', { id: entry.id }, del); };
      actions.append(del);
    }
    c.append(actions);
    return c;
  }

  async function act(path, body, button) {
    button.disabled = true;
    say('');
    try {
      await api(path, body);
      await load();
    } catch (err) {
      say(err.message, true);
      button.disabled = false;
    }
  }

  async function load() {
    try {
      var data = await api('?status=' + encodeURIComponent(state.filter));
      state.counts = data.counts || state.counts;
      renderTabs();
      var list = $('list');
      list.replaceChildren();
      if (!data.entries.length) list.append(el('p', 'fb-empty', state.filter === 'new' ? 'Nothing new.' : 'Nothing here.'));
      data.entries.forEach(function (e) { list.append(card(e)); });
      say('');
    } catch (err) {
      say(err.message, true);
    }
  }

  (async function () {
    try {
      var res = await fetch('/api/me', { credentials: 'same-origin', cache: 'no-store' });
      var me = await res.json().catch(function () { return {}; });
      var admin = !!(me.isAdmin || me.isOwner || me.isCoOwner);
      if (!admin && !me.isModerator) { $('denied').hidden = false; return; }
      state.isAdmin = admin;
      $('app').hidden = false;
      renderTabs();
      await load();
    } catch (err) {
      $('denied').hidden = false;
    }
  })();
})();
