/* Premium email requests: approve or deny. Admins only.
   Create the mailbox on the mail host first, then approve: approving emails
   the member that their address is ready. See docs/premium-email.md. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  function badge(r) {
    var map = { pending: ['Pending', 'warn'], approved: ['Approved', 'ok'], denied: ['Denied', 'warn'] };
    var m = map[r.status] || [r.status, ''];
    return ui.pill(m[0], m[1]);
  }

  async function render(root) {
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    function say(t, bad) { status.className = 'sd-form-status' + (bad ? ' err' : t ? ' ok' : ''); status.textContent = t || ''; }
    root.appendChild(status);

    var data = await SD.api('/api/admin/premium-email');
    var requests = data.requests || [];

    async function decide(r, action, note, button) {
      button.disabled = true;
      say(action === 'approve' ? 'Approving...' : 'Denying...');
      try {
        await SD.api('/api/admin/premium-email/decide', { id: r.id, action: action, note: note || '' });
        SD.show();
      } catch (e) { say(e.message, true); button.disabled = false; }
    }

    var rows = requests.map(function (r) {
      var actions = [];
      if (r.status === 'pending') {
        actions.push(
          ui.button('Approve', 'primary', async function (b) {
            if (!(await SD.confirm('Approve ' + r.address, 'Create the mailbox first. The member is emailed when you approve.', { ok: 'Approve' }))) return;
            decide(r, 'approve', '', b);
          }),
          ui.button('Deny', 'danger', async function (b) {
            var note = await SD.prompt('Deny ' + r.address, 'Deny', { message: 'Optional reason, included in the email to the member.' });
            if (note === null) return;
            decide(r, 'deny', note, b);
          }));
      } else if (r.decidedBy) {
        actions.push(h('span', { class: 'sd-meta', text: SD.time(r.decidedAt) + ' by ' + r.decidedBy + (r.note ? ' — ' + r.note : '') }));
      }
      return [
        h('div', null, [h('strong', { text: r.address }), h('div', { class: 'sd-meta', text: r.email + ' · ' + SD.time(r.createdAt) })]),
        r.fullName,
        r.reasons,
        badge(r),
        h('div', { class: 'sd-actions' }, actions),
      ];
    });
    root.appendChild(ui.table(['Address', 'Name', 'Reason', 'Status', ''], rows, 'No requests yet.'));
  }

  SD.register({ key: 'premium-email', render: render });
})();
