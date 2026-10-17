(() => {
  const video = document.getElementById('site-tour');
  const status = document.getElementById('tour-status');
  if (video) {
    const chapters = [...document.querySelectorAll('[data-tour-time]')];
    const updateChapter = () => {
      const current = chapters.filter(button => Number(button.dataset.tourTime) <= video.currentTime).at(-1);
      chapters.forEach(button => {
        if (button === current) button.setAttribute('aria-current', 'true');
        else button.removeAttribute('aria-current');
      });
    };
    video.addEventListener('timeupdate', updateChapter);
    video.addEventListener('loadedmetadata', () => {
      const duration = document.getElementById('tour-duration');
      if (duration && Number.isFinite(video.duration)) {
        const seconds = Math.floor(video.duration);
        duration.textContent = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
      }
    });
    updateChapter();
    chapters.forEach(button => {
      button.addEventListener('click', async () => {
        try {
          if (video.readyState < 1) {
            await new Promise((resolve, reject) => {
              const cleanup = () => { video.removeEventListener('loadedmetadata', loaded); video.removeEventListener('error', failed); };
              const loaded = () => { cleanup(); resolve(); };
              const failed = () => { cleanup(); reject(new Error('Video unavailable')); };
              video.addEventListener('loadedmetadata', loaded, { once: true });
              video.addEventListener('error', failed, { once: true });
              video.load();
            });
          }
          video.currentTime = Number(button.dataset.tourTime);
          await video.play();
          status.textContent = '';
        } catch (_) { status.textContent = 'Use the video’s play button, or download the video.'; }
      });
    });
    video.addEventListener('error', () => { status.textContent = 'The video could not load. Try again or download it.'; });
  }
  document.querySelectorAll('.ob-password-toggle').forEach(button => {
    button.addEventListener('click', () => {
      const input = document.getElementById(button.getAttribute('aria-controls'));
      const show = input.type === 'password';
      input.type = show ? 'text' : 'password';
      button.textContent = show ? 'Hide' : 'Show';
      button.setAttribute('aria-pressed', String(show));
      button.setAttribute('aria-label', `${show ? 'Hide' : 'Show'} password`);
    });
  });
  const tabs = [...document.querySelectorAll('.auth-tab-btn')];
  const syncTabs = () => tabs.forEach(tab => { tab.tabIndex = tab.getAttribute('aria-selected') === 'true' ? 0 : -1; });
  tabs.forEach((tab, index) => {
    tab.addEventListener('click', syncTabs);
    tab.addEventListener('keydown', event => {
      const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : -1;
      if (next < 0) return;
      event.preventDefault(); tabs[next].click(); tabs[next].focus();
    });
  });
  if (tabs.length) {
    syncTabs();
    const observer = new MutationObserver(syncTabs);
    tabs.forEach(tab => observer.observe(tab, { attributes: true, attributeFilter: ['aria-selected'] }));
  }
})();
