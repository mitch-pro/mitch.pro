(function () {
  'use strict';
  var $ = function (id) { return document.getElementById(id); };

  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c];
    });
  }

  function links(status) {
    var adminOk = status.isAdmin;
    var modOk = status.isAdmin || status.isModerator;
    var testerOk = status.isTester;
    var list = [
      { href: '/admin/', name: 'Admin dashboard', desc: 'Approvals, people, economy, moderation, VM cloud, logs', ok: adminOk },
      { href: '/admin/bans/', name: 'Account bans', desc: 'Review bans and restore accounts', ok: adminOk },
      { href: '/admin/updates/', name: 'Updates', desc: 'Write the homepage update log', ok: adminOk },
      { href: '/admin/vms/', name: 'Virtual machines', desc: 'Manage computers and resources', ok: adminOk },
      { href: '/admin/email/', name: 'Email & newsletters', desc: 'Compose and send site email', ok: adminOk },
      { href: '/admin/companion/', name: 'Companion characters', desc: 'Preset characters and review queue', ok: adminOk },
      { href: '/moderator/', name: 'Moderator panel', desc: 'Reports and content review', ok: modOk },
      { href: '/moderator/feedback/', name: 'Feedback', desc: 'Read what people send from the feedback page', ok: modOk },
      { href: '/moderator/teachers/', name: 'Teacher ratings', desc: 'Review teacher notes and manage the teacher list', ok: modOk },
      { href: '/moderator/support/', name: 'Support tickets', desc: 'Help people who are locked out; request email changes', ok: modOk },
      { href: '/tester/', name: 'Beta tester hub', desc: 'Tester tools and cooldown bypasses', ok: testerOk },
    ];
    // Owner-only: added only for the owner, never shown as a locked card, so
    // other staff can't tell it exists.
    if (status.isOwner) {
      list.push({ href: '/vms/desktop/?container=owner', name: 'Container 500', desc: 'Owner-only desktop', ok: true });
    }
    return list;
  }

  function render(status) {
    var container = $('staff-links');
    container.innerHTML = '';
    links(status).forEach(function (l) {
      var card = document.createElement(l.ok ? 'a' : 'div');
      card.className = 'staff-link-card';
      if (l.ok) {
        card.href = l.href;
      } else {
        card.setAttribute('data-locked', '');
      }
      card.innerHTML = '<h3>' + escapeHtml(l.name) + '</h3><p>' + escapeHtml(l.desc) + '</p>';
      container.appendChild(card);
    });
    container.hidden = false;
  }

  function load() {
  $('staff-status').textContent = 'Loading...';
  fetch('/api/staff-hub/status', { credentials: 'same-origin', cache: 'no-store' })
    .then(function (r) {
      if (r.status === 401) {
        location.href = '/enroll/?next=' + encodeURIComponent('/staff/');
        return null;
      }
      if (!r.ok) throw new Error('status ' + r.status);
      return r.json();
    })
    .then(function (status) {
      if (!status) return;
      var el = $('staff-status');
      if (!status.isStaff) {
        el.textContent = 'Staff access required.';
        return;
      }
      el.textContent = '';
      render(status);
    })
    .catch(function () {
      var el = $('staff-status');
      el.textContent = 'Could not load staff status. ';
      var retry = document.createElement('button');
      retry.type = 'button';
      retry.textContent = 'Retry';
      retry.addEventListener('click', load);
      el.appendChild(retry);
    });
  }
  load();
})();
