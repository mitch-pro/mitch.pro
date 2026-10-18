(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  const fields = ['subject', 'preheader', 'title', 'intro', 'heroUrl', 'buttonLabel', 'buttonUrl', 'signature', 'accent', 'theme'];
  const selected = new Set();
  let accounts = [], example, draft, previewHtml = '', previewTimer, previewVersion = 0;
  let dirty = false, ready = false, reviewing = false, sending = false, review, reviewSnapshot = '', jobTimer;
  const dialog = $('review-dialog');

  function status(id, message, error = false) {
    $(id).textContent = message;
    $(id).classList.toggle('error', error);
  }
  function showAccess(message, allowUnlock = true) {
    ready = false;
    $('access-panel').hidden = false;
    $('editor-workspace').hidden = true;
    $('access-title').textContent = allowUnlock ? 'Unlock email tools' : 'Admin access required';
    $('access-message').textContent = message;
    $('unlock-form').hidden = !allowUnlock;
    $('sign-in-link').hidden = allowUnlock;
    $('save-draft').disabled = $('review-email').disabled = true;
  }
  async function api(path, data, options = {}) {
    const pass = sessionStorage.getItem('admin_passphrase');
    const headers = { 'X-Mitch-Requested-With': '1' };
    if (pass) headers['X-Admin-Passphrase'] = pass;
    if (data !== undefined) headers['Content-Type'] = 'application/json';
    const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store',
      method: data === undefined ? 'GET' : 'POST', headers, body: data === undefined ? undefined : JSON.stringify(data), ...options });
    const body = await response.json().catch(() => ({}));
    if (response.status === 401) showAccess('Sign in with an administrator account to continue.', false);
    if (response.status === 403 && ['invalid_passphrase', 'passphrase_not_configured'].includes(body.error)) {
      showAccess('Enter your admin passphrase to continue.');
    }
    if (!response.ok) throw new Error(body.error || 'The request could not be completed.');
    return body;
  }
  function audience() { return document.querySelector('input[name="audience"]:checked').value; }
  function otherRecipients() { return $('extra-recipients').value.split(/[,;\n]+/).map(s => s.trim().toLowerCase()).filter(Boolean); }
  function readDraft() {
    const value = { ...draft };
    for (const field of fields) value[field] = $(field).value;
    value.audience = audience();
    value.recipients = [...new Set([...selected, ...otherRecipients()])];
    value.sections = [...document.querySelectorAll('.content-section')].map(section => ({
      title: section.querySelector('[data-section-title]').value,
      body: section.querySelector('[data-section-body]').value,
    }));
    return value;
  }
  function addSection(section = { title: '', body: '' }) {
    const row = document.createElement('div');
    row.className = 'content-section';
    row.innerHTML = '<div class="section-controls"><span></span><button type="button" data-move="up">Move up</button><button type="button" data-move="down">Move down</button><button type="button" data-remove>Remove</button></div><label>Section heading<input data-section-title maxlength="180"></label><label>Text<textarea data-section-body rows="5" maxlength="12000"></textarea></label>';
    row.querySelector('[data-section-title]').value = section.title;
    row.querySelector('[data-section-body]').value = section.body;
    row.querySelector('[data-remove]').onclick = () => { row.remove(); updateSections(); changed(); };
    row.querySelector('[data-move="up"]').onclick = () => { if (row.previousElementSibling) row.previousElementSibling.before(row); updateSections(); changed(); };
    row.querySelector('[data-move="down"]').onclick = () => { if (row.nextElementSibling) row.nextElementSibling.after(row); updateSections(); changed(); };
    $('newsletter-sections').append(row);
    updateSections();
  }
  function updateSections() {
    const rows = [...document.querySelectorAll('.content-section')];
    rows.forEach((row, i) => {
      row.querySelector('.section-controls span').textContent = 'Section ' + (i + 1);
      row.querySelector('[data-move="up"]').disabled = i === 0;
      row.querySelector('[data-move="down"]').disabled = i === rows.length - 1;
    });
    $('add-section').disabled = rows.length >= 8;
  }
  function populate(value) {
    draft = structuredClone(value);
    for (const field of fields) $(field).value = value[field];
    selected.clear();
    const known = new Set(accounts.map(a => a.email));
    value.recipients.forEach(email => { if (known.has(email)) selected.add(email); });
    $('extra-recipients').value = value.recipients.filter(email => !known.has(email)).join(', ');
    document.querySelector(`input[name="audience"][value="${value.audience}"]`).checked = true;
    $('newsletter-sections').replaceChildren();
    value.sections.forEach(addSection);
    updateAudience(); renderRecipients();
  }
  function renderRecipients() {
    const query = $('recipient-search').value.trim().toLowerCase();
    const list = $('recipient-list');
    list.replaceChildren();
    const matches = accounts.filter(account => [account.email, account.displayName, account.username].some(v => String(v || '').toLowerCase().includes(query)));
    for (const account of matches.slice(0, 100)) {
      const label = document.createElement('label'); label.className = 'recipient-option';
      const checkbox = document.createElement('input'); checkbox.type = 'checkbox'; checkbox.value = account.email;
      checkbox.checked = selected.has(account.email); checkbox.disabled = !account.eligible;
      checkbox.onchange = () => { checkbox.checked ? selected.add(account.email) : selected.delete(account.email); changed(); };
      const name = document.createElement('span'); name.textContent = account.displayName || account.email;
      const email = document.createElement('small'); email.textContent = account.email + (account.eligible ? '' : ' · Unsubscribed'); name.append(email);
      label.append(checkbox, name); list.append(label);
    }
    if (!matches.length || matches.length > 100) {
      const note = document.createElement('p'); note.className = 'recipient-empty';
      note.textContent = matches.length ? 'Search to find more members.' : 'No matching members.'; list.append(note);
    }
  }
  function updateAudience() {
    $('selected-recipients').hidden = audience() === 'everyone';
    const eligible = accounts.filter(a => a.eligible).length;
    $('everyone-count').textContent = `(${eligible})`;
    const unsub = new Set(accounts.filter(a => !a.eligible).map(a => a.email));
    const count = audience() === 'everyone' ? eligible : [...new Set([...selected, ...otherRecipients()])].filter(email => !unsub.has(email)).length;
    $('recipient-count').textContent = `${count} ${count === 1 ? 'recipient' : 'recipients'}`;
  }
  function changed() {
    if (!ready) return;
    dirty = true; review = null; $('example-note').hidden = true;
    status('draft-status', 'Unsaved changes');
    updateAudience(); clearTimeout(previewTimer); previewTimer = setTimeout(updatePreview, 350);
  }
  async function updatePreview() {
    if (!ready) return;
    const version = ++previewVersion;
    status('preview-status', 'Updating preview…');
    try {
      const result = await api('/api/admin/newsletters/preview', readDraft());
      if (version !== previewVersion) return;
      previewHtml = result.html;
      $('newsletter-preview').srcdoc = result.html;
      $('preview-subject').textContent = result.subject || '(No subject)';
      $('preview-preheader').textContent = result.preheader;
      status('preview-status', 'Preview only. Nothing has been sent.');
    } catch (error) { if (version === previewVersion) status('preview-status', error.message, true); }
  }
  async function initialize() {
    try {
      const me = await api('/api/me');
      if (!me.isAdmin) { showAccess('This page is available to administrators.', false); return; }
      const [saved, directory] = await Promise.all([api('/api/admin/newsletters/draft'), api('/api/admin/newsletters/recipients')]);
      accounts = directory.accounts; example = saved.example;
      populate(saved.draft);
      $('access-panel').hidden = true; $('editor-workspace').hidden = false;
      $('save-draft').disabled = $('review-email').disabled = false;
      $('example-note').hidden = !!saved.savedAt;
      ready = true; dirty = false;
      status('draft-status', saved.savedAt ? 'Saved ' + new Date(saved.savedAt).toLocaleString() : 'Example draft');
      await updatePreview();
      const lastJob = sessionStorage.getItem('newsletter_last_job');
      if (lastJob) pollJob(lastJob);
    } catch (error) {
      if (!$('access-panel').hidden) $('access-message').textContent = error.message;
    }
  }
  $('unlock-form').onsubmit = async event => {
    event.preventDefault();
    const passphrase = $('admin-passphrase').value;
    try {
      const response = await fetch('/api/admin/passphrase-status', { method: 'POST', credentials: 'same-origin', headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' }, body: JSON.stringify({ passphrase }) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error || 'Passphrase could not be verified.');
      sessionStorage.setItem('admin_passphrase', passphrase); $('admin-passphrase').value = '';
      await initialize();
    } catch (error) { $('access-message').textContent = error.message; }
  };
  $('newsletter-form').onsubmit = event => event.preventDefault();
  $('newsletter-form').addEventListener('input', event => { if (event.target.id !== 'recipient-search') changed(); });
  document.querySelectorAll('[name="audience"]').forEach(radio => radio.addEventListener('change', changed));
  $('recipient-search').oninput = renderRecipients;
  $('add-section').onclick = () => { addSection(); changed(); };
  $('load-example').onclick = () => {
    const current = readDraft(); populate({ ...structuredClone(example), audience: current.audience, recipients: current.recipients });
    changed(); $('example-note').hidden = false;
  };
  $('save-draft').onclick = async () => {
    const snapshot = JSON.stringify(readDraft());
    $('save-draft').disabled = true;
    try {
      const result = await api('/api/admin/newsletters/draft', JSON.parse(snapshot));
      dirty = JSON.stringify(readDraft()) !== snapshot;
      status('draft-status', dirty ? 'Unsaved changes' : 'Saved ' + new Date(result.savedAt).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }));
    } catch (error) { status('editor-status', error.message, true); }
    finally { $('save-draft').disabled = !ready; }
  };
  $('review-email').onclick = async () => {
    if (!ready || reviewing || sending || !$('newsletter-form').reportValidity()) return;
    reviewing = true; $('review-email').disabled = true;
    reviewSnapshot = JSON.stringify(readDraft());
    status('editor-status', 'Preparing recipient list…');
    try {
      const result = await api('/api/admin/newsletters/review', JSON.parse(reviewSnapshot));
      if (JSON.stringify(readDraft()) !== reviewSnapshot) throw new Error('The draft changed. Review the updated email again.');
      review = result; $('review-subject').textContent = result.subject;
      $('review-count').textContent = `${result.count} ${result.count === 1 ? 'recipient' : 'recipients'}`;
      $('send-newsletter').textContent = `Send to ${result.count} ${result.count === 1 ? 'recipient' : 'recipients'}`;
      $('review-recipients').replaceChildren(...result.recipients.map(email => { const li = document.createElement('li'); li.textContent = email; return li; }));
      $('review-preview').srcdoc = result.html;
      $('send-newsletter').disabled = false; status('review-status', 'Nothing will be sent until you choose Send.');
      status('editor-status', ''); dialog.showModal();
    } catch (error) { status('editor-status', error.message, true); }
    finally { reviewing = false; $('review-email').disabled = !ready || sending; }
  };
  for (const id of ['close-review', 'back-to-editor']) $(id).onclick = () => { if (!sending) dialog.close(); };
  dialog.addEventListener('cancel', event => { if (sending) event.preventDefault(); });
  $('send-newsletter').onclick = async () => {
    if (!review || sending || JSON.stringify(readDraft()) !== reviewSnapshot) return;
    sending = true; $('send-newsletter').disabled = true; $('review-email').disabled = true;
    status('review-status', 'Submitting newsletter…');
    sessionStorage.setItem('newsletter_last_job', review.id);
    try {
      const result = await api('/api/admin/newsletters/send', { id: review.id, confirm: true });
      sessionStorage.setItem('newsletter_last_job', result.job.id);
      dialog.close(); status('editor-status', 'Newsletter submitted. Delivery status is below.');
      dirty = false; pollJob(result.job.id);
    } catch (error) {
      status('review-status', error.message + ' Check delivery status before trying again.', true);
      pollJob(review.id);
    }
    finally { sending = false; $('review-email').disabled = !ready; $('send-newsletter').disabled = !ready; }
  };
  async function pollJob(id) {
    clearTimeout(jobTimer);
    try {
      const { job } = await api('/api/admin/newsletters/job?id=' + encodeURIComponent(id));
      $('delivery-panel').hidden = false;
      const label = { sending: 'Sending', complete: 'Finished', interrupted: 'Interrupted — check accepted emails before sending again', review: 'Not sent' }[job.status] || job.status;
      status('delivery-status', `${label}. ${job.accepted} accepted by SMTP, ${job.failed} failed, ${job.uncertain} unconfirmed, ${job.skipped} skipped. ${job.total} total.`);
      $('refresh-delivery').onclick = () => pollJob(id);
      if (job.status === 'sending') jobTimer = setTimeout(() => pollJob(id), 3000);
    } catch (error) { status('editor-status', 'Delivery status: ' + error.message, true); }
  }
  document.querySelectorAll('[data-preview-width]').forEach(button => {
    button.onclick = () => {
      $('preview-canvas').classList.toggle('phone', button.dataset.previewWidth === 'phone');
      document.querySelectorAll('[data-preview-width]').forEach(item => item.setAttribute('aria-pressed', String(item === button)));
    };
  });
  $('download-html').onclick = () => {
    if (!previewHtml) return;
    const url = URL.createObjectURL(new Blob([previewHtml], { type: 'text/html' }));
    const link = document.createElement('a'); link.href = url; link.download = 'mitch-newsletter.html'; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  window.addEventListener('beforeunload', event => { if (dirty) { event.preventDefault(); event.returnValue = ''; } });
  initialize();
})();
