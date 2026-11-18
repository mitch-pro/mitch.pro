/* Matrix chat controls for the official room: health, slowmode, lockdown, staff
   roles, channels, per-user actions and the audit log. Moderators and above. */
(function () {
  'use strict';
  var SD = window.SD, h = SD.h, ui = SD.ui;
  var M = '/api/matrix/moderation/';

  function userAction(title, desc, url, extra, label, danger) {
    return ui.form({
      access: 'mod', title: title, desc: desc, submitLabel: label, danger: danger,
      fields: [{ name: 'userId', label: 'Matrix ID or email', placeholder: '@name:mitch.pro', required: true }].concat(extra || []),
      confirm: danger ? function (v) { return label + ' ' + v.userId + '?'; } : undefined,
      submit: function (v) { return SD.api(url, v); },
      ok: function () { return 'Done.'; },
    });
  }

  function health() {
    var box = h('div', { class: 'sd-stats' });
    SD.api('/api/admin/matrix/health').then(function (d) {
      ui.add(box, [
        ui.stat('Status', d.reachable ? 'Online' : 'Unreachable', d.reachable ? 'good' : 'bad'),
        ui.stat('Latency', d.latencyMs + ' ms'),
        ui.stat('Database', (d.dbSizeBytes / 1073741824).toFixed(2) + ' GB'),
        ui.stat('Rooms', d.trackedRooms + ' + ' + d.officialRoomCount + ' official'),
        ui.stat('Federation', d.federationAllowed ? 'Allowed' : 'Disabled'),
      ]);
    }).catch(function (e) { box.appendChild(ui.empty(e.message)); });
    return box;
  }

  async function render(root) {
    ui.add(root, [ui.heading('Matrix chat', 'Controls for the official chat rooms. Reports are on the Overview tab.')]);
    root.appendChild(health());
    var ov = {};
    try { ov = await SD.api(M + 'overview'); } catch (e) { SD.say(e.message, true); }

    root.appendChild(ui.heading('Room controls'));
    root.appendChild(ui.grid([
      ui.form({
        access: 'mod', title: 'Slowmode', desc: 'Currently ' + (ov.slowmodeSeconds ? ov.slowmodeSeconds + ' seconds' : 'off') + '.', submitLabel: 'Apply', keep: true,
        fields: [{ name: 'seconds', label: 'Seconds between messages', type: 'select', value: String(ov.slowmodeSeconds || 0),
          options: [['0', 'Off'], ['5', '5 seconds'], ['10', '10 seconds'], ['30', '30 seconds'], ['60', '1 minute'], ['300', '5 minutes']] }],
        submit: function (v) { return SD.api(M + 'slowmode', { seconds: Number(v.seconds) }); }, ok: function () { return 'Slowmode updated.'; },
      }),
      ui.form({
        access: 'mod', title: ov.roomMuted ? 'Room is locked down' : 'Lock down the room', desc: 'While locked, only staff can send messages.', danger: !ov.roomMuted,
        submitLabel: ov.roomMuted ? 'Unlock room' : 'Lock down room', fields: [],
        confirm: ov.roomMuted ? 'Let regular members chat again?' : 'Stop regular members sending messages until unlocked?',
        submit: function () { return SD.api(M + 'mute-room', { muted: !ov.roomMuted }); },
        ok: function () { return 'Updated.'; }, onDone: function () { SD.show('matrix'); },
      }),
      ui.form({
        access: 'mod', title: 'Clean up old sessions', desc: 'Removes abandoned chat devices older than 7 days. Fixes key-sharing and "unable to decrypt" errors.', submitLabel: 'Prune old sessions',
        fields: [], submit: function () { return SD.api('/api/matrix/devices/prune-stale', { maxAgeDays: 7 }); },
        ok: function (d) { return 'Pruned ' + (d.prunedCount || 0) + ' session(s); ' + (d.remainingCount || 0) + ' remain.'; },
      }),
    ]));

    root.appendChild(ui.heading('People'));
    root.appendChild(ui.grid([
      userAction('Mute', 'Stops a user sending messages.', M + 'mute-user', [{ name: 'durationSeconds', label: 'For', type: 'select', value: '3600', options: [['900', '15 minutes'], ['3600', '1 hour'], ['86400', '24 hours'], ['0', 'Until unmuted']] }, { name: 'reason', label: 'Reason' }], 'Mute', true),
      userAction('Unmute', '', M + 'unmute-user', [], 'Unmute', false),
      userAction('Kick', 'Removes the user from the room; they can rejoin.', M + 'kick', [{ name: 'reason', label: 'Reason' }], 'Kick', true),
      userAction('Ban from chat', 'Bans the user from the room.', M + 'ban', [{ name: 'reason', label: 'Reason' }], 'Ban', true),
      userAction('Unban from chat', '', M + 'unban', [], 'Unban', false),
      userAction('Per-user slowmode', 'Slows one user. 0 turns it off.', M + 'user-slowmode', [{ name: 'seconds', label: 'Seconds', type: 'number', required: true, min: 0 }], 'Apply', false),
      ui.form({
        access: 'mod', title: 'Delete a message', desc: 'Needs the room ID and the message’s event ID.', submitLabel: 'Delete message', danger: true,
        fields: [{ name: 'roomId', label: 'Room ID', placeholder: '!abc:mitch.pro', required: true }, { name: 'eventId', label: 'Event ID', placeholder: '$abc', required: true }, { name: 'reason', label: 'Reason' }],
        confirm: 'Delete this message? This cannot be undone.',
        submit: function (v) { return SD.api(M + 'redact', { roomId: v.roomId, eventId: v.eventId, reason: v.reason || 'Inappropriate content' }); }, ok: function () { return 'Message deleted.'; },
      }),
    ]));

    root.appendChild(ui.heading('Staff in the official room'));
    root.appendChild(ui.grid([ui.list({
      access: 'admin', title: 'Roles', empty: 'No staff assigned.',
      load: async function () { return (await SD.api(M + 'overview')).staff || []; },
      row: function (s) { return { label: s.userId, tag: s.role + ' (PL ' + s.powerLevel + ')' }; },
      actions: function (s) {
        var set = function (pl) { return function () { return SD.api(M + 'set-role', { userId: s.userId, powerLevel: pl }); }; };
        return [s.powerLevel >= 100 ? { label: 'Make moderator', run: set(50) } : { label: 'Make admin', run: set(100) }, { label: 'Remove staff', danger: true, confirm: 'Remove staff powers from ' + s.userId + '?', run: set(0) }];
      },
    }), ui.list({
      access: 'admin', title: 'Public channels', empty: 'No channels.',
      load: async function () { return (await SD.api(M + 'channels')).channels || []; },
      row: function (c) { return { label: c.name || c.alias, sub: '#' + c.alias + ':mitch.pro' + (c.topic ? ' · ' + c.topic : '') }; },
      actions: function (c) { return [{ label: 'Remove', danger: true, confirm: 'Remove #' + c.alias + ':mitch.pro from the official channels list?', run: function () { return SD.api(M + 'channels', { alias: c.alias }, 'DELETE'); } }]; },
      add: { submitLabel: 'Add channel', fields: [{ name: 'alias', label: 'Alias', required: true }, { name: 'name', label: 'Name', required: true }, { name: 'topic', label: 'Topic' }], submit: function (v) { return SD.api(M + 'channels', v); } },
    })]));

    root.appendChild(ui.heading('Moderation log'));
    try {
      var log = (await SD.api(M + 'audit-log')).auditLog || [];
      root.appendChild(ui.table(['Time', 'Where', 'Who', 'Action', 'Target', 'Reason'], log.slice(0, 100).map(function (e) {
        var d = e.details || {};
        var client = d.source === 'matrix_client' || String(e.action || '').indexOf('matrix_chat_') === 0;
        return [SD.time(e.ts), client ? 'In chat' : 'Dashboard', e.actor || 'staff', String(e.action || '').replace(/^matrix_(chat_)?/, '').replace(/_/g, ' '),
          d.targetUserId || d.userId || d.eventId || (d.alias ? '#' + d.alias + ':mitch.pro' : ''), d.reason || (d.powerLevel != null ? 'PL ' + d.powerLevel : d.seconds != null ? d.seconds + 's' : '')];
      }), 'No chat moderation events yet.'));
    } catch (e) { root.appendChild(ui.empty(e.message)); }
  }

  SD.register({ key: 'matrix', label: 'Matrix chat', access: 'mod', render: render });
})();
