/* Computer (VM) requests: approve, deny, delete, restore and purge. Admins only.
   Day-to-day computer management is on the Computers page. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;
  var failed = {};   // emails whose last delete failed, so "Force delete" is offered

  function badge(app) {
    var map = { pending: ['Pending', 'warn'], approved: ['Approved' + (app.vmid ? ' (VM ' + app.vmid + ')' : ''), 'ok'], denied: ['Denied', 'warn'] };
    if (app.status === 'deleted') {
      var left = Math.max(0, 7 - (Date.now() - app.deletedAt) / 86400000).toFixed(1);
      return ui.pill('Deleted (' + left + ' days to restore)', 'warn');
    }
    var m = map[app.status] || [app.status, ''];
    return ui.pill(m[0], m[1]);
  }

  async function render(root) {
    ui.add(root, [ui.heading('Computer requests', 'Requests for Linux containers and dedicated VMs.')]);
    root.appendChild(h('div', { class: 'sd-actions' }, [h('a', { class: 'sd-link', href: '/staff-dashboard/vms/', text: 'Manage computers' })]));
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    function say(t, bad) { status.className = 'sd-form-status' + (bad ? ' err' : t ? ' ok' : ''); status.textContent = t || ''; }
    root.appendChild(status);

    var requests = (await SD.api('/api/admin/vm-requests')).requests || {};
    var order = { pending: 1, approved: 2, deleted: 3, denied: 4 };
    var entries = Object.keys(requests).map(function (k) { return requests[k]; }).sort(function (a, b) { return (order[a.status] || 5) - (order[b.status] || 5); });

    async function call(url, body, busy) {
      say(busy);
      try { var d = await SD.api(url, body); delete failed[body.email]; SD.show('vms'); return d; }
      catch (e) { failed[body.email] = true; say(e.message, true); }
    }
    function run(label, danger, confirm, fn) {
      return ui.button(label, danger ? 'danger' : '', async function (b) {
        if (confirm && !(await SD.confirm(label, confirm, { danger: danger }))) return;
        b.disabled = true; await fn(); b.disabled = false;
      });
    }
    var rows = entries.map(function (app) {
      var actions = [];
      if (app.status === 'pending') {
        var vmid = h('input', { class: 'sd-input', type: 'number', min: 200, max: 999, value: 200 + Math.floor(Math.random() * 100), 'aria-label': 'VM ID', style: 'width:5.5em' });
        actions.push(vmid,
          ui.button('Approve', 'primary', async function (b) {
            var n = parseInt(vmid.value, 10);
            if (isNaN(n) || n < 200 || n > 999) return say('Enter a VM ID from 200 to 999.', true);
            b.disabled = true; await call('/api/admin/approve-vm', { email: app.email, vmid: n }, 'Approving and cloning the template...'); b.disabled = false;
          }),
          run('Deny', true, 'Deny this request?', function () { return call('/api/admin/deny-vm', { email: app.email }, 'Denying...'); }));
      } else if (app.status === 'approved') {
        actions.push(run('Delete VM', true, 'Stop and delete this VM? It can be restored for 7 days.', function () { return call('/api/admin/terminate-vm', { email: app.email, force: false }, 'Stopping the VM...'); }));
        if (failed[app.email]) actions.push(run('Force delete', true, 'Ignore Proxmox errors and delete the VM anyway?', function () { return call('/api/admin/terminate-vm', { email: app.email, force: true }, 'Force deleting...'); }));
      } else if (app.status === 'deleted') {
        if (Date.now() - app.deletedAt < 7 * 86400000) actions.push(run('Undo delete', false, '', function () { return call('/api/admin/restore-vm', { email: app.email }, 'Restoring...'); }));
        actions.push(run('Purge', true, 'Permanently destroy this VM? This cannot be undone.', function () { return call('/api/admin/purge-vm', { email: app.email, force: false }, 'Purging...'); }));
        if (failed[app.email]) actions.push(run('Force purge', true, 'Ignore Proxmox errors and delete the record anyway?', function () { return call('/api/admin/purge-vm', { email: app.email, force: true }, 'Force purging...'); }));
      }
      return [
        h('div', null, [h('strong', { text: app.email }), h('div', { class: 'sd-meta', text: 'Applied ' + SD.time(app.appliedAt) })]),
        (app.tier === 'paid' ? 'Dedicated VM' : 'Linux container') + (app.priceUsd === 0 ? ' · no cost' : ''),
        app.note || 'None', badge(app), h('div', { class: 'sd-actions' }, actions),
      ];
    });
    root.appendChild(ui.table(['Account', 'Type', 'Note', 'Status', ''], rows, 'No computer requests.'));
  }

  SD.register({ key: 'vms', label: 'Computers', access: 'admin', render: render });
})();
