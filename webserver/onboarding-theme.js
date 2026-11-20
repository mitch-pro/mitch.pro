(() => {
  function updateButtons() {
    const light = document.documentElement.classList.contains('theme-light');
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.setAttribute('aria-label', `Switch to ${light ? 'dark' : 'light'} mode`);
      button.setAttribute('aria-pressed', String(light));
    });
  }

  window.addEventListener('themechange', updateButtons);
  document.addEventListener('DOMContentLoaded', () => {
    updateButtons();
    document.querySelectorAll('[data-theme-toggle]').forEach(button => {
      button.addEventListener('click', () => {
        const light = document.documentElement.classList.contains('theme-light');
        window.__siteTheme.setMode(light ? 'dark' : 'light');
      });
    });
  });
})();
