(() => {
  'use strict';
  const $ = id => document.getElementById(id);
  let state = { today: '', schools: [], presets: [], overrides: [] };
  let setup = false;

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
  const schoolLabel = key => (state.schools.find(s => s.key === key) || {}).label || key;

  function showLock() {
    $('ss-workspace').hidden = true;
    $('passphrase-lock').hidden = false;
    $('lock-heading').textContent = setup ? 'Set your admin passphrase' : 'Unlock special schedule';
    $('unlock-button').textContent = setup ? 'Save and unlock' : 'Unlock';
    say('access-status', 'Admin access verified. Enter your passphrase to continue.');
  }

  // ---- dates (all in the server's Los Angeles calendar day) ----
  function addDays(iso, n) {
    const d = new Date(iso + 'T12:00:00Z');
    d.setUTCDate(d.getUTCDate() + n);
    return d.toISOString().slice(0, 10);
  }
  function dayLabel(iso) {
    return new Date(iso + 'T12:00:00Z').toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' });
  }

  // ---- period editors ----
  function addRow(listId, period = {}) {
    const row = document.createElement('div');
    row.className = 'ss-period';
    const name = document.createElement('input');
    name.type = 'text'; name.maxLength = 60; name.placeholder = 'Period name'; name.value = period.name || '';
    name.setAttribute('aria-label', 'Period name');
    const start = document.createElement('input');
    start.type = 'time'; start.value = period.start || ''; start.setAttribute('aria-label', 'Start time');
    const end = document.createElement('input');
    end.type = 'time'; end.value = period.end || ''; end.setAttribute('aria-label', 'End time');
    const remove = document.createElement('button');
    remove.type = 'button'; remove.textContent = 'Remove';
    remove.addEventListener('click', () => row.remove());
    row.append(name, start, end, remove);
    $(listId).append(row);
  }

  function setPeriods(listId, periods) {
    $(listId).replaceChildren();
    for (const p of periods) addRow(listId, p);
  }

  function readPeriods(listId) {
    return [...$(listId).children]
      .map(row => { const [n, s, e] = row.querySelectorAll('input'); return { name: n.value.trim(), start: s.value, end: e.value }; })
      .filter(p => p.name || p.start || p.end);
  }

  const sameLunch = (a, b) => JSON.stringify(a.map(({ name, start, end }) => [name, start, end])) === JSON.stringify(b.map(({ name, start, end }) => [name, start, end]));

  function syncLunch2() {
    const same = $('same').checked;
    $('lunch-2').hidden = same;
    if (!same && !$('periods-2').children.length) setPeriods('periods-2', readPeriods('periods-1'));
  }

  function loadSchedule(schedule, name) {
    setPeriods('periods-1', schedule.lunch1 || []);
    setPeriods('periods-2', schedule.lunch2 || []);
    $('same').checked = sameLunch(schedule.lunch1 || [], schedule.lunch2 || []);
    syncLunch2();
    if (name) $('name').value = name;
  }

  // ---- rendering ----
  function renderForm() {
    const school = $('school');
    const keep = school.value;
    school.replaceChildren(...state.schools.map(s => new Option(s.label, s.key)));
    if (keep) school.value = keep;
    const preset = $('preset');
    preset.replaceChildren(new Option('Blank schedule', ''), ...state.presets.map((p, i) => new Option(p.name, String(i))));
    $('date').min = state.today;
    if (!$('date').value) $('date').value = state.today;
  }

  function renderActive() {
    const list = $('active-list');
    list.replaceChildren();
    if (!state.overrides.length) {
      const empty = document.createElement('p');
      empty.className = 'ss-empty';
      empty.textContent = 'No special schedules are set.';
      list.append(empty);
      return;
    }
    for (const o of state.overrides) {
      const row = document.createElement('article');
      row.className = 'ss-item' + (o.date < state.today ? ' ss-item-past' : '');
      row.setAttribute('role', 'listitem');
      const copy = document.createElement('div');
      copy.className = 'ss-item-copy';
      const time = document.createElement('time');
      time.textContent = schoolLabel(o.school) + ' · ' + dayLabel(o.date) + (o.date < state.today ? ' (passed)' : '');
      const title = document.createElement('strong');
      title.textContent = o.name;
      copy.append(time, title);
      if (o.special_event) {
        const note = document.createElement('p');
        note.textContent = o.special_event;
        copy.append(note);
      }
      const buttons = document.createElement('div');
      buttons.className = 'ss-row';
      const edit = document.createElement('button');
      edit.type = 'button'; edit.textContent = 'Edit';
      edit.addEventListener('click', () => startEdit(o));
      const clear = document.createElement('button');
      clear.type = 'button'; clear.textContent = 'Clear';
      clear.addEventListener('click', () => clearOne(o));
      buttons.append(edit, clear);
      row.append(copy, buttons);
      list.append(row);
    }
    if (state.overrides.length > 1) {
      const all = document.createElement('button');
      all.type = 'button'; all.textContent = 'Clear all';
      all.addEventListener('click', clearAll);
      list.append(all);
    }
  }

  function startEdit(o) {
    $('school').value = o.school;
    $('date').value = o.date < state.today ? state.today : o.date;
    $('event').value = o.special_event || '';
    $('preset').value = '';
    loadSchedule(o.schedule, o.name);
    $('name').focus();
    say('form-status', 'Editing ' + schoolLabel(o.school) + '. Save to replace it.');
  }

  async function refresh() {
    state = await request('/api/admin/special-schedule');
    renderForm();
    renderActive();
    $('passphrase-lock').hidden = true;
    $('ss-workspace').hidden = false;
    say('access-status', '');
  }

  async function clearRequest(school, confirmText) {
    if (!confirm(confirmText)) return;
    say('active-status', 'Clearing…');
    try {
      await request('/api/admin/special-schedule/clear', { method: 'POST', body: JSON.stringify({ school }) });
      await refresh();
      say('active-status', 'Cleared.');
    } catch (error) {
      say('active-status', error.message, true);
      if (needsPassphrase(error)) showLock();
    }
  }
  const clearOne = o => clearRequest(o.school, 'Clear the ' + schoolLabel(o.school) + ' schedule "' + o.name + '"?');
  const clearAll = () => clearRequest('all', 'Clear every special schedule?');

  // ---- events ----
  $('preset').addEventListener('change', () => {
    const preset = state.presets[Number($('preset').value)];
    if ($('preset').value === '' || !preset) return;
    loadSchedule(preset.schedule, preset.name);
  });
  $('same').addEventListener('change', syncLunch2);
  $('add-1').addEventListener('click', () => addRow('periods-1'));
  $('add-2').addEventListener('click', () => addRow('periods-2'));
  $('day-today').addEventListener('click', () => { $('date').value = state.today; });
  $('day-tomorrow').addEventListener('click', () => { $('date').value = addDays(state.today, 1); });

  $('schedule-form').addEventListener('submit', async event => {
    event.preventDefault();
    const lunch1 = readPeriods('periods-1');
    const lunch2 = $('same').checked ? lunch1 : readPeriods('periods-2');
    const payload = { school: $('school').value, date: $('date').value, name: $('name').value, specialEvent: $('event').value, lunch1, lunch2 };
    $('save').disabled = true;
    say('form-status', 'Saving…');
    try {
      const data = await request('/api/admin/special-schedule', { method: 'POST', body: JSON.stringify(payload) });
      await refresh();
      say('form-status', schoolLabel(data.override.school) + ' will show "' + data.override.name + '" on ' + dayLabel(data.override.date) + '.');
    } catch (error) {
      say('form-status', error.message, true);
      if (needsPassphrase(error)) showLock();
    } finally {
      $('save').disabled = false;
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
      await refresh();
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
      const isAdmin = me.isAdmin || me.isOwner || me.isCoOwner;
      if (!isAdmin && !me.isModerator) { say('access-status', 'Admin or moderator access is required.', true); return; }
      if (!isAdmin) { await refresh(); return; } // moderators skip the passphrase
      setup = !(await request('/api/admin/passphrase-status')).set;
      if (!setup && sessionStorage.getItem('admin_passphrase')) {
        try { await refresh(); return; } catch (error) { if (!needsPassphrase(error)) throw error; }
      }
      showLock();
    } catch (error) {
      say('access-status', error.message, true);
    }
  })();
})();
