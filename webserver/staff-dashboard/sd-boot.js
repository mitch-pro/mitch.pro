/* Starts the page once its section module has registered. The overview also
   refreshes itself every minute and when the tab regains focus. */
(function () {
  'use strict';
  SD.start().then(function () {
    if (SD.current !== 'overview' || !SD.can('mod')) return;
    document.addEventListener('visibilitychange', function () { if (!document.hidden) SD.refreshOverview(); });
    setInterval(function () { if (!document.hidden && !document.querySelector('dialog[open]')) SD.refreshOverview(); }, 60000);
  });
})();
