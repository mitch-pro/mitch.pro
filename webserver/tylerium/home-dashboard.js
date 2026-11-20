(() => {
  const clock = document.getElementById('home-clock');
  if (!clock) return;

  const time = document.getElementById('home-clock-time');
  const date = document.getElementById('home-clock-date');
  const formatTime = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });
  const formatDate = new Intl.DateTimeFormat(undefined, { weekday: 'long', month: 'short', day: 'numeric' });
  let timer;

  function updateClock() {
    clearTimeout(timer);
    if (document.hidden) return;
    const now = new Date();
    time.dateTime = now.toISOString();
    time.replaceChildren(...formatTime.formatToParts(now).map(part => {
      if (part.type !== 'dayPeriod') return part.value;
      const period = document.createElement('span');
      period.textContent = part.value;
      return period;
    }));
    date.textContent = formatDate.format(now);
    clock.hidden = false;
    timer = setTimeout(updateClock, 60_000 - now.getTime() % 60_000);
  }

  document.addEventListener('visibilitychange', updateClock);
  window.addEventListener('pageshow', updateClock);
  window.addEventListener('pagehide', () => clearTimeout(timer));
  updateClock();
})();
