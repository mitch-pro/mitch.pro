(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  let entries = [], editingId = null, setup = false;

  function headers() {
    const result = { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' };
    const pass = sessionStorage.getItem('admin_passphrase');
    if (pass) result['X-Admin-Passphrase'] = pass;
    return result;
  }

  async function request(url, options = {}) {
    const response = await fetch(url, { credentials: 'same-origin', cache: 'no-store', ...options, headers: headers() });
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
      const error = new Error(data.error || 'Request failed. Try again.');
      error.code = data.error;
      throw error;
    }
    return data;
  }

  function say(id, text, isError = false) {
    $(id).textContent = text;
    $(id).dataset.error = String(isError);
  }

  const needsPassphrase = error => ['invalid_passphrase', 'passphrase_not_configured'].includes(error.code);

  function showLock() {
    $('updates-workspace').hidden = true;
    $('passphrase-lock').hidden = false;
    $('lock-heading').textContent = setup ? 'Set your admin passphrase' : 'Unlock updates';
    $('unlock-button').textContent = setup ? 'Save and unlock' : 'Unlock';
    say('access-status', 'Admin access verified. Enter your passphrase to continue.');
  }

  function render() {
    const list = $('entry-list');
    list.replaceChildren();
    if (!entries.length) {
      const empty = document.createElement('p');
      empty.className = 'entry-empty';
      empty.textContent = 'No entries yet.';
      list.append(empty);
    }
    for (const entry of entries) {
      const row = document.createElement('article');
      row.className = 'entry';
      row.setAttribute('role', 'listitem');
      const copy = document.createElement('div');
      copy.className = 'entry-copy';
      const time = document.createElement('time');
      time.textContent = new Date(entry.postedAt).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });
      const title = document.createElement('strong');
      title.textContent = entry.title;
      copy.append(time, title);
      if (entry.body) {
        const body = document.createElement('p');
        body.textContent = entry.body;
        copy.append(body);
      }
      const buttons = document.createElement('div');
      buttons.className = 'entry-buttons';
      const edit = document.createElement('button');
      edit.type = 'button';
      edit.textContent = 'Edit';
      edit.addEventListener('click', () => startEdit(entry));
      const remove = document.createElement('button');
      remove.type = 'button';
      remove.textContent = 'Delete';
      remove.addEventListener('click', () => removeEntry(entry));
      buttons.append(edit, remove);
      row.append(copy, buttons);
      list.append(row);
    }
  }

  async function load() {
    const data = await request('/api/admin/updates');
    entries = data.updates || [];
    $('passphrase-lock').hidden = true;
    $('updates-workspace').hidden = false;
    say('access-status', '');
    render();
  }

  function isoDay(ms) { return new Date(ms).toISOString().slice(0, 10); }

  function startEdit(entry) {
    editingId = entry.id;
    $('form-heading').textContent = 'Edit entry';
    $('entry-title').value = entry.title;
    $('entry-body').value = entry.body;
    $('entry-date').value = isoDay(entry.postedAt);
    $('entry-save').textContent = 'Save changes';
    $('entry-cancel').hidden = false;
    $('entry-title').focus();
  }

  function resetForm() {
    editingId = null;
    $('entry-form').reset();
    $('form-heading').textContent = 'New entry';
    $('entry-save').textContent = 'Post';
    $('entry-cancel').hidden = true;
  }

  async function removeEntry(entry) {
    if (!confirm('Delete "' + entry.title + '"?')) return;
    say('list-status', 'Deleting…');
    try {
      await request('/api/admin/updates/delete', { method: 'POST', body: JSON.stringify({ id: entry.id }) });
      if (editingId === entry.id) resetForm();
      await load();
      say('list-status', '');
    } catch (error) {
      say('list-status', error.message, true);
      if (needsPassphrase(error)) showLock();
    }
  }

  $('entry-cancel').addEventListener('click', () => { resetForm(); say('entry-status', ''); });

  $('entry-form').addEventListener('submit', async event => {
    event.preventDefault();
    $('entry-save').disabled = true;
    say('entry-status', 'Saving…');
    const payload = { title: $('entry-title').value, body: $('entry-body').value, date: $('entry-date').value };
    if (editingId !== null) payload.id = editingId;
    try {
      await request('/api/admin/updates', { method: 'POST', body: JSON.stringify(payload) });
      const wasEdit = editingId !== null;
      resetForm();
      await load();
      say('entry-status', wasEdit ? 'Saved.' : 'Posted.');
    } catch (error) {
      say('entry-status', error.message, true);
      if (needsPassphrase(error)) showLock();
    } finally {
      $('entry-save').disabled = false;
    }
  });

  $('passphrase-form').addEventListener('submit', async event => {
    event.preventDefault();
    $('unlock-button').disabled = true;
    say('unlock-status', 'Checking…');
    const pass = $('admin-passphrase').value;
    try {
      const response = await fetch('/api/admin/passphrase-status', {
        method: 'POST', credentials: 'same-origin',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', 'X-Admin-Passphrase': pass },
        body: JSON.stringify({ passphrase: pass })
      });
      const data = await response.json().catch(() => ({}));
      if (!response.ok) throw new Error(data.error === 'invalid_passphrase' ? 'Incorrect passphrase.' : data.error || 'Could not unlock.');
      sessionStorage.setItem('admin_passphrase', pass);
      $('admin-passphrase').value = '';
      await load();
      say('unlock-status', '');
    } catch (error) {
      say('unlock-status', error.message, true);
    } finally {
      $('unlock-button').disabled = false;
    }
  });

  (async () => {
    try {
      const me = await request('/api/me');
      if (!me.isAdmin && !me.isOwner && !me.isCoOwner) { say('access-status', 'Admin access is required to edit updates.', true); return; }
      setup = !(await request('/api/admin/passphrase-status')).set;
      if (!setup && sessionStorage.getItem('admin_passphrase')) {
        try { await load(); return; } catch (error) { if (!needsPassphrase(error)) throw error; }
      }
      showLock();
    } catch (error) {
      say('access-status', error.message, true);
    }
  })();
})();
