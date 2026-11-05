(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var characters = [];
  var mine = [];
  var selectedId = '';
  var subscriptionActive = false;

  function api(url, opts) {
    opts = opts || {};
    var headers = Object.assign({ 'X-Mitch-Requested-With': '1' }, opts.headers || {});
    if (opts.body) headers['Content-Type'] = 'application/json';
    return fetch(url, Object.assign({ credentials: 'same-origin', cache: 'no-store' }, opts, { headers: headers }))
      .then(function (r) {
        return r.json().catch(function () { return {}; }).then(function (data) { return { ok: r.ok, status: r.status, data: data }; });
      })
      .catch(function () {
        return { ok: false, status: 0, data: { error: 'Network error. Check your connection and try again.' } };
      });
  }

  // Disables a button and swaps its label while `promise` is pending.
  function busy(btn, label, promise) {
    var orig = btn.textContent;
    btn.disabled = true;
    btn.textContent = label;
    return promise.then(function (v) { btn.disabled = false; btn.textContent = orig; return v; });
  }

  function errMsg(res, fallback) {
    return (res.data && res.data.error) || fallback;
  }

  function showStatus(msg) {
    var s = $('companion-setup-status');
    s.textContent = msg;
    if (msg) s.scrollIntoView({ block: 'nearest' });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  // Bumped only when an image is actually uploaded/removed (see
  // loadMine's callers below) so a just-changed mood's picture is never
  // shown stale. A blanket `&t=' + Date.now()` here (the previous
  // version) defeated the server's `Cache-Control: private,
  // max-age=3600` on every single call — every grid/mood re-render
  // (which happens often: after every unlock, save, create, upload...)
  // issued a brand new network request for every character/mood shown,
  // which is what actually blew through the rate limit and produced a
  // wall of 429s during normal use, not abuse. Reusing the same URL
  // across unrelated re-renders lets the browser serve those straight
  // from cache instead.
  var imgVersion = 0;
  function imageUrl(id, mood) {
    return '/api/companion/character-image?id=' + encodeURIComponent(id) + '&mood=' + encodeURIComponent(mood) + '&v=' + imgVersion;
  }

  function formatExpiry(ms) {
    return new Date(ms).toLocaleString();
  }

  function renderSubscription(data) {
    subscriptionActive = !!data.subscriptionActive;
    var el = $('companion-subscription-status');
    if (subscriptionActive) {
      el.textContent = 'Active until ' + formatExpiry(data.subscriptionExpiresAt) + '.';
    } else {
      el.textContent = 'Not subscribed — subscribe to enable the companion.';
    }
  }

  function subscribe(plan) {
    var el = $('companion-subscription-status');
    var btn = $(plan === 'week' ? 'companion-sub-week' : 'companion-sub-month');
    var other = $(plan === 'week' ? 'companion-sub-month' : 'companion-sub-week');
    el.textContent = 'Subscribing...';
    other.disabled = true;
    busy(btn, 'Subscribing...', api('/api/companion/subscribe', { method: 'POST', body: JSON.stringify({ plan: plan }) })).then(function (res) {
      other.disabled = false;
      if (!res.ok) {
        el.textContent = errMsg(res, 'Could not subscribe.');
        return;
      }
      renderSubscription({ subscriptionActive: true, subscriptionExpiresAt: res.data.expiresAt });
    });
  }

  $('companion-sub-week').addEventListener('click', function () { subscribe('week'); });
  $('companion-sub-month').addEventListener('click', function () { subscribe('month'); });

  function renderGrid() {
    var grid = $('companion-setup-grid');
    grid.innerHTML = '';
    if (!characters.length) {
      grid.textContent = 'No characters are available yet.';
      return;
    }
    characters.forEach(function (c) {
      var thumbMood = c.moods && c.moods.length ? (c.moods.indexOf('idle') !== -1 ? 'idle' : c.moods[0]) : '';
      var card = document.createElement('div');
      card.className = 'companion-setup-card' + (c.id === selectedId ? ' selected' : '') + (!c.unlocked ? ' locked' : '');
      card.innerHTML =
        (thumbMood ? '<img src="' + imageUrl(c.id, thumbMood) + '" alt="">' : '') +
        '<div class="companion-setup-card-name">' + escapeHtml(c.name) + '</div>' +
        (c.specialAbility === 'sandevistan' ? '<div class="companion-setup-card-ability">Sandevistan</div>' : '') +
        (c.priceCoins > 0 ? '<div class="companion-setup-card-price">' + c.priceCoins + ' coins</div>' : '') +
        (!c.unlocked ? '<button type="button" class="companion-unlock-btn">Unlock for ' + c.priceCoins + ' coins</button>' : '');
      if (c.unlocked) {
        card.tabIndex = 0;
        card.setAttribute('role', 'button');
        card.setAttribute('aria-pressed', c.id === selectedId ? 'true' : 'false');
        var choose = function () {
          selectedId = c.id;
          renderGrid();
          var again = $('companion-setup-grid').querySelector('.selected');
          if (again) again.focus();
        };
        card.addEventListener('click', choose);
        card.addEventListener('keydown', function (e) {
          if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); choose(); }
        });
      } else {
        card.querySelector('.companion-unlock-btn').addEventListener('click', function (e) {
          e.stopPropagation();
          busy(e.currentTarget, 'Unlocking...', api('/api/companion/characters/unlock', { method: 'POST', body: JSON.stringify({ characterId: c.id }) }))
            .then(function (res) {
              if (!res.ok) {
                showStatus(errMsg(res, 'Could not unlock.'));
                return;
              }
              showStatus('');
              c.unlocked = true;
              selectedId = c.id;
              renderGrid();
            });
        });
      }
      grid.appendChild(card);
    });
  }

  // Chrome: Sandevistan strain and upgrades for every unlocked character
  // with a special ability (state and catalog come from
  // /api/companion/ability, backed by mitch_lib::companion_chrome).
  function renderChrome() {
    var withAbility = characters.filter(function (c) { return c.specialAbility && c.unlocked; });
    var list = $('companion-chrome-list');
    $('companion-chrome').hidden = !withAbility.length;
    list.innerHTML = '';
    withAbility.forEach(function (c) {
      var panel = document.createElement('div');
      panel.className = 'companion-chrome-panel';
      panel.textContent = 'Loading...';
      list.appendChild(panel);
      api('/api/companion/ability?characterId=' + encodeURIComponent(c.id)).then(function (res) {
        if (!res.ok) { panel.textContent = errMsg(res, 'Could not load chrome.'); return; }
        paintChrome(panel, c, res.data);
      });
    });
  }

  function paintChrome(panel, c, s) {
    var pct = Math.round((s.strain / (s.maxStrain || 100)) * 100);
    var psychosisLeft = s.psychosisUntil > s.now ? Math.ceil((s.psychosisUntil - s.now) / 60000) : 0;
    var condition = psychosisLeft ? 'Cyberpsychosis, ' + psychosisLeft + ' min left'
      : s.bleeding ? 'Coughing up blood. Let him rest.'
      : pct > 0 ? 'Recovering' : 'Stable';
    panel.innerHTML =
      '<div class="companion-chrome-head"><strong>' + escapeHtml(c.name) + '</strong><span class="companion-chrome-condition' + (psychosisLeft ? ' psychosis' : s.bleeding ? ' bleeding' : '') + '">' + escapeHtml(condition) + '</span></div>' +
      '<div class="companion-strain" role="meter" aria-label="Sandevistan strain" aria-valuemin="0" aria-valuemax="100" aria-valuenow="' + pct + '"><span style="width:' + pct + '%"></span></div>' +
      '<p class="companion-chrome-note">Strain ' + pct + ' / 100. Each Sandevistan use adds strain; it drains while he rests.</p>' +
      '<ul class="companion-upgrades"></ul>' +
      '<p class="companion-chrome-status" role="status"></p>';
    var ul = panel.querySelector('.companion-upgrades');
    (s.catalog || []).forEach(function (u) {
      var li = document.createElement('li');
      var needs = u.requires && (s.upgrades || []).indexOf(u.requires) === -1;
      var reqName = needs ? ((s.catalog.filter(function (x) { return x.id === u.requires; })[0] || {}).name || u.requires) : '';
      li.innerHTML = '<div><strong>' + escapeHtml(u.name) + '</strong><small>' + escapeHtml(u.description) + '</small></div>' +
        (u.owned ? '<span class="companion-upgrade-owned">Installed</span>'
          : '<button type="button"' + (needs ? ' disabled title="Needs ' + escapeHtml(reqName) + ' first"' : '') + '>' + (needs ? 'Needs ' + escapeHtml(reqName) : 'Install, ' + u.priceCoins + ' coins') + '</button>');
      var btn = li.querySelector('button');
      if (btn && !needs) {
        btn.addEventListener('click', function () {
          busy(btn, 'Installing...', api('/api/companion/upgrades/buy', { method: 'POST', body: JSON.stringify({ characterId: c.id, upgrade: u.id }) }))
            .then(function (res) {
              if (!res.ok) { panel.querySelector('.companion-chrome-status').textContent = errMsg(res, 'Could not install.'); return; }
              paintChrome(panel, c, res.data);
              window.dispatchEvent(new CustomEvent('mitch:coins-refresh'));
            });
        });
      }
      ul.appendChild(li);
    });
  }

  function loadCharacters() {
    return api('/api/companion/characters').then(function (res) {
      if (!res.ok) {
        $('companion-setup-grid').innerHTML = '';
        var msg = document.createElement('p');
        msg.textContent = errMsg(res, 'Could not load characters.') + ' ';
        var retry = document.createElement('button');
        retry.type = 'button';
        retry.textContent = 'Retry';
        retry.addEventListener('click', function () { $('companion-setup-grid').textContent = 'Loading...'; loadCharacters(); });
        msg.appendChild(retry);
        $('companion-setup-grid').appendChild(msg);
        return;
      }
      characters = res.data.characters || [];
      renderChrome();
      if (!selectedId) {
        var def = characters.filter(function (c) { return c.isDefault; })[0];
        if (def) selectedId = def.id;
      }
      renderGrid();
    });
  }

  function init() {
    var status = $('companion-setup-status');
    $('companion-setup-grid').textContent = 'Loading...';
    api('/api/companion/settings').then(function (res) {
      if (res.status === 401) {
        location.href = '/enroll/?next=' + encodeURIComponent('/companion/');
        return;
      }
      if (!res.ok) {
        status.textContent = errMsg(res, 'Could not load your companion settings.') + ' Reload the page to try again.';
        $('companion-subscription-status').textContent = '';
        return;
      }
      $('companion-setup-enabled').checked = !!res.data.enabled;
      selectedId = res.data.activeCharacterId || '';
      renderSubscription(res.data);
      status.textContent = '';
      return loadCharacters();
    });
    loadMine();
  }

  $('companion-setup-save').addEventListener('click', function () {
    var status = $('companion-setup-status');
    var enabled = $('companion-setup-enabled').checked;
    if (enabled && !subscriptionActive) {
      status.textContent = 'Subscribe first.';
      return;
    }
    if (enabled && !selectedId) {
      status.textContent = 'Choose a character first.';
      return;
    }
    status.textContent = 'Saving...';
    busy($('companion-setup-save'), 'Saving...', api('/api/companion/settings', {
      method: 'POST',
      body: JSON.stringify({ enabled: enabled, activeCharacterId: selectedId }),
    })).then(function (res) {
      if (!res.ok) {
        status.textContent = errMsg(res, 'Could not save.');
        return;
      }
      // Reload so companion-widget.js re-reads the new settings and
      // shows/hides the live character accordingly.
      $('companion-setup-save').disabled = true;
      location.reload();
    });
  });

  // ---- Create your own character ----

  $('companion-create-form').addEventListener('submit', function (e) {
    e.preventDefault();
    var status = $('companion-create-status');
    var submit = e.currentTarget.querySelector('button[type="submit"]');
    status.textContent = '';
    var payload = {
      name: $('companion-create-name').value.trim(),
      personalityPrompt: $('companion-create-prompt').value.trim(),
      priceCoins: Number($('companion-create-price').value) || 0,
    };
    busy(submit, 'Creating...', api('/api/companion/mine', { method: 'POST', body: JSON.stringify(payload) })).then(function (res) {
      if (!res.ok) {
        status.textContent = errMsg(res, 'Could not create character.');
        return;
      }
      status.textContent = 'Created — add its mood images below in "Your characters".';
      $('companion-create-form').reset();
      loadMine().then(function () {
        var card = document.getElementById('mine-' + res.data.id);
        if (card) card.scrollIntoView({ behavior: 'smooth', block: 'start' });
      });
      loadCharacters();
    });
  });

  // ---- Your characters ----

  function loadMine() {
    return api('/api/companion/mine').then(function (res) {
      if (!res.ok) {
        var list = $('companion-mine-list');
        list.textContent = errMsg(res, 'Could not load your characters.') + ' ';
        var retry = document.createElement('button');
        retry.type = 'button';
        retry.textContent = 'Retry';
        retry.addEventListener('click', loadMine);
        list.appendChild(retry);
        return;
      }
      mine = res.data.characters || [];
      renderMine();
    });
  }

  function renderMine() {
    var list = $('companion-mine-list');
    list.innerHTML = '';
    if (!mine.length) {
      list.textContent = "You haven't created any characters yet. Use the form above to make one.";
      return;
    }
    mine.forEach(function (c) {
      var card = document.createElement('div');
      card.className = 'companion-mine-card';
      card.id = 'mine-' + c.id;
      card.innerHTML =
        '<h3>' + escapeHtml(c.name) + (c.needsReview ? ' <span class="companion-review-tag">Pending review</span>' : '') + '</h3>' +
        '<label>Mood images</label>' +
        (c.moods && c.moods.length ? '' : '<p class="companion-mine-no-images">No images yet — add at least one below so this character can be used.</p>') +
        '<div class="companion-mine-moods"></div>' +
        '<form class="companion-mine-image-form">' +
        '<input type="text" class="mine-mood-name" placeholder="mood name (e.g. happy)" maxlength="40" required>' +
        '<input type="file" class="mine-mood-file" accept="image/png,image/jpeg,image/webp,image/gif" required>' +
        '<button type="submit">Add mood image</button>' +
        '</form>' +
        '<label>Name</label><input type="text" class="mine-name" maxlength="60" value="' + escapeHtml(c.name) + '">' +
        '<label>Personality prompt</label><textarea class="mine-prompt" rows="4" maxlength="16000">' + escapeHtml(c.personalityPrompt) + '</textarea>' +
        '<label>Price for others to unlock (coins)</label><input type="number" class="mine-price" min="0" step="1" value="' + c.priceCoins + '">' +
        '<button type="button" class="mine-save-btn">Save changes</button>' +
        '<button type="button" class="mine-delete-btn">Delete character</button>' +
        '<p class="mine-msg" role="status"></p>';

      function setCardMsg(msg) { card.querySelector('.mine-msg').textContent = msg; }

      var moodsEl = card.querySelector('.companion-mine-moods');
      (c.moods || []).forEach(function (mood) {
        var tile = document.createElement('div');
        tile.className = 'companion-mine-mood-tile';
        tile.innerHTML =
          '<img src="' + imageUrl(c.id, mood) + '" alt="' + escapeHtml(mood) + '">' +
          '<div>' + escapeHtml(mood) + '</div>' +
          '<button type="button" class="mine-remove-mood-btn">Remove</button>';
        tile.querySelector('.mine-remove-mood-btn').addEventListener('click', function (e) {
          busy(e.currentTarget, 'Removing...', api('/api/companion/mine/image/delete', { method: 'POST', body: JSON.stringify({ id: c.id, mood: mood }) }))
            .then(function (res) {
              if (!res.ok) { setCardMsg(errMsg(res, 'Could not remove image.')); return; }
              imgVersion++; loadMine();
            });
        });
        moodsEl.appendChild(tile);
      });

      card.querySelector('.mine-save-btn').addEventListener('click', function (e) {
        var payload = {
          id: c.id,
          name: card.querySelector('.mine-name').value.trim(),
          personalityPrompt: card.querySelector('.mine-prompt').value.trim(),
          priceCoins: Number(card.querySelector('.mine-price').value) || 0,
        };
        busy(e.currentTarget, 'Saving...', api('/api/companion/mine/update', { method: 'POST', body: JSON.stringify(payload) })).then(function (res) {
          if (!res.ok) {
            setCardMsg(errMsg(res, 'Could not save.'));
            return;
          }
          setCardMsg('Saved.');
          loadMine();
          loadCharacters();
        });
      });
      card.querySelector('.mine-delete-btn').addEventListener('click', function (e) {
        if (!confirm('Delete "' + c.name + '" and all its images? Anyone who unlocked it loses access.')) return;
        busy(e.currentTarget, 'Deleting...', api('/api/companion/mine/delete', { method: 'POST', body: JSON.stringify({ id: c.id }) })).then(function (res) {
          if (!res.ok) { setCardMsg(errMsg(res, 'Could not delete.')); return; }
          loadMine();
          loadCharacters();
        });
      });
      card.querySelector('.companion-mine-image-form').addEventListener('submit', function (e) {
        e.preventDefault();
        var moodInput = card.querySelector('.mine-mood-name');
        var fileInput = card.querySelector('.mine-mood-file');
        var submitBtn = e.currentTarget.querySelector('button[type="submit"]');
        var form = new FormData();
        form.append('id', c.id);
        form.append('mood', moodInput.value.trim());
        form.append('file', fileInput.files[0]);
        busy(submitBtn, 'Uploading...', fetch('/api/companion/mine/image', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'X-Mitch-Requested-With': '1' },
          body: form,
        })
          .then(function (r) { return r.json().catch(function () { return {}; }).then(function (d) { return { ok: r.ok, data: d }; }); })
          .catch(function () { return { ok: false, data: { error: 'Network error. Try again.' } }; }))
          .then(function (res) {
            if (!res.ok) { setCardMsg(errMsg(res, 'Upload failed.')); return; }
            imgVersion++;
            loadMine();
          });
      });

      list.appendChild(card);
    });
  }

  init();
})();
