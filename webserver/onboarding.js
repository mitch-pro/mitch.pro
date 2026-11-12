(() => {
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
