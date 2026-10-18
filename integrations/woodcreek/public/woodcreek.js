(() => {
  const empty = document.getElementById('session-empty');
  const fleet = document.getElementById('fleet-section');
  if (empty && fleet) {
    const sync = () => { empty.hidden = !fleet.hidden; };
    new MutationObserver(sync).observe(fleet, { attributes: true, attributeFilter: ['hidden'] });
    sync();
  }
  const navigation = document.querySelectorAll('.site-nav a[href^="#"], .sidebar-bottom a[href^="#"]');
  navigation.forEach(link => link.addEventListener('click', () => {
    const target = document.getElementById(link.hash.slice(1));
    if (target?.tagName === 'DETAILS') target.open = true;
    document.querySelectorAll('.site-nav .is-current').forEach(item => item.classList.remove('is-current'));
    link.classList.add('is-current');
  }));
  // Keep keyboard focus inside an open dialog and restore it when dismissed.
  let previousFocus;
  const dialogs = [...document.querySelectorAll('.modal-backdrop[role="dialog"]')];
  dialogs.forEach(dialog => new MutationObserver(() => {
    if (!dialog.hidden) {
      previousFocus = document.activeElement;
      requestAnimationFrame(() => dialog.querySelector('button, input, [href], select, textarea')?.focus());
    } else if (!dialogs.some(item => !item.hidden) && previousFocus?.isConnected) previousFocus.focus();
  }).observe(dialog, {attributes:true,attributeFilter:['hidden']}));
  document.addEventListener('keydown', event => {
    const dialog = dialogs.find(item => !item.hidden);
    if (!dialog || event.key !== 'Tab') return;
    const items = [...dialog.querySelectorAll('button, [href], input, select, textarea, [tabindex="0"]')].filter(item => !item.disabled && item.getClientRects().length);
    if (!items.length) return;
    const first = items[0], last = items.at(-1);
    if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
  });
})();
