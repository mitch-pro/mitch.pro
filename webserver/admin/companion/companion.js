(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var editingId = '';
  var setup = false;

  function headers() {
    var result = { 'X-Mitch-Requested-With': '1' };
    var pass = sessionStorage.getItem('admin_passphrase');
    if (pass) result['X-Admin-Passphrase'] = pass;
    return result;
  }

  function status(id, text, isError) {
    $(id).textContent = text;
    $(id).dataset.error = String(!!isError);
  }

  function showLock() {
    $('companion-workspace').hidden = true;
    $('passphrase-lock').hidden = false;
    $('lock-heading').textContent = setup ? 'Set your admin passphrase' : 'Unlock companion management';
    $('unlock-button').textContent = setup ? 'Save and unlock' : 'Unlock';
    $('admin-passphrase').autocomplete = setup ? 'new-password' : 'current-password';
    status('access-status', 'Admin access verified. Enter your passphrase to continue.');
  }

  function requestJson(url, options) {
    options = options || {};
    var h = Object.assign({}, headers(), options.headers || {});
    if (options.body && typeof options.body === 'string') h['Content-Type'] = 'application/json';
    return fetch(url, Object.assign({ credentials: 'same-origin', cache: 'no-store' }, options, { headers: h }))
      .then(function (r) {
        return r.json().then(function (data) {
          if (!r.ok) {
            var error = new Error(data.error || 'request failed');
            error.code = data.error;
            throw error;
          }
          return data;
        });
      });
  }

  // Any admin API call can come back needing the passphrase (it may have
  // never been set, or sessionStorage's cached one may be stale/wrong) --
  // re-show the lock screen instead of just failing silently.
  function withLockHandling(promise) {
    return promise.catch(function (error) {
      if (error.code === 'invalid_passphrase' || error.code === 'passphrase_not_configured') {
        setup = error.code === 'passphrase_not_configured';
        showLock();
      }
      throw error;
    });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  // Bumped only on an actual image upload/delete (see below), not on
  // every render — `loadCharacters()` runs after almost every action
  // here (create/update/delete/set-default/mark-reviewed/...), so a
  // blanket cache-buster on every call defeats the server's
  // `Cache-Control: private, max-age=3600` on this endpoint entirely and
  // turns routine admin use into a flood of fresh image requests. See
  // the matching comment in webserver/companion/setup.js.
  var imgVersion = 0;
  function imageUrl(id, mood) {
    return '/api/companion/character-image?id=' + encodeURIComponent(id) + '&mood=' + encodeURIComponent(mood) + '&v=' + imgVersion;
  }

  function loadCharacters() {
    return withLockHandling(requestJson('/api/admin/companion/characters')).then(function (data) {
      $('passphrase-lock').hidden = true;
      $('companion-workspace').hidden = false;
      status('access-status', 'Admin access verified.');
      renderList(data.characters || []);
    });
  }

  function renderList(characters) {
    var list = $('character-list');
    list.innerHTML = '';
    // Needs-review characters surface first — that's the whole point of
    // the queue, and these are exclusively user-generated prompts.
    var sorted = characters.slice().sort(function (a, b) {
      return (b.needsReview ? 1 : 0) - (a.needsReview ? 1 : 0);
    });
    sorted.forEach(function (c) {
      var card = document.createElement('div');
      card.className = 'companion-card';
      card.innerHTML =
        '<h3>' + escapeHtml(c.name) +
        (c.isDefault ? ' <span class="companion-default-tag">Default</span>' : '') +
        (c.needsReview ? ' <span class="companion-review-tag">Needs review</span>' : '') +
        '</h3>' +
        (c.creatorEmail ? '<p class="companion-creator">Made by ' + escapeHtml(c.creatorEmail) + ' &middot; ' + c.priceCoins + ' coins to unlock</p>' : '') +
        '<p class="companion-prompt">' + escapeHtml(c.personalityPrompt) + '</p>' +
        '<div class="companion-moods"></div>' +
        '<div class="companion-card-actions">' +
        '<button type="button" class="edit-btn">Edit</button>' +
        '<button type="button" class="default-btn"' + (c.isDefault ? ' disabled' : '') + '>Set as default</button>' +
        (c.needsReview ? '<button type="button" class="reviewed-btn">Mark reviewed</button>' : '') +
        '<button type="button" class="delete-btn">Delete character</button>' +
        '</div>' +
        '<form class="companion-image-form">' +
        '<input type="text" class="mood-name" placeholder="mood name (e.g. happy)" maxlength="40" required>' +
        '<input type="file" class="mood-file" accept="image/png,image/jpeg,image/webp,image/gif" required>' +
        '<button type="submit">Upload image</button>' +
        '</form>';

      var moodsEl = card.querySelector('.companion-moods');
      (c.moods || []).forEach(function (mood) {
        var tile = document.createElement('div');
        tile.className = 'companion-mood-tile';
        tile.innerHTML =
          '<img src="' + imageUrl(c.id, mood) + '" alt="' + escapeHtml(mood) + '">' +
          '<div>' + escapeHtml(mood) + '</div>' +
          '<button type="button" class="remove-mood-btn">Remove</button>';
        tile.querySelector('.remove-mood-btn').addEventListener('click', function () {
          withLockHandling(requestJson('/api/admin/companion/characters/image/delete', {
            method: 'POST',
            body: JSON.stringify({ id: c.id, mood: mood }),
          })).then(function () { imgVersion++; return loadCharacters(); }).catch(function () {});
        });
        moodsEl.appendChild(tile);
      });

      card.querySelector('.edit-btn').addEventListener('click', function () {
        editingId = c.id;
        $('character-id').value = c.id;
        $('character-name').value = c.name;
        $('character-prompt').value = c.personalityPrompt;
        $('character-price').value = c.priceCoins;
        $('character-submit').textContent = 'Save changes';
        $('character-cancel-edit').hidden = false;
        $('form-heading').textContent = 'Edit character';
        window.scrollTo(0, 0);
      });
      card.querySelector('.default-btn').addEventListener('click', function () {
        withLockHandling(requestJson('/api/admin/companion/characters/set-default', {
          method: 'POST',
          body: JSON.stringify({ id: c.id }),
        })).then(loadCharacters).catch(function () {});
      });
      var reviewedBtn = card.querySelector('.reviewed-btn');
      if (reviewedBtn) {
        reviewedBtn.addEventListener('click', function () {
          withLockHandling(requestJson('/api/admin/companion/characters/mark-reviewed', {
            method: 'POST',
            body: JSON.stringify({ id: c.id }),
          })).then(loadCharacters).catch(function () {});
        });
      }
      card.querySelector('.delete-btn').addEventListener('click', function () {
        if (!confirm('Delete "' + c.name + '" and all its images?')) return;
        withLockHandling(requestJson('/api/admin/companion/characters/delete', {
          method: 'POST',
          body: JSON.stringify({ id: c.id }),
        })).then(loadCharacters).catch(function () {});
      });
      card.querySelector('.companion-image-form').addEventListener('submit', function (e) {
        e.preventDefault();
        var moodInput = card.querySelector('.mood-name');
        var fileInput = card.querySelector('.mood-file');
        var form = new FormData();
        form.append('id', c.id);
        form.append('mood', moodInput.value.trim());
        form.append('file', fileInput.files[0]);
        fetch('/api/admin/companion/characters/image', {
          method: 'POST',
          credentials: 'same-origin',
          headers: headers(),
          body: form,
        })
          .then(function (r) { return r.json().then(function (d) { return { ok: r.ok, data: d }; }); })
          .then(function (res) {
            if (!res.ok) {
              if (res.data.error === 'invalid_passphrase' || res.data.error === 'passphrase_not_configured') {
                setup = res.data.error === 'passphrase_not_configured';
                showLock();
                return;
              }
              alert(res.data.error || 'upload failed');
              return;
            }
            imgVersion++;
            loadCharacters();
          });
      });

      list.appendChild(card);
    });
  }

  function resetForm() {
    editingId = '';
    $('character-id').value = '';
    $('character-form').reset();
    $('character-price').value = 0;
    $('character-submit').textContent = 'Create character';
    $('character-cancel-edit').hidden = true;
    $('form-heading').textContent = 'New character';
  }

  $('character-form').addEventListener('submit', function (e) {
    e.preventDefault();
    status('character-status', 'Saving...');
    var payload = {
      name: $('character-name').value.trim(),
      personalityPrompt: $('character-prompt').value.trim(),
      priceCoins: Number($('character-price').value) || 0,
    };
    if (editingId) payload.id = editingId;
    withLockHandling(requestJson('/api/admin/companion/characters', { method: 'POST', body: JSON.stringify(payload) }))
      .then(function () {
        status('character-status', 'Saved.');
        resetForm();
        loadCharacters();
      })
      .catch(function (err) { status('character-status', err.message, true); });
  });
  $('character-cancel-edit').addEventListener('click', resetForm);

  $('passphrase-form').addEventListener('submit', function (e) {
    e.preventDefault();
    $('unlock-button').disabled = true;
    status('unlock-status', 'Checking…');
    var pass = $('admin-passphrase').value;
    fetch('/api/admin/passphrase-status', {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1', 'X-Admin-Passphrase': pass },
      body: JSON.stringify({ passphrase: pass }),
    })
      .then(function (r) { return r.json().then(function (d) { return { ok: r.ok, data: d }; }); })
      .then(function (res) {
        if (!res.ok) {
          throw new Error(res.data.error === 'invalid_passphrase' ? 'Incorrect passphrase.' : res.data.error || 'Could not unlock companion management.');
        }
        sessionStorage.setItem('admin_passphrase', pass);
        $('admin-passphrase').value = '';
        return loadCharacters();
      })
      .then(function () { status('unlock-status', ''); })
      .catch(function (error) { status('unlock-status', error.message, true); })
      .finally(function () { $('unlock-button').disabled = false; });
  });

  (function init() {
    requestJson('/api/me').then(function (me) {
      if (!me.isAdmin && !me.isOwner && !me.isCoOwner) {
        status('access-status', 'Admin access is required to manage companion characters.', true);
        return;
      }
      return requestJson('/api/admin/passphrase-status').then(function (config) {
        setup = !config.set;
        if (!setup && sessionStorage.getItem('admin_passphrase')) {
          return loadCharacters().catch(function (error) {
            if (error.code !== 'invalid_passphrase' && error.code !== 'passphrase_not_configured') throw error;
          });
        }
        showLock();
      });
    }).catch(function (error) { status('access-status', error.message, true); });
  })();
})();
