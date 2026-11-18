/* Staff dashboard: one place for everything waiting on moderators and admins.
   Data comes from GET /api/admin/staff-dashboard/summary; each action calls the
   existing endpoint for that kind of item. See docs/staff-dashboard.md. */
(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var OLD_MS = 24 * 3600 * 1000;
  var data = null;        // last summary
  var role = {};          // /api/staff-hub/status
  var current = 'all';    // selected tab key
  var showClosedMatrix = false;
  var closedMatrix = [];

  // ---- small helpers -------------------------------------------------------
  function h(tag, attrs, kids) {
    var el = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (k) {
      if (k === 'text') el.textContent = attrs[k];
      else if (k === 'class') el.className = attrs[k];
      else if (k === 'onclick') el.addEventListener('click', attrs[k]);
      else if (attrs[k] !== false && attrs[k] != null) el.setAttribute(k, attrs[k]);
    });
    (kids || []).forEach(function (c) { if (c) el.appendChild(typeof c === 'string' ? document.createTextNode(c) : c); });
    return el;
  }
  function say(text, bad) { var s = $('sd-status'); s.className = bad ? 'err' : ''; s.textContent = text || ''; }
  function ago(ms) {
    if (!ms) return '';
    var m = Math.max(0, Math.round((Date.now() - ms) / 60000));
    if (m < 1) return 'just now';
    if (m < 60) return m + ' min ago';
    if (m < 2880) return Math.round(m / 60) + ' h ago';
    return Math.round(m / 1440) + ' days ago';
  }
  function age(ms) { return h('span', { class: 'sd-age', title: ms ? new Date(ms).toLocaleString() : '', text: ago(ms) }); }
  function headers(extra) {
    var out = Object.assign({ 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, extra || {});
    var pass = sessionStorage.getItem('admin_passphrase');
    if (pass) out['X-Admin-Passphrase'] = pass;
    return out;
  }

  // Admins unlock with their passphrase (kept in session storage, like every
  // admin tool); moderators never reach this.
  function askPassphrase() {
    var dialog = $('sd-pass-dialog'), form = $('sd-pass-form'), input = $('sd-pass-input'), st = $('sd-pass-status');
    return new Promise(function (resolve, reject) {
      st.textContent = ''; input.value = '';
      $('sd-pass-cancel').onclick = function () { dialog.close(); reject(new Error('Admin passphrase required.')); };
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
      return fetch(path, Object.assign({ credentials: 'same-origin', cache: 'no-store', headers: headers() }, body ? { method: 'POST', body: JSON.stringify(body) } : {}));
    }
    var res = await send();
    var d = await res.json().catch(function () { return {}; });
    if (res.status === 403 && d.error === 'invalid_passphrase') {
      sessionStorage.removeItem('admin_passphrase');
      await askPassphrase();
      res = await send();
      d = await res.json().catch(function () { return {}; });
    }
    if (res.status === 403 && d.error === 'passphrase_not_configured') throw new Error('Set your admin passphrase on the admin page first.');
    if (!res.ok || d.ok === false) throw new Error(d.error || d.message || 'Request failed (' + res.status + ').');
    return d;
  }

  // Optional note from the staff member; resolves to null when cancelled.
  function askNote(title, okLabel) {
    var dialog = $('sd-note-dialog'), input = $('sd-note-input');
    $('sd-note-title').textContent = title;
    $('sd-note-ok').textContent = okLabel || 'Save';
    input.value = '';
    return new Promise(function (resolve) {
      var done = false;
      function finish(v) { if (done) return; done = true; dialog.close(); resolve(v); }
      $('sd-note-cancel').onclick = function () { finish(null); };
      dialog.oncancel = function (e) { e.preventDefault(); finish(null); };
      $('sd-note-form').onsubmit = function (e) { e.preventDefault(); finish(input.value.trim()); };
      dialog.showModal(); input.focus();
    });
  }

  // Runs an action for a card, then reloads; errors go to the status line.
  async function act(button, fn) {
    var buttons = button ? button.parentNode.querySelectorAll('button') : [];
    buttons.forEach(function (b) { b.disabled = true; });
    try { await fn(); say(''); await load(true); }
    catch (err) { say(err.message || 'Something went wrong.', true); buttons.forEach(function (b) { b.disabled = false; }); }
  }
  function btn(label, cls, fn) {
    var b = h('button', { type: 'button', class: cls || '', text: label });
    b.addEventListener('click', function () { act(b, function () { return fn(b); }); });
    return b;
  }

  // ---- queue renderers -------------------------------------------------------
  // Each returns an element for one item. `old` marks items waiting > 24 h.
  function card(ts, kids, actions) {
    var c = h('div', { class: 'sd-item' + (ts && Date.now() - ts > OLD_MS ? ' old' : '') }, kids);
    if (actions && actions.length) c.appendChild(h('div', { class: 'sd-actions' }, actions));
    return c;
  }
  function head(title, ts, extra) {
    return h('div', { class: 'sd-item-head' }, [h('div', { class: 'sd-item-title' }, [title].concat(extra || [])), age(ts)]);
  }

  function matrixCard(r, closed) {
    var where = r.roomName || r.roomId;
    var kids = [
      head(r.eventId ? 'Reported message' : 'Reported room', r.createdAt, [h('span', { class: 'sd-flag', text: where })]),
      h('div', { class: 'sd-meta', text: 'Reason: ' + (r.reason || 'none given') }),
      h('div', { class: 'sd-meta', text: 'Reported by ' + (r.reportedBy || 'unknown') + (r.sender ? ' · written by ' + r.sender : '') }),
    ];
    if (r.eventId) {
      kids.push(r.body
        ? h('div', { class: 'sd-quote', text: r.body })
        : h('div', { class: 'sd-quote missing', text: r.sender ? 'No text (image, file, or already deleted).' : 'The message could not be loaded. It may already be deleted.' }));
    }
    if (closed) {
      kids.push(h('div', { class: 'sd-meta', text: (r.status === 'dismissed' ? 'Dismissed' : 'Resolved') + ' by ' + (r.handledBy || 'staff') + (r.note ? ' — ' + r.note : '') }));
      return card(0, kids, [btn('Reopen', '', function () { return setMatrix(r.id, 'open', ''); })]);
    }
    var actions = [];
    if (r.eventId) {
      actions.push(btn('Delete message', 'danger', async function () {
        if (!confirm('Delete this message from the chat? This cannot be undone.')) throw new Error('');
        await api('/api/matrix/moderation/redact', { eventId: r.eventId, roomId: r.roomId, reason: 'Removed after a report' });
        await setMatrix(r.id, 'resolved', 'Message deleted');
      }));
    }
    if (r.sender) {
      actions.push(btn('Ban sender from chat', '', async function () {
        if (!confirm('Ban ' + r.sender + ' from chat?')) throw new Error('');
        await api('/api/matrix/moderation/ban', { userId: r.sender, reason: 'Banned after a report' });
        await setMatrix(r.id, 'resolved', 'Sender banned');
      }));
    }
    actions.push(btn('Resolve', 'primary', function () { return setMatrix(r.id, 'resolved', ''); }));
    actions.push(btn('Dismiss', '', async function () {
      var note = await askNote('Dismiss report', 'Dismiss');
      if (note === null) throw new Error('');
      return setMatrix(r.id, 'dismissed', note);
    }));
    return card(r.createdAt, kids, actions);
  }
  function setMatrix(id, status, note) {
    return api('/api/admin/staff-dashboard/matrix-reports/status', { id: id, status: status, note: note });
  }

  function chatCard(r) {
    var ctx = (r.context || []).map(function (m) {
      return h('li', { class: m.reported ? 'reported' : '' }, [h('b', { text: m.from || '?' }), m.text || '']);
    });
    return card(r.ts, [
      head('Chat report', r.ts),
      h('div', { class: 'sd-meta', text: 'Reason: ' + (r.reason || 'none given') }),
      h('div', { class: 'sd-meta', text: 'Reported by ' + (r.reportedBy || 'unknown') }),
      ctx.length ? h('ul', { class: 'sd-ctx' }, ctx) : null,
    ], [
      btn('Resolve', 'primary', function () { return api('/api/admin/chat-reports/resolve', { ts: r.ts, action: 'resolve' }); }),
      btn('Delete', '', async function () {
        if (!confirm('Delete this report?')) throw new Error('');
        await api('/api/admin/chat-reports/resolve', { ts: r.ts, action: 'delete' });
      }),
    ]);
  }

  function profileCard(r) {
    return card(r.ts, [
      head('Profile report' + (r.targetHandle ? ': ' + r.targetHandle : ''), r.ts),
      h('div', { class: 'sd-meta', text: 'Reason: ' + (r.reason || 'none given') }),
      r.details ? h('div', { class: 'sd-quote', text: r.details }) : null,
      h('div', { class: 'sd-meta', text: 'Reported by ' + (r.reporter || 'unknown') + ' · account ' + (r.target || 'unknown') }),
    ], [
      btn('Resolve', 'primary', function () { return api('/api/admin/profile-reports/resolve', { id: r.id, status: 'resolved', resolution: '' }); }),
      btn('Dismiss', '', async function () {
        var note = await askNote('Dismiss report', 'Dismiss');
        if (note === null) throw new Error('');
        await api('/api/admin/profile-reports/resolve', { id: r.id, status: 'dismissed', resolution: note });
      }),
    ]);
  }

  function outsideCard(r) {
    function decide(ok) { return function () { return api('/api/admin/matrix/outside-accounts', { email: r.email, approved: ok }); }; }
    return card(r.requestedAt, [
      head(r.email, r.requestedAt),
      h('div', { class: 'sd-meta', text: 'Outside account waiting for chat access.' }),
    ], [btn('Approve', 'primary', decide(true)), btn('Deny', '', decide(false))]);
  }

  function ticketCard(t) {
    var cat = { signin: 'Can’t sign in', compromised: 'Account compromised', email: 'Email change', banned: 'Ban or restriction', other: 'Other' }[t.category] || t.category;
    return card(t.updatedAt, [
      head(t.subject || cat, t.updatedAt, [
        t.compromised ? h('span', { class: 'sd-flag warn', text: 'Compromised' }) : null,
        t.assignedTo ? h('span', { class: 'sd-flag', text: 'Claimed' }) : null,
      ]),
      h('div', { class: 'sd-meta', text: cat + (t.claimedAccount ? ' · ' + t.claimedAccount : '') }),
    ], [h('a', { href: '/moderator/support/', class: 'primary', text: 'Open ticket' })]);
  }

  function emailRequestCard(r) {
    return card(r.requestedAt, [
      head('Email change request', r.requestedAt),
      h('div', { class: 'sd-meta', text: 'Requested by ' + r.requestedBy }),
      r.reason ? h('div', { class: 'sd-quote', text: r.reason }) : null,
    ], [h('a', { href: '/moderator/support/', class: 'primary', text: 'Review' })]);
  }

  function feedbackCard(e) {
    return card(e.createdAt, [
      head((e.type || 'feedback').replace(/_/g, ' '), e.createdAt),
      h('div', { class: 'sd-quote', text: e.text }),
      h('div', { class: 'sd-meta', text: e.account ? 'From ' + e.account : 'From a visitor' }),
    ], [btn('Mark handled', 'primary', function () { return api('/api/admin/feedback/status', { id: e.id, status: 'handled' }); })]);
  }

  function noteCard(n) {
    function review(status) { return function () { return api('/api/admin/teachers/note-status', { id: n.id, status: status }); }; }
    return card(n.createdAt, [
      head(n.teacher, n.createdAt, [h('span', { class: 'sd-flag', text: n.school })]),
      h('div', { class: 'sd-quote', text: n.text }),
    ], [btn('Approve', 'primary', review('approved')), btn('Reject', '', review('rejected'))]);
  }

  function modRequestCard(r) {
    return card(r.requestedAt, [
      head(r.label || r.action, r.requestedAt),
      h('div', { class: 'sd-meta', text: 'Requested by ' + (r.requestedBy || r.requestedByEmail) }),
      h('div', { class: 'sd-quote', text: JSON.stringify(r.payload || {}, null, 1) }),
    ], [
      btn('Approve', 'primary', function () { return api('/api/admin/moderator-requests/resolve', { id: r.id, decision: 'approve', note: '' }); }),
      btn('Reject', '', async function () {
        var note = await askNote('Reject request', 'Reject');
        if (note === null) throw new Error('');
        await api('/api/admin/moderator-requests/resolve', { id: r.id, decision: 'reject', note: note });
      }),
    ]);
  }

  // key, tab label, section title, counts key, queue key, renderer, admin only
  var QUEUES = [
    { key: 'matrix', label: 'Matrix reports', title: 'Matrix chat reports', count: 'matrixReports', render: function (r) { return matrixCard(r, false); }, empty: 'No open Matrix reports.' },
    { key: 'chat', label: 'Chat reports', title: 'Chat and DM reports', count: 'chatReports', render: chatCard, empty: 'No open chat reports.' },
    { key: 'profile', label: 'Profile reports', title: 'Profile reports', count: 'profileReports', render: profileCard, empty: 'No open profile reports.' },
    { key: 'outside', label: 'Chat access', title: 'Chat access requests', count: 'outsideAccounts', queue: 'outsideAccounts', render: outsideCard, empty: 'Nobody is waiting for chat access.' },
    { key: 'support', label: 'Support', title: 'Support tickets', count: 'supportTickets', render: ticketCard, more: { href: '/moderator/support/', label: 'Open support tools' }, empty: 'No open support tickets.' },
    { key: 'email', label: 'Email changes', title: 'Email change requests', count: 'emailRequests', render: emailRequestCard, admin: true, empty: 'No email changes waiting.' },
    { key: 'feedback', label: 'Feedback', title: 'Feedback', count: 'feedback', render: feedbackCard, more: { href: '/moderator/feedback/', label: 'All feedback' }, empty: 'No new feedback.' },
    { key: 'notes', label: 'Teacher notes', title: 'Teacher notes', count: 'teacherNotes', render: noteCard, more: { href: '/moderator/teachers/', label: 'Teacher tools' }, empty: 'No notes waiting for review.' },
    { key: 'modreq', label: 'Moderator requests', title: 'Moderator requests', count: 'moderatorRequests', render: modRequestCard, admin: true, empty: 'No moderator requests.' },
  ];
  function queueItems(q) { return (data.queues[q.queue || q.count] || []); }
  function visibleQueues() { return QUEUES.filter(function (q) { return !q.admin || (data.viewer && data.viewer.isAdmin); }); }

  // ---- layout ------------------------------------------------------------------
  function renderTabs() {
    var tabs = $('sd-tabs'); tabs.textContent = '';
    function tab(key, label, n) {
      var b = h('button', { type: 'button', class: 'sd-tab' + (n ? ' has-items' : ''), 'aria-pressed': String(current === key) }, [label, h('span', { class: 'sd-count', text: String(n) })]);
      b.addEventListener('click', function () { current = key; render(); });
      tabs.appendChild(b);
    }
    tab('all', 'Needs attention', data.total);
    visibleQueues().forEach(function (q) { tab(q.key, q.label, data.counts[q.count] || 0); });
  }

  function renderQueue(q, single) {
    var count = data.counts[q.count] || 0;
    var items = queueItems(q);
    var sec = h('section', { class: 'sd-queue', 'aria-label': q.title });
    var title = h('h2', null, [q.title, h('span', { class: 'sd-count', text: String(count) })]);
    var side = [];
    if (q.more) side.push(h('a', { href: q.more.href, text: q.more.label }));
    if (q.key === 'matrix') {
      side.push(h('button', { type: 'button', class: 'sd-link', text: showClosedMatrix ? 'Hide resolved' : 'Show resolved', onclick: toggleClosedMatrix }));
    }
    sec.appendChild(h('div', { class: 'sd-queue-head' }, [title, h('span', null, side.length > 1 ? [side[0], '  ', side[1]] : side)]));
    if (!items.length) sec.appendChild(h('div', { class: 'sd-empty', text: q.empty }));
    items.forEach(function (it) { sec.appendChild(q.render(it)); });
    if (count > items.length) sec.appendChild(h('p', { class: 'sd-more', text: 'Showing the first ' + items.length + ' of ' + count + '. Handle these to see the rest.' }));
    if (q.key === 'matrix' && showClosedMatrix) {
      sec.appendChild(h('h3', { class: 'sd-meta', text: 'Recently resolved' }));
      if (!closedMatrix.length) sec.appendChild(h('div', { class: 'sd-empty', text: 'Nothing resolved yet.' }));
      closedMatrix.slice(0, 30).forEach(function (r) { sec.appendChild(matrixCard(r, true)); });
    }
    return sec;
  }

  async function toggleClosedMatrix() {
    showClosedMatrix = !showClosedMatrix;
    if (showClosedMatrix) {
      try { closedMatrix = (await api('/api/admin/staff-dashboard/matrix-reports?filter=closed')).reports || []; }
      catch (err) { say(err.message, true); showClosedMatrix = false; }
    }
    render();
  }

  function render() {
    renderTabs();
    var box = $('sd-queues'); box.textContent = '';
    var list = visibleQueues();
    if (current === 'all') {
      var any = false;
      list.forEach(function (q) { if (data.counts[q.count]) { any = true; box.appendChild(renderQueue(q)); } });
      if (!any) box.appendChild(h('div', { class: 'sd-empty', text: 'Nothing is waiting on staff.' }));
    } else {
      var q = list.filter(function (x) { return x.key === current; })[0];
      if (q) box.appendChild(renderQueue(q, true)); else { current = 'all'; render(); return; }
    }
    var total = $('sd-total');
    total.hidden = false;
    total.textContent = data.total ? data.total + ' waiting' : 'All clear';
    document.title = (data.total ? '(' + data.total + ') ' : '') + 'Staff dashboard — mitch.pro';
  }

  // ---- tools -----------------------------------------------------------------
  function renderTools() {
    var admin = !!role.isAdmin, mod = admin || !!role.isModerator;
    var groups = [
      { name: 'Moderation', tools: [
        { href: '/moderator/', name: 'Moderator panel', desc: 'Chat moderation, bans and audit tools', ok: mod },
        { href: '/moderator/support/', name: 'Support tickets', desc: 'Chat with people who are locked out', ok: mod },
        { href: '/moderator/feedback/', name: 'Feedback', desc: 'Everything sent from the feedback page', ok: mod },
        { href: '/moderator/teachers/', name: 'Teacher ratings', desc: 'Notes and the teacher list', ok: mod },
        { href: '/admin/special-schedule/', name: 'Special bell schedule', desc: 'Override the day’s schedule', ok: mod },
      ] },
      { name: 'Administration', tools: [
        { href: '/admin/', name: 'Admin panel', desc: 'Economy, accounts, logs and the rest', ok: admin },
        { href: '/admin/bans/', name: 'Account bans', desc: 'Review bans and restore accounts', ok: admin },
        { href: '/admin/vms/', name: 'Virtual machines', desc: 'Computers and resources', ok: admin },
        { href: '/admin/email/', name: 'Email and newsletters', desc: 'Compose and send site email', ok: admin },
        { href: '/admin/updates/', name: 'Updates', desc: 'Homepage update log', ok: admin },
        { href: '/admin/companion/', name: 'Companion characters', desc: 'Preset characters and review queue', ok: admin },
      ] },
      { name: 'Testing', tools: [
        { href: '/tester/', name: 'Beta tester hub', desc: 'Tester tools and cooldown bypasses', ok: !!role.isTester },
      ] },
    ];
    if (role.isOwner) groups.push({ name: 'Owner', tools: [{ href: '/vms/desktop/?container=owner', name: 'Container 500', desc: 'Root-only desktop', ok: true }] });
    var box = $('sd-tool-list'); box.textContent = '';
    groups.forEach(function (g) {
      var grid = h('div', { class: 'sd-tool-grid' });
      g.tools.forEach(function (t) {
        var el = h(t.ok ? 'a' : 'div', { class: 'sd-tool', href: t.ok ? t.href : false, 'data-locked': t.ok ? false : '' }, [h('strong', { text: t.name }), h('span', { text: t.desc })]);
        grid.appendChild(el);
      });
      box.appendChild(h('div', { class: 'sd-tool-group' }, [h('h3', { text: g.name }), grid]));
    });
  }

  // ---- loading -----------------------------------------------------------------
  async function load(quiet) {
    if (!quiet) say('Loading...');
    try {
      data = await api('/api/admin/staff-dashboard/summary');
      if (showClosedMatrix) closedMatrix = (await api('/api/admin/staff-dashboard/matrix-reports?filter=closed')).reports || [];
      $('sd-app').hidden = false;
      if (!quiet) say('');
      render();
    } catch (err) {
      say(err.message || 'Could not load the dashboard.', true);
    }
  }

  async function start() {
    try {
      var res = await fetch('/api/staff-hub/status', { credentials: 'same-origin', cache: 'no-store' });
      if (res.status === 401) { location.href = '/enroll/?next=' + encodeURIComponent('/staff-dashboard/'); return; }
      if (!res.ok) throw new Error('status ' + res.status);
      role = await res.json();
    } catch (err) {
      say('Could not load staff status.', true);
      return;
    }
    if (!role.isStaff) { say('Staff access required.', true); return; }
    renderToolsOnly();
    if (!role.isAdmin && !role.isModerator) { say('The queues are for moderators and admins.'); return; }
    await load(false);
  }
  function renderToolsOnly() { $('sd-app').hidden = false; renderTools(); }

  $('sd-refresh').addEventListener('click', function () { load(false); });
  document.addEventListener('visibilitychange', function () { if (!document.hidden && data) load(true); });
  setInterval(function () { if (!document.hidden && data && !document.querySelector('dialog[open]')) load(true); }, 60000);
  start();
})();
