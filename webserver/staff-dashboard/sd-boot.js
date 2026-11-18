/* Starts the dashboard once every section module has registered. */
(function () {
  'use strict';
  async function start() {
    try {
      if (!(await SD.loadRole())) { SD.say('Staff access required.', true); return; }
    } catch (err) { SD.say(err.message || 'Could not load staff status.', true); return; }
    var badge = SD.$('sd-role');
    var r = SD.role;
    badge.textContent = r.isOwner ? 'Root' : r.isCoOwner ? 'Developer' : r.isAdmin ? 'Admin' : r.isModerator ? 'Moderator' : 'Tester';
    SD.$('sd-refresh').addEventListener('click', function () { SD.show(SD.current); });
    document.addEventListener('visibilitychange', function () {
      if (!document.hidden && SD.current === 'overview') SD.refreshOverview();
    });
    setInterval(function () {
      if (!document.hidden && SD.current === 'overview' && !document.querySelector('dialog[open]')) SD.refreshOverview();
    }, 60000);
    SD.show(location.hash.slice(1) || 'overview');
  }
  start();
})();
