/* Overview section: everything waiting on staff, handled inline where possible.
   Data comes from GET /api/admin/staff-dashboard/summary; each action calls the
   existing endpoint for that kind of item. See docs/staff-dashboard.md. */
(function () {
  'use strict';
  var h = SD.h, api = SD.api;
  var OLD_MS = 24 * 3600 * 1000;
  var data = null;          // last summary
  var current = 'all';      // selected queue tab
  var showClosedMatrix = false;
  var closedMatrix = [];
  var root = null;

  function say(t, bad) { SD.say(t, bad); }
  function ago(ms) { return SD.ago(ms); }
  function age(ms) { return h('span', { class: 'sd-age', title: ms ? new Date(ms).toLocaleString() : '', text: ago(ms) }); }
  function askNote(title, okLabel) { return SD.prompt(title, okLabel); }

  // Runs an action for a card, then reloads; errors go to the status line.
  async function act(button, fn) {
    var buttons = button ? button.parentNode.querySelectorAll('button') : [];
    buttons.forEach(function (b) { b.disabled = true; });
    try { await fn(); say(''); await load(); }
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
        if (!await SD.confirm('Delete message','Delete this message from the chat? This cannot be undone.',{danger:true,ok:'Delete'})) throw new Error('');
        await api('/api/matrix/moderation/redact', { eventId: r.eventId, roomId: r.roomId, reason: 'Removed after a report' });
        await setMatrix(r.id, 'resolved', 'Message deleted');
      }));
    }
    if (r.sender) {
      actions.push(btn('Ban sender from chat', '', async function () {
        if (!await SD.confirm('Ban sender','Ban ' + r.sender + ' from chat?',{danger:true,ok:'Ban'})) throw new Error('');
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
        if (!await SD.confirm('Delete report','Delete this report?',{danger:true,ok:'Delete'})) throw new Error('');
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
    var tabs = h('nav', { class: 'sd-tabs', 'aria-label': 'Queues' });
    function tab(key, label, n) {
      var b = h('button', { type: 'button', class: 'sd-tab' + (n ? ' has-items' : ''), 'aria-pressed': String(current === key) }, [label, h('span', { class: 'sd-count', text: String(n) })]);
      b.addEventListener('click', function () { current = key; render(); });
      tabs.appendChild(b);
    }
    tab('all', 'Needs attention', data.total);
    visibleQueues().forEach(function (q) { tab(q.key, q.label, data.counts[q.count] || 0); });
    return tabs;
  }

  function renderQueue(q) {
    var count = data.counts[q.count] || 0;
    var items = queueItems(q);
    var sec = h('section', { class: 'sd-queue', 'aria-label': q.title });
    var side = [];
    if (q.more) side.push(h('a', { href: q.more.href, text: q.more.label }));
    if (q.key === 'matrix') side.push(h('button', { type: 'button', class: 'sd-link', text: showClosedMatrix ? 'Hide resolved' : 'Show resolved', onclick: toggleClosedMatrix }));
    sec.appendChild(h('div', { class: 'sd-queue-head' }, [h('h2', null, [q.title, h('span', { class: 'sd-count', text: String(count) })]), h('span', { class: 'sd-queue-side' }, side)]));
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
    root.textContent = '';
    if (!SD.can('mod')) {
      root.appendChild(SD.ui.empty('The queues are for moderators and admins. Use the links on the left.'));
      return;
    }
    root.appendChild(renderTabs());
    var box = h('div');
    var list = visibleQueues();
    if (current === 'all') {
      var any = false;
      list.forEach(function (q) { if (data.counts[q.count]) { any = true; box.appendChild(renderQueue(q)); } });
      if (!any) box.appendChild(h('div', { class: 'sd-empty', text: 'Nothing is waiting on staff.' }));
    } else {
      var q = list.filter(function (x) { return x.key === current; })[0];
      if (q) box.appendChild(renderQueue(q)); else { current = 'all'; render(); return; }
    }
    root.appendChild(box);
    var total = SD.$('sd-total');
    total.hidden = false;
    total.textContent = data.total ? data.total + ' waiting' : 'All clear';
    document.title = (data.total ? '(' + data.total + ') ' : '') + 'Overview — mitch.pro';
    SD.badge('overview', data.total);
  }

  async function load() {
    data = await api('/api/admin/staff-dashboard/summary');
    if (showClosedMatrix) closedMatrix = (await api('/api/admin/staff-dashboard/matrix-reports?filter=closed')).reports || [];
    render();
  }

  SD.register({
    key: 'overview',
    render: async function (el) {
      root = el;
      if (!SD.can('mod')) { render(); return; }
      say('Loading...');
      await load();
      say('');
    },
  });
  SD.refreshOverview = function () { if (SD.can('mod')) return load().catch(function () {}); };
})();
