(function () {
  'use strict';
  var BASE = '/api/admin/teachers';
  var $ = function (id) { return document.getElementById(id); };
  var state = { view: 'notes', filter: 'pending', isAdmin: false, school: '', counts: { pending: 0, approved: 0, rejected: 0 } };

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

  function button(label, primary, onclick) {
    var b = el('button', 'fb-btn' + (primary ? ' primary' : ''), label);
    b.type = 'button';
    b.onclick = function () { onclick(b); };
    return b;
  }

  async function act(fn, b) {
    if (b) b.disabled = true;
    say('');
    try { await fn(); } catch (err) { say(err.message, true); if (b) b.disabled = false; }
  }

  // ── Notes ──
  function renderTabs() {
    var tabs = $('tabs');
    tabs.replaceChildren();
    [['pending', 'Pending'], ['approved', 'Approved'], ['rejected', 'Rejected'], ['all', 'All']].forEach(function (t) {
      var n = t[0] === 'all' ? state.counts.pending + state.counts.approved + state.counts.rejected : state.counts[t[0]];
      var b = el('button', '', t[1] + ' (' + n + ')');
      b.type = 'button';
      b.setAttribute('role', 'tab');
      b.setAttribute('aria-selected', String(state.filter === t[0]));
      b.onclick = function () { state.filter = t[0]; loadNotes(); };
      tabs.append(b);
    });
  }

  function noteCard(n) {
    var c = el('article', 'fb-card' + (n.status === 'rejected' ? ' is-handled' : ''));
    var meta = el('div', 'fb-meta');
    meta.append(el('span', 'fb-kind', n.teacher), el('span', '', n.school), el('span', '', new Date(n.day * 86400000).toLocaleDateString(undefined, { timeZone: 'UTC' })));
    if (n.status !== 'pending') meta.append(el('span', '', (n.status === 'approved' ? 'Approved' : 'Rejected') + (n.reviewedBy ? ' by ' + n.reviewedBy : '')));
    c.append(meta, el('p', 'fb-text', n.text));
    var actions = el('div', 'fb-actions');
    function set(status) { return function (b) { act(async function () { await api('/note-status', { id: n.id, status: status }); await loadNotes(); }, b); }; }
    if (n.status !== 'approved') actions.append(button('Approve', n.status === 'pending', set('approved')));
    if (n.status !== 'rejected') actions.append(button('Reject', false, set('rejected')));
    if (n.status !== 'pending') actions.append(button('Back to pending', false, set('pending')));
    if (state.isAdmin) actions.append(button('Delete', false, function (b) {
      if (confirm('Delete this note?')) act(async function () { await api('/note-delete', { id: n.id }); await loadNotes(); }, b);
    }));
    c.append(actions);
    return c;
  }

  async function loadNotes() {
    try {
      var data = await api('?status=' + encodeURIComponent(state.filter));
      state.counts = data.counts || state.counts;
      renderTabs();
      var list = $('list');
      list.replaceChildren();
      if (!data.notes.length) list.append(el('p', 'fb-empty', state.filter === 'pending' ? 'No notes waiting.' : 'Nothing here.'));
      data.notes.forEach(function (n) { list.append(noteCard(n)); });
      say('');
    } catch (err) { say(err.message, true); }
  }

  // ── Teachers ──
  async function loadTeachers() {
    try {
      var data = await api('/list?school=' + encodeURIComponent(state.school));
      var sel = $('school');
      if (!sel.options.length) {
        data.schools.forEach(function (s) { var o = el('option', '', s.name); o.value = s.key; o.dataset.name = s.name; sel.append(o); });
      }
      data.schools.forEach(function (s, i) { sel.options[i].textContent = s.name + ' (' + s.teachers + ')'; });
      var cur = data.schools.find(function (s) { return s.key === data.school; });
      var info = $('sync-info');
      if (cur && cur.sync) info.textContent = 'Last read ' + new Date(cur.sync.at).toLocaleString() + ': ' + (cur.sync.error ? cur.sync.error : cur.sync.found + ' found');
      else info.textContent = 'Not read from the school site yet.';
      state.school = data.school;
      sel.value = state.school;
      var list = $('teacher-list');
      list.replaceChildren();
      if (!data.teachers.length) list.append(el('p', 'fb-empty', 'No teachers listed for this school.'));
      data.teachers.forEach(function (t) {
        var row = el('div', 'tr-row');
        var left = el('span', '', t.name);
        var info = [t.department, t.ratings + (t.ratings === 1 ? ' rating' : ' ratings')].filter(Boolean).join(' · ');
        left.append(el('small', '', info));
        row.append(left);
        if (state.isAdmin) row.append(button('Remove', false, function (b) {
          if (confirm('Remove ' + t.name + ' and their ratings?')) act(async function () { await api('/remove', { id: t.id }); await loadTeachers(); }, b);
        }));
        list.append(row);
      });
      say('');
    } catch (err) { say(err.message, true); }
  }

  function show(view) {
    state.view = view;
    $('notes-view').hidden = view !== 'notes';
    $('teachers-view').hidden = view !== 'teachers';
    $('tab-notes').setAttribute('aria-selected', String(view === 'notes'));
    $('tab-teachers').setAttribute('aria-selected', String(view === 'teachers'));
    if (view === 'notes') loadNotes(); else loadTeachers();
  }

  $('tab-notes').onclick = function () { show('notes'); };
  $('tab-teachers').onclick = function () { show('teachers'); };
  $('school').onchange = function () { state.school = $('school').value; loadTeachers(); };

  $('add-form').onsubmit = function (e) {
    e.preventDefault();
    act(async function () {
      await api('/add', { school: state.school, name: $('add-name').value, department: $('add-dept').value });
      $('add-name').value = ''; $('add-dept').value = '';
      await loadTeachers();
    });
  };

  $('paste-form').onsubmit = function (e) {
    e.preventDefault();
    act(async function () {
      var r = await api('/import', { school: state.school, text: $('paste-text').value });
      $('paste-text').value = '';
      await loadTeachers();
      say(r.added + ' added, ' + (r.found - r.added) + ' already listed.');
    });
  };

  $('sync').onclick = function () {
    var b = $('sync');
    act(async function () {
      say('Checking the school sites…');
      var r = await api('/sync', { school: '' });
      await loadTeachers();
      var lines = r.results.map(function (x) { return x.school + ': ' + (x.error ? x.error : x.added + ' added of ' + x.found + ' found'); });
      say(lines.join('. '), r.results.every(function (x) { return x.error; }));
      b.disabled = false;
    }, b);
  };

  (async function () {
    try {
      var res = await fetch('/api/me', { credentials: 'same-origin', cache: 'no-store' });
      var me = await res.json().catch(function () { return {}; });
      var admin = !!(me.isAdmin || me.isOwner || me.isCoOwner);
      if (!admin && !me.isModerator) { $('denied').hidden = false; return; }
      state.isAdmin = admin;
      $('app').hidden = false;
      show('notes');
    } catch (err) {
      $('denied').hidden = false;
    }
  })();
})();
