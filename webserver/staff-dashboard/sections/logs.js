/* Logs and audit: server load, admin activity (with undo), sent notifications,
   bans, coin and anti-cheat logs, application and Docker logs. Admins only;
   application and Docker logs need developer or owner access. The old live
   traffic feed is gone: staff don't see what pages people visit. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;
  var REVERTIBLE = ['ban_account', 'unban_account', 'add_admin', 'remove_admin', 'add_co_owner', 'remove_co_owner', 'add_moderator', 'remove_moderator', 'shadow_ban', 'add_coins', 'gift_coins', 'matrix_ban_user', 'matrix_chat_ban', 'matrix_add_channel'];

  function resources() {
    var box = h('div', { class: 'sd-stats' });
    SD.api('/api/admin/resources').then(function (d) {
      var gb = function (n) { return (n / 1073741824).toFixed(2); };
      var rss = (d.memory && d.memory.rss) || 0, total = d.totalMem || 8 * 1073741824;
      var cpu = Math.round(((Array.isArray(d.load) ? d.load[0] || 0 : 0) / (d.cpus || 1)) * 100);
      ui.add(box, [ui.stat('Memory', gb(rss) + ' / ' + gb(total) + ' GB'), ui.stat('CPU load', cpu + '%', cpu > 85 ? 'bad' : '')]);
    }).catch(function (e) { box.appendChild(ui.empty(e.message)); });
    return box;
  }

  function undo(row) {
    return ui.button('Undo', '', async function (b) {
      if (!(await SD.confirm('Undo action', 'Reverse this action (' + row.action + ')?'))) return;
      b.disabled = true;
      try { var d = await SD.api('/api/admin/audit/revert', { id: row.id }); await SD.alert('Undone', d.message || 'Action reversed.'); SD.show(); }
      catch (e) { await SD.alert('Could not undo', e.message); b.disabled = false; }
    });
  }

  function logViewer(spec) {
    var out = ui.pre('Choose filters and load.');
    var status = h('p', { class: 'sd-form-status', role: 'status' });
    var inputs = {};
    var bar = h('div', { class: 'sd-toolbar' }, spec.filters.map(function (f) {
      var el = f.type === 'select' ? h('select', { class: 'sd-input' }, f.options.map(function (o) { return h('option', { value: o[0], text: o[1] }); })) : h('input', { class: 'sd-input', placeholder: f.placeholder || '' });
      if (f.value != null) el.value = f.value;
      inputs[f.name] = el;
      return h('label', { class: 'sd-field' }, [h('span', { text: f.label }), el]);
    }));
    bar.appendChild(ui.button('Load', 'primary', async function (b) {
      b.disabled = true; status.textContent = 'Loading...'; status.className = 'sd-form-status';
      try {
        var q = new URLSearchParams();
        Object.keys(inputs).forEach(function (k) { q.set(k, inputs[k].value); });
        var d = await SD.api(spec.url + '?' + q.toString());
        out.textContent = spec.text(d) || 'Nothing to show.';
        status.textContent = '';
      } catch (e) { out.textContent = ''; status.textContent = e.message; status.className = 'sd-form-status err'; }
      b.disabled = false;
    }));
    return ui.card(spec.title, spec.desc, [bar, status, out], 'wide');
  }

  // ---- searchable logs -------------------------------------------------------
  function ms(input) { return input.value ? String(new Date(input.value).getTime()) : ''; }
  function stamp(d) { var p = function (n) { return String(n).padStart(2, '0'); }; return d.getFullYear() + '-' + p(d.getMonth() + 1) + '-' + p(d.getDate()) + 'T' + p(d.getHours()) + ':' + p(d.getMinutes()); }

  // A log with search fields. spec: {title, desc, url, fields: [{name, label, type, options, placeholder}],
  //   columns, row(entry, filterBy) -> cells, empty, total label}. `from` and `to` (a "jump to" time:
  //   show entries at or before it) are always offered. Fetches `spec.url` with the filters.
  function searchLog(spec) {
    var inputs = {}, limit = 100;
    var fields = spec.fields.concat([
      { name: 'from', label: 'From', type: 'datetime-local' },
      { name: 'to', label: 'Jump to (at or before)', type: 'datetime-local' },
    ]).map(function (f) {
      var el = f.type === 'select' ? h('select', { class: 'sd-input' }, (f.options || []).map(function (o) { return h('option', { value: o[0], text: o[1] }); }))
        : h('input', { class: 'sd-input', type: f.type || 'text', placeholder: f.placeholder || false, min: f.min != null ? f.min : false, step: f.step || false });
      inputs[f.name] = el;
      return h('label', { class: 'sd-field' }, [h('span', { text: f.label }), el]);
    });
    var results = h('div'), count = h('p', { class: 'sd-meta' }), status = h('p', { class: 'sd-form-status', role: 'status' });
    function filterBy(name, value) { inputs[name].value = value; limit = 100; run(); }
    async function run() {
      status.className = 'sd-form-status'; status.textContent = 'Searching...';
      var q = new URLSearchParams();
      Object.keys(inputs).forEach(function (k) {
        var v = (k === 'from' || k === 'to') ? ms(inputs[k]) : inputs[k].value.trim();
        if (v) q.set(k, v);
      });
      q.set('limit', String(limit));
      try {
        var d = await SD.api(spec.url + '?' + q.toString());
        if (spec.onData) spec.onData(d, inputs);
        status.textContent = '';
        results.textContent = '';
        results.appendChild(ui.table(spec.columns, d.entries.map(function (e) { return spec.row(e, filterBy); }), spec.empty));
        count.textContent = d.total ? 'Showing ' + d.entries.length + ' of ' + d.total + ' matching.' : '';
        if (d.total > d.entries.length && limit < 500) {
          results.appendChild(h('div', { class: 'sd-actions' }, [ui.button('Show more', '', function () { limit += 100; run(); })]));
        }
      } catch (e) { status.textContent = e.message; status.className = 'sd-form-status err'; }
    }
    function quick(label, hours) {
      return ui.button(label, '', function () { inputs.from.value = stamp(new Date(Date.now() - hours * 3600000)); inputs.to.value = ''; limit = 100; run(); });
    }
    var form = h('form', { class: 'sd-fields' }, fields.concat([
      h('div', { class: 'sd-actions' }, [
        h('button', { type: 'submit', class: 'primary', text: 'Search' }),
        ui.button('Clear', '', function () { Object.keys(inputs).forEach(function (k) { inputs[k].value = ''; }); limit = 100; run(); }),
        quick('Last hour', 1), quick('Last 24 hours', 24), quick('Last 7 days', 168),
      ]),
    ]));
    form.addEventListener('submit', function (e) { e.preventDefault(); limit = 100; run(); });
    var card = ui.card(spec.title, spec.desc, [form, status, count, results], 'wide');
    run();
    return card;
  }

  function nice(d) {
    return Object.keys(d || {}).filter(function (k) { return ['role', 'ip', 'userAgent'].indexOf(k) < 0; })
      .map(function (k) { return k + ': ' + (typeof d[k] === 'object' ? JSON.stringify(d[k]) : d[k]); }).join(' · ');
  }

  function activityLog() {
    var picked = false;
    return searchLog({
      title: 'Admin activity', desc: 'Everything staff did, newest first. Search by who did it or who it affected. Searching for a person is noted here too.',
      url: '/api/admin/staff-dashboard/logs/actions',
      fields: [
        { name: 'person', label: 'Person', placeholder: 'Staff or affected account' },
        { name: 'action', label: 'Action', placeholder: 'e.g. ban, coins, tester' },
      ],
      columns: ['Time', 'Who', 'Action', 'Details', ''],
      empty: 'No matching activity.',
      row: function (r, filterBy) {
        return [SD.time(r.ts), ui.button(r.actor, 'sd-link', function () { filterBy('person', r.actor); }), ui.pill(r.action), nice(r.details),
          REVERTIBLE.indexOf(r.action) >= 0 && r.action !== 'revert_action' ? undo(r) : ''];
      },
    });
  }

  function coinLog() {
    return searchLog({
      title: 'Coin log', desc: 'Balance changes of 10 coins or more from the last 30 days. Smaller changes are not recorded. Searching for a person is noted in the admin activity log.',
      url: '/api/admin/staff-dashboard/logs/coins',
      fields: [
        { name: 'person', label: 'Account', placeholder: 'Email' },
        { name: 'reason', label: 'Reason', placeholder: 'e.g. casino, daily' },
        { name: 'min', label: 'At least (coins)', type: 'number', min: 0, step: 'any' },
        { name: 'direction', label: 'Direction', type: 'select', options: [['', 'Gains and losses'], ['gain', 'Gains only'], ['loss', 'Losses only']] },
      ],
      columns: ['Time', 'Account', 'Change', 'Before to after', 'Reason'],
      empty: 'No matching coin changes.',
      row: function (r, filterBy) {
        var delta = Number(r.delta);
        return [SD.time(Date.parse(r.ts)), ui.button(r.email, 'sd-link', function () { filterBy('person', r.email); }),
          (delta > 0 ? '+' : '') + SD.money(delta), SD.money(r.before) + ' to ' + SD.money(r.after), ui.button(r.reason, 'sd-link', function () { filterBy('reason', r.reason); })];
      },
    });
  }

  async function render(root) {
    ui.add(root, [ui.heading('Server')]);
    root.appendChild(resources());

    root.appendChild(ui.grid([activityLog(), coinLog()]));

    var data = await SD.api('/api/admin/staff-dashboard/logs/other');
    root.appendChild(ui.heading('Sent notifications'));
    root.appendChild(ui.table(['Time', 'To', 'Title', 'Status', ''], (data.sentNotifications || []).map(function (r) {
      return [SD.time(r.ts), r.targetEmail, r.title, r.read ? 'read' : 'unread', ui.button('Unsend', '', async function (b) {
        if (!(await SD.confirm('Unsend', r.batchId ? 'Unsend this notification batch?' : 'Unsend this notification?'))) return;
        b.disabled = true;
        try { var d = await SD.post('/api/admin/unsend-notification', { id: r.id, batchId: r.batchId }); await SD.alert('Unsent', d.pending ? SD.pendingText(d) : 'Removed ' + (d.removed || 0) + ' record(s).'); SD.show(); }
        catch (e) { await SD.alert('Failed', e.message); b.disabled = false; }
      })];
    }), 'No notifications sent.'));

    root.appendChild(ui.heading('Banned accounts'));
    root.appendChild(ui.table(['Banned', 'Account', 'By', 'Reason'], (data.bannedAccounts || []).map(function (r) {
      return [SD.time(r.bannedAt), r.email, r.by, r.reason];
    }), 'No banned accounts.'));

    root.appendChild(ui.heading('Anti-cheat'));
    root.appendChild(ui.table(['Time', 'Account', 'Game', 'Details'], (data.cheatLogs || []).map(function (r) {
      return [SD.time(r.ts), r.email, r.game, r.details];
    }), 'Nothing flagged.'));

    if (SD.can('dev')) {
      root.appendChild(logViewer({
        title: 'Application logs', url: '/api/admin/app-logs',
        filters: [
          { name: 'level', label: 'Level', type: 'select', options: [['all', 'All'], ['debug', 'Debug'], ['info', 'Info'], ['warn', 'Warn'], ['error', 'Error']] },
          { name: 'category', label: 'Category', placeholder: 'all', value: 'all' },
          { name: 'limit', label: 'Lines', type: 'select', value: '200', options: [['100', '100'], ['200', '200'], ['500', '500']] },
          { name: 'search', label: 'Search' },
        ],
        text: function (d) { return (d.logs || []).map(function (r) { return '[' + SD.time(r.ts) + '] ' + String(r.level || '').toUpperCase().padEnd(5) + ' ' + String(r.category || 'general').padEnd(14) + ' ' + (r.message || '') + (r.details ? ' ' + r.details : ''); }).join('\n'); },
      }));
      root.appendChild(logViewer({
        title: 'Docker logs', url: '/api/admin/docker-logs',
        filters: [
          { name: 'service', label: 'Service', type: 'select', options: [['all', 'All services'], ['reverse-proxy', 'reverse-proxy'], ['webserver-blue', 'webserver-blue'], ['webserver-green', 'webserver-green']] },
          { name: 'tail', label: 'Lines', type: 'select', value: '200', options: [['100', '100'], ['200', '200'], ['500', '500']] },
        ],
        text: function (d) { return d.output || d.stderr || 'No recent log output.'; },
      }));
    }
  }

  SD.register({ key: 'logs', render: render });
})();
