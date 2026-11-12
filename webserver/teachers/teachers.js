(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var state = { school: '', canRate: false, current: null, picks: {}, timer: 0 };
  var CATS = [
    ['overall', 'Overall', 'Required'],
    ['teaching', 'Teaching', 'How well they explain things'],
    ['homework', 'Homework', '5 is very little'],
    ['chill', 'Chill', '5 is very relaxed']
  ];

  function el(tag, cls, text) {
    var n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  }

  function say(id, text, bad) {
    var s = $(id);
    s.className = 'rt-status' + (bad ? ' err' : '');
    s.textContent = text || '';
  }

  async function api(path, body) {
    var opts = { credentials: 'same-origin', cache: 'no-store', headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' } };
    if (body) { opts.method = 'POST'; opts.body = JSON.stringify(body); }
    var res = await fetch(path, opts);
    var data = await res.json().catch(function () { return {}; });
    if (!res.ok) { var e = new Error(data.error || 'Request failed.'); e.code = data.code; e.status = res.status; throw e; }
    return data;
  }

  function stars(value) {
    var s = el('span', 'rt-stars');
    s.setAttribute('role', 'img');
    s.setAttribute('aria-label', value == null ? 'No ratings' : value + ' out of 5');
    var fill = el('i');
    fill.style.width = (value == null ? 0 : Math.max(0, Math.min(5, value)) * 20) + '%';
    s.append(fill);
    return s;
  }

  function renderList(data) {
    var list = $('list');
    list.replaceChildren();
    if (!data.teachers.length) {
      list.append(el('p', 'rt-sub', data.total ? 'No matches.' : 'No teachers are listed for this school yet.'));
      return;
    }
    data.teachers.forEach(function (t) {
      var b = el('button', 'rt-row');
      b.type = 'button';
      var left = el('span');
      left.append(el('strong', '', t.name));
      if (t.department) left.append(el('small', '', t.department));
      var right = el('span', 'rt-score');
      if (t.rated) right.append(el('span', 'rt-tag', 'Rated'));
      right.append(stars(t.ratings.overall));
      right.append(el('span', '', t.ratings.count ? t.ratings.overall.toFixed(1) + ' (' + t.ratings.count + ')' : 'No ratings'));
      b.append(left, right);
      b.onclick = function () { openTeacher(t.id); };
      list.append(b);
    });
  }

  async function load() {
    try {
      var data = await api('/api/teachers?school=' + encodeURIComponent(state.school) + '&q=' + encodeURIComponent($('q').value.trim()));
      if (!state.school) {
        state.school = data.school;
        var sel = $('school');
        data.schools.forEach(function (s) { var o = el('option', '', s.name); o.value = s.key; sel.append(o); });
        sel.value = state.school;
      }
      state.canRate = data.canRate;
      renderList(data);
      say('status', '');
    } catch (err) { say('status', err.message, true); }
  }

  function pickerRow(cat) {
    var row = el('div', 'rt-pick');
    var label = el('span', '', cat[1]);
    label.append(el('small', '', cat[2]));
    var group = el('div', 'rt-star-group');
    group.setAttribute('role', 'radiogroup');
    group.setAttribute('aria-label', cat[1]);
    var buttons = [];
    function paint() { buttons.forEach(function (b, i) { b.className = i < (state.picks[cat[0]] || 0) ? 'on' : ''; b.setAttribute('aria-checked', String(state.picks[cat[0]] === i + 1)); }); }
    for (var i = 1; i <= 5; i++) {
      (function (n) {
        var b = el('button', '', '★');
        b.type = 'button';
        b.setAttribute('role', 'radio');
        b.setAttribute('aria-label', n + (n === 1 ? ' star' : ' stars'));
        b.onclick = function () { state.picks[cat[0]] = (cat[0] !== 'overall' && state.picks[cat[0]] === n) ? 0 : n; paint(); };
        buttons.push(b); group.append(b);
      })(i);
    }
    paint();
    row.append(label, group);
    return row;
  }

  function renderSummary(r) {
    var box = $('d-summary');
    box.replaceChildren();
    if (!r.count) { box.append(el('p', 'rt-sub', 'No ratings yet.')); return; }
    [['overall', 'Overall'], ['teaching', 'Teaching'], ['homework', 'Homework (5 is very little)'], ['chill', 'Chill']].forEach(function (c) {
      if (r[c[0]] == null) return;
      var row = el('div', 'rt-sumrow');
      row.append(el('span', '', c[1]));
      var right = el('span', 'rt-score');
      right.append(stars(r[c[0]]), el('span', '', r[c[0]].toFixed(1)));
      row.append(right);
      box.append(row);
    });
    box.append(el('p', 'rt-sub', r.count + (r.count === 1 ? ' rating' : ' ratings')));
  }

  async function openTeacher(id) {
    try {
      var d = await api('/api/teachers/one?id=' + id);
      state.current = d;
      state.picks = {};
      $('d-name').textContent = d.teacher.name;
      $('d-meta').textContent = [d.teacher.department, d.teacher.schoolName].filter(Boolean).join(' · ');
      renderSummary(d.ratings);
      var notes = $('d-notes');
      notes.replaceChildren();
      d.notes.forEach(function (n) { notes.append(el('blockquote', '', n.text)); });
      var form = $('rate-form'), done = $('d-done');
      form.hidden = !(d.canRate && !d.rated);
      done.hidden = !(d.canRate && d.rated);
      if (d.rated) done.textContent = 'You rated this teacher.';
      $('rate-rows').replaceChildren.apply($('rate-rows'), CATS.map(pickerRow));
      $('note').value = '';
      say('rate-status', '');
      $('submit').disabled = false;
      $('teacher-dialog').showModal();
    } catch (err) { say('status', err.message, true); }
  }

  $('rate-form').onsubmit = async function (e) {
    e.preventDefault();
    if (!state.picks.overall) { say('rate-status', 'Pick an overall rating.', true); return; }
    var body = { teacherId: state.current.teacher.id, overall: state.picks.overall, note: $('note').value.trim() };
    ['teaching', 'homework', 'chill'].forEach(function (k) { if (state.picks[k]) body[k] = state.picks[k]; });
    $('submit').disabled = true;
    try {
      var res = await api('/api/teachers/rate', body);
      $('rate-form').hidden = true;
      $('d-done').hidden = false;
      $('d-done').textContent = res.noteQueued ? 'Rating saved. Your note will show after a moderator reads it.' : 'Rating saved.';
      renderSummary(res.ratings);
      load();
    } catch (err) {
      say('rate-status', err.message, true);
      $('submit').disabled = err.status === 409;
    }
  };

  $('school').onchange = function () { state.school = $('school').value; load(); };
  $('q').oninput = function () { clearTimeout(state.timer); state.timer = setTimeout(load, 200); };

  (async function () {
    try {
      await load0();
    } catch (err) {
      var d = $('denied');
      d.hidden = false;
      d.textContent = err.status === 401 ? 'Sign in with your student account to rate teachers.' : err.message;
    }
  })();

  async function load0() {
    var data = await api('/api/teachers');
    $('app').hidden = false;
    state.school = data.school;
    var sel = $('school');
    data.schools.forEach(function (s) { var o = el('option', '', s.name); o.value = s.key; sel.append(o); });
    sel.value = state.school;
    state.canRate = data.canRate;
    renderList(data);
  }
})();
