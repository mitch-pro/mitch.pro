// Adds a "Forgot your passphrase?" link under every admin passphrase prompt.
// The prompts are separate per tool; this keeps the link in one place.
(() => {
  function addLinks() {
    document.querySelectorAll('input[type=password][autocomplete="current-password"]:not([data-forgot-link])').forEach(input => {
      input.dataset.forgotLink = '1';
      const form = input.closest('form') || input.parentElement;
      if (!form) return;
      const link = document.createElement('a');
      link.href = '/admin/passphrase-reset/';
      link.textContent = 'Forgot your passphrase?';
      link.style.cssText = 'display:inline-block;margin-top:10px;font-size:13px;color:var(--t-fg2)';
      form.appendChild(link);
    });
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', addLinks);
  else addLinks();
})();
