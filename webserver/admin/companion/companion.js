(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };
  var editingId = '';

  function headers() {
    return { 'X-Mitch-Requested-With': '1' };
  }

  function requestJson(url, options) {
    options = options || {};
    var h = Object.assign({}, headers(), options.headers || {});
    if (options.body && typeof options.body === 'string') h['Content-Type'] = 'application/json';
    return fetch(url, Object.assign({ credentials: 'same-origin', cache: 'no-store' }, options, { headers: h }))
      .then(function (r) {
        return r.json().then(function (data) {
          if (!r.ok) throw new Error(data.error || 'request failed');
          return data;
        });
      });
  }

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function imageUrl(id, mood) {
    return '/api/companion/character-image?id=' + encodeURIComponent(id) + '&mood=' + encodeURIComponent(mood) + '&t=' + Date.now();
  }

  function loadCharacters() {
    requestJson('/api/admin/companion/characters').then(function (data) {
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
          requestJson('/api/admin/companion/characters/image/delete', {
            method: 'POST',
            body: JSON.stringify({ id: c.id, mood: mood }),
          }).then(loadCharacters);
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
        requestJson('/api/admin/companion/characters/set-default', {
          method: 'POST',
          body: JSON.stringify({ id: c.id }),
        }).then(loadCharacters);
      });
      var reviewedBtn = card.querySelector('.reviewed-btn');
      if (reviewedBtn) {
        reviewedBtn.addEventListener('click', function () {
          requestJson('/api/admin/companion/characters/mark-reviewed', {
            method: 'POST',
            body: JSON.stringify({ id: c.id }),
          }).then(loadCharacters);
        });
      }
      card.querySelector('.delete-btn').addEventListener('click', function () {
        if (!confirm('Delete "' + c.name + '" and all its images?')) return;
        requestJson('/api/admin/companion/characters/delete', {
          method: 'POST',
          body: JSON.stringify({ id: c.id }),
        }).then(loadCharacters);
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
          headers: { 'X-Mitch-Requested-With': '1' },
          body: form,
        })
          .then(function (r) { return r.json().then(function (d) { return { ok: r.ok, data: d }; }); })
          .then(function (res) {
            if (!res.ok) { alert(res.data.error || 'upload failed'); return; }
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
    var status = $('character-status');
    status.textContent = 'Saving...';
    var payload = {
      name: $('character-name').value.trim(),
      personalityPrompt: $('character-prompt').value.trim(),
      priceCoins: Number($('character-price').value) || 0,
    };
    if (editingId) payload.id = editingId;
    requestJson('/api/admin/companion/characters', { method: 'POST', body: JSON.stringify(payload) })
      .then(function () {
        status.textContent = 'Saved.';
        resetForm();
        loadCharacters();
      })
      .catch(function (err) { status.textContent = err.message; });
  });
  $('character-cancel-edit').addEventListener('click', resetForm);

  loadCharacters();
})();
