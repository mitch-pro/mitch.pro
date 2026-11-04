(() => {
  const light = /(?:^|;\s*)theme=light(?:;|$)/.test(document.cookie);
  const apply = value => {
    document.documentElement.classList.toggle('theme-light', value);
    // The holiday palettes in theme-tokens.css pick light or dark from
    // data-theme, so pin it to this page's own mode.
    document.documentElement.setAttribute('data-theme', value ? 'light' : 'dark');
    if (window.__holidayFx) window.__holidayFx.refresh();
    document.documentElement.style.colorScheme = value ? 'light' : 'dark';
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.setAttribute('aria-label', `Switch to ${value ? 'dark' : 'light'} mode`);
      button.setAttribute('aria-pressed', String(value));
    });
  };
  apply(light);
  document.addEventListener('DOMContentLoaded', () => {
    apply(light);
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        const next = !document.documentElement.classList.contains('theme-light');
        document.cookie = `theme=${next ? 'light' : 'dark'}; path=/; max-age=31536000; SameSite=Lax`;
        apply(next);
      });
    });
  });
})();
