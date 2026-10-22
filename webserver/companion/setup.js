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
      .then(function (r) { return r.json().then(function (data) { return { ok: r.ok, status: r.status, data: data }; }); });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function imageUrl(id, mood) {
    return '/api/companion/character-image?id=' + encodeURIComponent(id) + '&mood=' + encodeURIComponent(mood) + '&t=' + Date.now();
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
    el.textContent = 'Subscribing...';
    api('/api/companion/subscribe', { method: 'POST', body: JSON.stringify({ plan: plan }) }).then(function (res) {
      if (!res.ok) {
        el.textContent = (res.data && res.data.error) || 'Could not subscribe.';
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
        (c.priceCoins > 0 ? '<div class="companion-setup-card-price">' + c.priceCoins + ' coins</div>' : '') +
        (!c.unlocked ? '<button type="button" class="companion-unlock-btn">Unlock for ' + c.priceCoins + ' coins</button>' : '');
      if (c.unlocked) {
        card.addEventListener('click', function () {
          selectedId = c.id;
          renderGrid();
        });
      } else {
        card.querySelector('.companion-unlock-btn').addEventListener('click', function (e) {
          e.stopPropagation();
          api('/api/companion/characters/unlock', { method: 'POST', body: JSON.stringify({ characterId: c.id }) })
            .then(function (res) {
              if (!res.ok) {
                alert((res.data && res.data.error) || 'Could not unlock.');
                return;
              }
              c.unlocked = true;
              selectedId = c.id;
              renderGrid();
            });
        });
      }
      grid.appendChild(card);
    });
  }

  function loadCharacters() {
    return api('/api/companion/characters').then(function (res) {
      if (!res.ok) {
        $('companion-setup-status').textContent = 'Could not load characters.';
        return;
      }
      characters = res.data.characters || [];
      if (!selectedId) {
        var def = characters.filter(function (c) { return c.isDefault; })[0];
        if (def) selectedId = def.id;
      }
      renderGrid();
    });
  }

  function init() {
    var status = $('companion-setup-status');
    api('/api/companion/settings').then(function (res) {
      if (res.status === 401) {
        location.href = '/enroll/?next=' + encodeURIComponent('/companion/');
        return;
      }
      if (!res.ok) {
        status.textContent = 'Could not load your companion settings.';
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
    api('/api/companion/settings', {
      method: 'POST',
      body: JSON.stringify({ enabled: enabled, activeCharacterId: selectedId }),
    }).then(function (res) {
      if (!res.ok) {
        status.textContent = (res.data && res.data.error) || 'Could not save.';
        return;
      }
      // Reload so companion-widget.js re-reads the new settings and
      // shows/hides the live character accordingly.
      location.reload();
    });
  });

  // ---- Create your own character ----

  $('companion-create-form').addEventListener('submit', function (e) {
    e.preventDefault();
    var status = $('companion-create-status');
    status.textContent = 'Creating (500 coins)...';
    var payload = {
      name: $('companion-create-name').value.trim(),
      personalityPrompt: $('companion-create-prompt').value.trim(),
      priceCoins: Number($('companion-create-price').value) || 0,
    };
    api('/api/companion/mine', { method: 'POST', body: JSON.stringify(payload) }).then(function (res) {
      if (!res.ok) {
        status.textContent = (res.data && res.data.error) || 'Could not create character.';
        return;
      }
      status.textContent = 'Created.';
      $('companion-create-form').reset();
      loadMine();
      loadCharacters();
    });
  });

  // ---- Your characters ----

  function loadMine() {
    api('/api/companion/mine').then(function (res) {
      if (!res.ok) return;
      mine = res.data.characters || [];
      renderMine();
    });
  }

  function renderMine() {
    var list = $('companion-mine-list');
    list.innerHTML = '';
    if (!mine.length) {
      list.textContent = "You haven't created any characters yet.";
      return;
    }
    mine.forEach(function (c) {
      var card = document.createElement('div');
      card.className = 'companion-mine-card';
      card.innerHTML =
        '<h3>' + escapeHtml(c.name) + (c.needsReview ? ' <span class="companion-review-tag">Pending review</span>' : '') + '</h3>' +
        '<label>Name</label><input type="text" class="mine-name" maxlength="60" value="' + escapeHtml(c.name) + '">' +
        '<label>Personality prompt</label><textarea class="mine-prompt" rows="4" maxlength="4000">' + escapeHtml(c.personalityPrompt) + '</textarea>' +
        '<label>Price for others to unlock (coins)</label><input type="number" class="mine-price" min="0" step="1" value="' + c.priceCoins + '">' +
        '<div class="companion-mine-moods"></div>' +
        '<button type="button" class="mine-save-btn">Save changes</button>' +
        '<button type="button" class="mine-delete-btn">Delete character</button>' +
        '<form class="companion-mine-image-form">' +
        '<input type="text" class="mine-mood-name" placeholder="mood name (e.g. happy)" maxlength="40" required>' +
        '<input type="file" class="mine-mood-file" accept="image/png,image/jpeg,image/webp,image/gif" required>' +
        '<button type="submit">Upload image</button>' +
        '</form>';

      var moodsEl = card.querySelector('.companion-mine-moods');
      (c.moods || []).forEach(function (mood) {
        var tile = document.createElement('div');
        tile.className = 'companion-mine-mood-tile';
        tile.innerHTML =
          '<img src="' + imageUrl(c.id, mood) + '" alt="' + escapeHtml(mood) + '">' +
          '<div>' + escapeHtml(mood) + '</div>' +
          '<button type="button" class="mine-remove-mood-btn">Remove</button>';
        tile.querySelector('.mine-remove-mood-btn').addEventListener('click', function () {
          api('/api/companion/mine/image/delete', { method: 'POST', body: JSON.stringify({ id: c.id, mood: mood }) })
            .then(loadMine);
        });
        moodsEl.appendChild(tile);
      });

      card.querySelector('.mine-save-btn').addEventListener('click', function () {
        var payload = {
          id: c.id,
          name: card.querySelector('.mine-name').value.trim(),
          personalityPrompt: card.querySelector('.mine-prompt').value.trim(),
          priceCoins: Number(card.querySelector('.mine-price').value) || 0,
        };
        api('/api/companion/mine/update', { method: 'POST', body: JSON.stringify(payload) }).then(function (res) {
          if (!res.ok) {
            alert((res.data && res.data.error) || 'Could not save.');
            return;
          }
          loadMine();
          loadCharacters();
        });
      });
      card.querySelector('.mine-delete-btn').addEventListener('click', function () {
        if (!confirm('Delete "' + c.name + '" and all its images? Anyone who unlocked it loses access.')) return;
        api('/api/companion/mine/delete', { method: 'POST', body: JSON.stringify({ id: c.id }) }).then(function () {
          loadMine();
          loadCharacters();
        });
      });
      card.querySelector('.companion-mine-image-form').addEventListener('submit', function (e) {
        e.preventDefault();
        var moodInput = card.querySelector('.mine-mood-name');
        var fileInput = card.querySelector('.mine-mood-file');
        var form = new FormData();
        form.append('id', c.id);
        form.append('mood', moodInput.value.trim());
        form.append('file', fileInput.files[0]);
        fetch('/api/companion/mine/image', {
          method: 'POST',
          credentials: 'same-origin',
          headers: { 'X-Mitch-Requested-With': '1' },
          body: form,
        })
          .then(function (r) { return r.json().then(function (d) { return { ok: r.ok, data: d }; }); })
          .then(function (res) {
            if (!res.ok) { alert(res.data.error || 'upload failed'); return; }
            loadMine();
          });
      });

      list.appendChild(card);
    });
  }

  init();
})();
