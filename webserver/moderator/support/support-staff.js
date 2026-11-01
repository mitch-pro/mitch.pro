(function () {
  'use strict';
  var BASE = '/api/admin/support/';
  var $ = function (id) { return document.getElementById(id); };
  var state = { filter: 'open', current: '', isAdmin: false };
  var CATEGORY = { signin: 'Can\'t sign in', compromised: 'Account compromised', email: 'Email change', banned: 'Ban / restriction', other: 'Other' };

  function esc(v) { return String(v == null ? '' : v).replace(/[&<>"']/g, function (c) { return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]; }); }
  function say(text, bad) { var s = $('status'); s.className = 'support-status' + (bad ? ' err' : ''); s.textContent = text || ''; }
  function headers(extra) {
    var h = Object.assign({ 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, extra || {});
    var pass = sessionStorage.getItem('admin_passphrase');
    if (pass) h['X-Admin-Passphrase'] = pass;
    return h;
  }

  // Admins must unlock with their passphrase; moderators never see this.
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

  function when(ms) { return new Date(ms).toLocaleString(); }

  function renderTabs() {
    var tabs = $('tabs'); tabs.textContent = '';
    ['open', 'pending', 'resolved', 'all'].forEach(function (f) {
      var b = document.createElement('button');
      b.type = 'button'; b.textContent = f[0].toUpperCase() + f.slice(1);
      b.setAttribute('aria-pressed', String(state.filter === f));
      b.onclick = function () { state.filter = f; loadList(); };
      tabs.appendChild(b);
    });
  }

  async function loadList() {
    renderTabs();
    try {
      var d = await api('tickets?status=' + encodeURIComponent(state.filter));
      var box = $('tickets'); box.textContent = '';
      if (!d.tickets.length) box.innerHTML = '<p class="support-sub">No tickets.</p>';
      d.tickets.forEach(function (t) {
        var b = document.createElement('button');
        b.type = 'button'; b.className = 'staff-ticket';
        b.setAttribute('aria-current', String(t.id === state.current));
        b.innerHTML = '<strong>' + esc(t.subject || 'Request') + '</strong>' +
          '<small>' + esc(CATEGORY[t.category] || t.category) + (t.compromised ? ' · compromised' : '') + (t.emailRequestPending ? ' · email change pending' : '') + '</small>' +
          '<small>' + esc(t.claimedAccount) + ' · ' + esc(when(t.updatedAt)) + '</small>';
        b.onclick = function () { state.current = t.id; loadDetail(); loadList(); };
        box.appendChild(b);
      });
    } catch (err) { say(err.message, true); }
  }

  async function loadDetail() {
    if (!state.current) return;
    try {
      var d = await api('ticket?id=' + encodeURIComponent(state.current));
      renderDetail(d);
    } catch (err) { say(err.message, true); }
  }

  function renderDetail(d) {
    var t = d.ticket, a = d.account;
    var acct = a
      ? 'Linked account: <b>' + esc(a.username || a.displayName || 'unknown') + '</b> (' + esc(a.emailMasked) + ')' +
        (a.emailChangeBlockedReason ? '<br><span class="staff-warn">Email can\'t be changed here: ' + esc(a.emailChangeBlockedReason) + '</span>' : '')
      : '<span class="staff-warn">No account matched what they typed. Do not guess: ask them for details.</span>';
    var thread = d.messages.map(function (m) {
      var who = m.from === 'user' ? 'Requester' : (m.from === 'staff' ? (m.staff || 'Staff') : '');
      return '<div class="support-msg ' + esc(m.from === 'staff' ? 'user' : m.from === 'user' ? '' : 'system') + '">' +
        (who ? '<small>' + esc(who) + ' · ' + esc(when(m.at)) + '</small>' : '') + esc(m.body) + '</div>';
    }).join('');
    var reqs = d.emailRequests.map(function (r) {
      return '<div class="staff-box"><b>Email change:</b> ' + esc(r.oldEmail) + ' → ' + esc(r.newEmail) + ' · <span class="support-pill">' + esc(r.status) + '</span><br>' +
        'Requested by ' + esc(r.requestedBy) + ': ' + esc(r.reason) +
        (r.decidedBy ? '<br>Decided by ' + esc(r.decidedBy) + (r.decisionNote ? ': ' + esc(r.decisionNote) : '') : '') + '</div>';
    }).join('');
    var open = d.emailRequests.some(function (r) { return r.status === 'pending' || r.status === 'approved'; });
    var canRequest = a && !a.emailChangeBlockedReason && !open;
    $('detail').innerHTML =
      '<h2>' + esc(t.subject || 'Request') + '</h2>' +
      '<div class="staff-meta">' + esc(CATEGORY[t.category] || t.category) + ' · typed: ' + esc(t.claimedAccount) +
        (t.contactEmail ? ' · contact: ' + esc(t.contactEmail) : '') + '<br>' + acct + '</div>' +
      '<div class="staff-row">' +
        '<button class="support-btn" data-act="status" data-value="open">Open</button>' +
        '<button class="support-btn" data-act="status" data-value="pending">Pending</button>' +
        '<button class="support-btn" data-act="status" data-value="resolved">Resolve</button>' +
        '<button class="support-btn" data-act="assign">' + (t.assignedTo ? 'Release' : 'Claim') + '</button>' +
        '<label><input type="checkbox" id="compromised"' + (t.compromised ? ' checked' : '') + '> Account compromised</label>' +
      '</div>' +
      '<div class="support-thread">' + thread + '</div>' +
      '<form id="reply-form"><textarea id="reply-body" class="staff-input" rows="3" maxlength="4000" placeholder="Reply to the requester"></textarea>' +
        '<div class="staff-row"><button class="support-btn primary" type="submit">Send reply</button></div></form>' +
      reqs +
      (canRequest
        ? '<form id="req-form" class="staff-box"><b>Request an email change</b><p class="support-sub">An admin has to approve it, and the person must confirm a code sent to the new address. Say how you confirmed they own the account.</p>' +
          '<input id="req-email" class="staff-input" type="email" placeholder="New email address" required style="margin-bottom:8px">' +
          '<textarea id="req-reason" class="staff-input" rows="2" maxlength="500" placeholder="How you verified them" required></textarea>' +
          '<div class="staff-row"><button class="support-btn" type="submit">Send to admins</button></div></form>'
        : '');
    var root = $('detail');
    root.querySelectorAll('[data-act]').forEach(function (b) {
      b.onclick = async function () {
        try {
          if (b.dataset.act === 'status') await api('status', { id: t.id, status: b.dataset.value });
          else await api('assign', { id: t.id, assign: !t.assignedTo });
          await loadDetail(); loadList();
        } catch (err) { say(err.message, true); }
      };
    });
    $('compromised').onchange = async function (e) {
      try { await api('flag', { id: t.id, compromised: e.target.checked }); await loadDetail(); loadList(); } catch (err) { say(err.message, true); }
    };
    $('reply-form').onsubmit = async function (e) {
      e.preventDefault();
      var body = $('reply-body').value.trim();
      if (!body) return;
      try { await api('reply', { id: t.id, body: body }); say(''); await loadDetail(); loadList(); } catch (err) { say(err.message, true); }
    };
    if ($('req-form')) $('req-form').onsubmit = async function (e) {
      e.preventDefault();
      try {
        await api('email-request', { id: t.id, newEmail: $('req-email').value, reason: $('req-reason').value });
        say('Sent to admins.'); await loadDetail(); loadApprovals();
      } catch (err) { say(err.message, true); }
    };
  }

  async function loadApprovals() {
    if (!state.isAdmin) return;
    try {
      var d = await api('email-requests');
      $('approvals').hidden = !d.requests.length;
      var box = $('approval-list'); box.textContent = '';
      d.requests.forEach(function (r) {
        var div = document.createElement('div');
        div.className = 'staff-box';
        div.innerHTML = esc(r.oldEmail) + ' → <b>' + esc(r.newEmail) + '</b> · requested by ' + esc(r.requestedBy) + '<br>' +
          'Ticket: ' + esc((r.ticket && r.ticket.subject) || r.ticketId) + ' · ' + esc((r.ticket && r.ticket.compromised) ? 'marked compromised' : 'not marked compromised') + '<br>' +
          'Reason: ' + esc(r.reason) +
          '<input class="staff-input" placeholder="Note (required to deny)" style="margin:8px 0" maxlength="500">' +
          '<div class="staff-row"><button class="support-btn primary" data-approve="1">Approve</button><button class="support-btn" data-approve="0">Deny</button> <button class="support-btn" data-open="1">Open ticket</button></div>';
        div.querySelectorAll('[data-approve]').forEach(function (b) {
          b.onclick = async function () {
            try {
              await api('email-request/decide', { requestId: r.id, approve: b.dataset.approve === '1', note: div.querySelector('input').value });
              say(b.dataset.approve === '1' ? 'Approved. The requester was emailed a code at the new address.' : 'Denied.');
              await loadApprovals(); await loadList(); await loadDetail();
            } catch (err) { say(err.message, true); }
          };
        });
        div.querySelector('[data-open]').onclick = function () { state.current = r.ticketId; loadDetail(); loadList(); };
        box.appendChild(div);
      });
    } catch (err) { say(err.message, true); }
  }

  async function init() {
    var me = await fetch('/api/me', { credentials: 'same-origin' }).then(function (r) { return r.ok ? r.json() : {}; }).catch(function () { return {}; });
    var isAdmin = !!(me.isAdmin || me.isOwner || me.isCoOwner);
    if (!isAdmin && !me.isModerator) { $('denied').hidden = false; return; }
    state.isAdmin = isAdmin;
    $('app').hidden = false;
    await loadList();
    await loadApprovals();
    setInterval(function () {
      if (document.hidden) return;
      loadList(); loadApprovals();
      if (state.current && !(document.activeElement && /TEXTAREA|INPUT/.test(document.activeElement.tagName))) loadDetail();
    }, 15000);
  }
  init();
})();
