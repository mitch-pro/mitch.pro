/* "Send playlist to a person" prompt (docs/music.md). The music page's Share
 * button opens /matrix/?mshare=<playlist link>&mname=<name>. Once the chat
 * client has synced, the user picks one person (people they already message,
 * or a directory search), reviews and can edit the message, and sends it as a
 * direct message: an existing DM with that person if there is one, otherwise a
 * new private, encrypted DM. It never posts to a room or channel. Sending uses
 * the client's own sendTextMessage, so encryption is handled by the client.
 * It touches nothing in the chat UI: the prompt is its own overlay. */
(function () {
  'use strict';
  var params = new URLSearchParams(location.search);
  var link = params.get('mshare');
  if (!link) return;
  var name = (params.get('mname') || 'Playlist').slice(0, 80);

  // Only a playlist link on this site can be sent.
  var url;
  try { url = new URL(link, location.origin); } catch (e) { return; }
  if (url.origin !== location.origin || url.pathname !== '/music/' || !url.searchParams.get('playlist')) return;
  link = url.href;

  try {
    params.delete('mshare'); params.delete('mname');
    var rest = params.toString();
    history.replaceState(null, '', location.pathname + (rest ? '?' + rest : '') + location.hash);
  } catch (e) { /* ignore */ }

  var css = document.createElement('style');
  css.textContent =
    '.mshare-back{position:fixed;inset:0;z-index:2147483000;display:grid;place-items:center;background:rgba(0,0,0,.5);font-family:var(--t-font,"DM Sans",system-ui,sans-serif)}' +
    '.mshare{width:min(420px,calc(100vw - 32px));max-height:80vh;display:flex;flex-direction:column;box-sizing:border-box;padding:16px;background:var(--t-bg2,#1b1b24);color:var(--t-fg,#e8e6e3);border:1px solid var(--t-bd,rgba(255,255,255,.14));border-radius:var(--t-radius,10px)}' +
    '.mshare h2{margin:0 0 2px;font-size:18px;font-weight:700;letter-spacing:-.02em}' +
    '.mshare p{margin:0 0 10px;color:var(--t-fg2,#a8a6a3);font-size:13px;overflow-wrap:anywhere}' +
    '.mshare input{width:100%;box-sizing:border-box;padding:9px 11px;margin-bottom:8px;background:var(--t-bg,#101016);color:inherit;border:1px solid var(--t-bd,rgba(255,255,255,.14));border-radius:var(--t-radius-sm,7px);font:inherit}' +
    '.mshare ul{list-style:none;margin:0 0 10px;padding:0;overflow-y:auto;min-height:60px}' +
    '.mshare li button{display:block;width:100%;padding:9px 10px;text-align:left;background:none;color:inherit;border:0;border-radius:var(--t-radius-sm,7px);font:inherit;cursor:pointer}' +
    '.mshare li button:hover,.mshare li button:focus-visible{background:var(--t-bg3,rgba(255,255,255,.08));outline:none}' +
    '.mshare .mshare-empty{padding:12px 10px;color:var(--t-fg2,#a8a6a3)}' +
    '.mshare textarea{width:100%;box-sizing:border-box;min-height:84px;padding:9px 11px;margin-bottom:10px;background:var(--t-bg,#101016);color:inherit;border:1px solid var(--t-bd,rgba(255,255,255,.14));border-radius:var(--t-radius-sm,7px);font:inherit;resize:vertical}' +
    '.mshare-foot{display:flex;justify-content:flex-end;gap:8px}' +
    '.mshare-foot .mshare-primary{background:var(--t-ac,#a78bfa);color:var(--t-ac-ink,#111);border-color:transparent}' +
    '.mshare-foot button{padding:8px 14px;background:var(--t-bg3,rgba(255,255,255,.08));color:inherit;border:1px solid var(--t-bd,rgba(255,255,255,.14));border-radius:var(--t-radius-sm,7px);font:inherit;font-weight:600;cursor:pointer}';
  document.head.appendChild(css);

  var back = document.createElement('div');
  back.className = 'mshare-back';
  var box = document.createElement('div');
  box.className = 'mshare';
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-modal', 'true');
  box.setAttribute('aria-label', 'Send playlist to a person');
  var h = document.createElement('h2'); h.textContent = 'Send playlist to a person';
  var sub = document.createElement('p'); sub.textContent = name;
  var search = document.createElement('input');
  search.type = 'search'; search.placeholder = 'Find a person'; search.setAttribute('aria-label', 'Find a person');
  var list = document.createElement('ul');
  var draft = document.createElement('textarea');
  draft.maxLength = 1000; draft.hidden = true; draft.setAttribute('aria-label', 'Message');
  var foot = document.createElement('div'); foot.className = 'mshare-foot';
  var backBtn = document.createElement('button'); backBtn.type = 'button'; backBtn.textContent = 'Back'; backBtn.hidden = true;
  var sendBtn = document.createElement('button'); sendBtn.type = 'button'; sendBtn.textContent = 'Send'; sendBtn.className = 'mshare-primary'; sendBtn.hidden = true;
  var close = document.createElement('button'); close.type = 'button'; close.textContent = 'Close';
  foot.append(backBtn, sendBtn, close);
  box.append(h, sub, search, list, draft, foot);
  back.appendChild(box);

  function dismiss() { back.remove(); css.remove(); }
  close.addEventListener('click', dismiss);
  back.addEventListener('click', function (e) { if (e.target === back) dismiss(); });
  document.addEventListener('keydown', function esc(e) { if (e.key === 'Escape') { dismiss(); document.removeEventListener('keydown', esc); } });

  function note(text) {
    list.replaceChildren();
    var li = document.createElement('li');
    li.className = 'mshare-empty';
    li.textContent = text;
    list.appendChild(li);
  }

  /** userId -> [roomId] of the user's existing direct chats (m.direct). */
  function directMap(c) {
    var ev = c.getAccountData && c.getAccountData('m.direct');
    var map = ev && ev.getContent ? ev.getContent() : {};
    return map && typeof map === 'object' ? map : {};
  }

  function shownName(c, userId, fallback) {
    var u = c.getUser && c.getUser(userId);
    return fallback || (u && u.displayName) || userId;
  }

  function contacts(c) {
    var map = directMap(c);
    return Object.keys(map).filter(function (id) { return id !== c.getUserId(); }).map(function (id) {
      return { userId: id, name: shownName(c, id) };
    }).sort(function (a, b) { return a.name.localeCompare(b.name); });
  }

  function show(c) {
    var timer = null, token = 0;
    function draw(people, empty) {
      list.replaceChildren();
      if (!people.length) return note(empty);
      people.slice(0, 50).forEach(function (p) {
        var li = document.createElement('li');
        var b = document.createElement('button');
        b.type = 'button';
        b.textContent = p.name === p.userId ? p.userId : p.name + '  ' + p.userId;
        b.addEventListener('click', function () { compose(c, p); });
        li.appendChild(b);
        list.appendChild(li);
      });
    }
    function refresh() {
      var q = search.value.trim();
      var mine = contacts(c);
      if (q.length < 2) return draw(mine, 'Search for a person by name');
      var lower = q.toLowerCase();
      draw(mine.filter(function (p) { return p.name.toLowerCase().indexOf(lower) !== -1 || p.userId.toLowerCase().indexOf(lower) !== -1; }), 'Searching…');
      var mine_ = ++token;
      Promise.resolve(c.searchUserDirectory({ term: q, limit: 20 })).then(function (res) {
        if (mine_ !== token) return;
        var seen = {}, out = [];
        mine.concat((res.results || []).map(function (r) { return { userId: r.user_id, name: shownName(c, r.user_id, r.display_name) }; })).forEach(function (p) {
          if (p.userId === c.getUserId() || seen[p.userId]) return;
          if (p.name.toLowerCase().indexOf(lower) === -1 && p.userId.toLowerCase().indexOf(lower) === -1) return;
          seen[p.userId] = true; out.push(p);
        });
        draw(out, 'No one found');
      }, function () { if (mine_ === token) note("Couldn't search. Try again."); });
    }
    search.addEventListener('input', function () { clearTimeout(timer); timer = setTimeout(refresh, 250); });
    backBtn.addEventListener('click', function () { step('pick'); });
    step('pick');
    refresh();
    search.focus();
  }

  function step(which) {
    var pick = which === 'pick';
    search.hidden = !pick; list.hidden = !pick;
    draft.hidden = pick; backBtn.hidden = pick; sendBtn.hidden = pick;
    sub.textContent = pick ? name : sub.textContent;
    if (pick) search.focus();
  }

  var target = null;
  function compose(c, person) {
    target = person;
    sub.textContent = 'To ' + person.name;
    draft.value = '\uD83C\uDFB5 ' + name + ' ' + link;
    step('draft');
    sendBtn.disabled = false;
    sendBtn.textContent = 'Send';
    sendBtn.onclick = function () { send(c, person); };
    draft.focus();
  }

  /** An existing direct chat with `userId` that we are in, if any. */
  function existingDm(c, userId) {
    var ids = directMap(c)[userId] || [];
    for (var i = 0; i < ids.length; i++) {
      var room = c.getRoom && c.getRoom(ids[i]);
      if (room && room.getMyMembership() === 'join') return ids[i];
    }
    return null;
  }

  /** A new private, encrypted DM, recorded in m.direct so the chat lists it. */
  function createDm(c, userId) {
    return Promise.resolve(c.createRoom({
      is_direct: true, invite: [userId], visibility: 'private', preset: 'trusted_private_chat',
      initial_state: [{ type: 'm.room.encryption', state_key: '', content: { algorithm: 'm.megolm.v1.aes-sha2' } }]
    })).then(function (res) {
      var map = Object.assign({}, directMap(c));
      map[userId] = (map[userId] || []).concat(res.room_id);
      return Promise.resolve(c.setAccountData('m.direct', map)).then(function () { return res.room_id; });
    });
  }

  function send(c, person) {
    var text = draft.value.trim();
    if (!text) return;
    sendBtn.disabled = true;
    sendBtn.textContent = 'Sending…';
    var room = existingDm(c, person.userId);
    Promise.resolve(room || createDm(c, person.userId)).then(function (roomId) {
      return c.sendTextMessage(roomId, text);
    }).then(function () {
      sub.textContent = 'Sent to ' + person.name;
      draft.hidden = true; sendBtn.hidden = true; backBtn.hidden = true;
      close.textContent = 'Done';
    }, function () {
      sendBtn.disabled = false;
      sendBtn.textContent = 'Send';
      sub.textContent = "Couldn't send to " + person.name + '. Try again.';
    });
  }

  function start() {
    document.body.appendChild(back);
    note('Loading chat…');
    var tries = 0;
    var timer = setInterval(function () {
      var c = window.__mxClient;
      var state = c && c.getSyncState && c.getSyncState();
      if (state === 'PREPARED' || state === 'SYNCING') { clearInterval(timer); show(c); }
      else if (++tries > 200) { clearInterval(timer); note('Chat is not ready. Open it and try again.'); }
    }, 300);
  }
  if (document.body) start(); else document.addEventListener('DOMContentLoaded', start);
})();
