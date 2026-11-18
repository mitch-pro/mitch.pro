/* Moderation: look up an account, ban or restore it, outside-account chat access,
   featured game and the blog deletion log. Moderators and above. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;

  function profileCard(root, d) {
    var rooms = d.rooms || [];
    var pills = [
      d.accountBanned ? ui.pill('Account banned' + (d.banReason ? ': ' + d.banReason : ''), 'warn') : ui.pill('Not banned', 'ok'),
      d.vmBanned ? ui.pill('Banned from VMs', 'warn') : null,
      d.byoOsBanned ? ui.pill('Banned from BYO-OS', 'warn') : null,
    ];
    var canRestore = d.accountBanned || rooms.some(function (r) { return r.bannedInSettings || r.mutedInSettings || r.membership === 'ban'; });
    function run(fn) { return async function (b) { b.disabled = true; try { await fn(); await lookup(root, d.email); } catch (e) { await SD.alert('Failed', e.message); b.disabled = false; } }; }
    var actions = [];
    if (canRestore) actions.push(ui.button('Restore account (unban and unmute everywhere)', '', run(async function () {
      if (!(await SD.confirm('Restore account', 'Unban ' + d.email + ' everywhere: the account ban, IP bans and all Matrix room bans, mutes and negative power levels will be cleared.'))) throw new Error('Cancelled.');
      await SD.post('/api/admin/unban-account', { email: d.email });
    })));
    function restriction(label, url, on) {
      return ui.button((on ? 'Unban from ' : 'Ban from ') + label, on ? '' : 'danger', run(async function () {
        if (!(await SD.confirm((on ? 'Restore ' : 'Restrict ') + label + ' access', d.email, { danger: !on }))) throw new Error('Cancelled.');
        await SD.post(url, { email: d.email, banned: !on });
      }));
    }
    actions.push(restriction('VMs', '/api/admin/vm-ban', d.vmBanned), restriction('BYO-OS', '/api/admin/byo-os-ban', d.byoOsBanned));
    var table = ui.table(['Room', 'Membership', 'Power level', 'Status'], rooms.map(function (r) {
      return [r.alias ? '#' + r.alias + ':mitch.pro' : r.roomId, r.membership, r.powerLevel, r.bannedInSettings ? 'Banned' : r.mutedInSettings ? 'Muted' : '-'];
    }), 'No room relationship found (or Matrix is unreachable).');
    return ui.card(d.email, (d.matrixUserIds || []).join(', ') || 'No Matrix account found.', [h('div', { class: 'sd-actions' }, pills), table, h('div', { class: 'sd-actions' }, actions)], 'wide');
  }

  async function lookup(root, q) {
    var out = root.querySelector('.sd-profile-out');
    out.textContent = '';
    out.appendChild(ui.empty('Looking up...'));
    try {
      var d = await SD.api('/api/admin/moderation/lookup?q=' + encodeURIComponent(q));
      out.textContent = '';
      out.appendChild(profileCard(root, d));
    } catch (e) { out.textContent = ''; out.appendChild(ui.empty(e.message)); }
  }

  function render(root) {
    var search = ui.form({
      access: 'mod', title: 'Account lookup', desc: 'Bans, room status and restrictions in one place.', submitLabel: 'Look up', keep: true,
      fields: [{ name: 'q', label: 'Email, username or Matrix ID', required: true }],
      submit: async function (v) { await lookup(root, v.q); return ''; },
    });
    root.appendChild(ui.grid([search]));
    root.appendChild(h('div', { class: 'sd-profile-out' }));

    root.appendChild(ui.heading('Account actions'));
    root.appendChild(ui.grid([
      ui.form({
        access: 'mod', title: 'Ban an account', desc: 'Blocks sign-in and removes the account’s chat messages. Restore it from the lookup above or on the bans page.',
        submitLabel: 'Ban account', danger: true,
        fields: [
          { name: 'email', label: 'Email or username', required: true },
          { name: 'reason', label: 'Reason', required: true },
          { name: 'duration', label: 'Duration', type: 'select', options: [['1h', '1 hour'], ['1d', '1 day'], ['3d', '3 days'], ['7d', '7 days'], ['14d', '14 days'], ['permanent', 'Permanent']], value: '1d' },
        ],
        confirm: function (v) { return 'Ban ' + v.email + ' (' + v.duration + ')?'; },
        submit: function (v) { return SD.post('/api/admin/ban-account', v); },
        ok: function (d) { return 'Banned ' + (d.targetEmail || '') + '.'; },
      }),
      ui.card('Ban records', 'Searchable ban list, durations, appeals and cleanup status.', [h('a', { class: 'sd-link', href: '/staff-dashboard/bans/', text: 'Open the bans page' })]),
    ].filter(function (c) { return c && (SD.can('admin') || c.tagName !== 'SECTION'); })));

    root.appendChild(ui.heading('Chat access', 'Accounts outside the student directory need approval before they can chat.'));
    root.appendChild(ui.grid([ui.list({
      access: 'mod', title: 'Waiting for approval', empty: 'Nobody is waiting.',
      load: async function () { return (await SD.api('/api/admin/matrix/outside-accounts')).pending || []; },
      row: function (it) { return { label: it.email, sub: 'Requested ' + SD.time(it.requestedAt) }; },
      actions: function (it) {
        return [
          { label: 'Approve', run: function () { return SD.post('/api/admin/matrix/outside-accounts', { email: it.email, approved: true }); } },
          { label: 'Deny', danger: true, confirm: 'Deny chat access for ' + it.email + '?', run: function () { return SD.post('/api/admin/matrix/outside-accounts', { email: it.email, approved: false }); } },
        ];
      },
    })]));

    var content = ui.grid([
      ui.form({
        access: 'mod', request: true, title: 'Featured game', desc: 'The game highlighted on the homepage (its page path).', submitLabel: 'Set featured game',
        fields: [{ name: 'href', label: 'Game link', placeholder: '/games/...', required: true }],
        submit: function (v) { return SD.post('/api/admin/content/featured', { href: v.href }); }, ok: function () { return 'Featured game updated.'; },
      }),
    ]);
    root.appendChild(ui.heading('Content'));
    root.appendChild(content);
    root.appendChild(ui.grid([ui.list({
      access: 'mod', title: 'Deleted blog posts', empty: 'No blog posts have been deleted.',
      load: async function () { return (await SD.api('/api/admin/blog-deletions')).logs || []; },
      row: function (r) { return { label: r.title || r.slug || r.postId || 'Deleted post', tag: r.status || 'post', sub: 'Deleted by ' + (r.deletedBy || 'unknown') + ' (' + (r.deletedByRole || 'user') + ') · ' + SD.time(r.ts) + ' · author ' + (r.authorName || r.authorEmail || 'unknown') }; },
    })]));
  }

  SD.register({ key: 'moderation', render: render });
})();
