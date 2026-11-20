/* "Send playlist to chat" prompt (docs/music.md). The music page's Share
 * button opens /matrix/?mshare=<playlist link>&mname=<name>. Once the chat
 * client has synced, this lists the signed-in user's rooms and sends the link
 * as a normal message with the client's own sendTextMessage, so encrypted
 * rooms stay encrypted. It touches nothing in the chat UI: the prompt is its
 * own overlay. */
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
    '.mshare-foot{display:flex;justify-content:flex-end}' +
    '.mshare-foot button{padding:8px 14px;background:var(--t-bg3,rgba(255,255,255,.08));color:inherit;border:1px solid var(--t-bd,rgba(255,255,255,.14));border-radius:var(--t-radius-sm,7px);font:inherit;font-weight:600;cursor:pointer}';
  document.head.appendChild(css);

  var back = document.createElement('div');
  back.className = 'mshare-back';
  var box = document.createElement('div');
  box.className = 'mshare';
  box.setAttribute('role', 'dialog');
  box.setAttribute('aria-modal', 'true');
  box.setAttribute('aria-label', 'Send playlist to chat');
  var h = document.createElement('h2'); h.textContent = 'Send playlist to chat';
  var sub = document.createElement('p'); sub.textContent = name;
  var search = document.createElement('input');
  search.type = 'search'; search.placeholder = 'Find a chat'; search.setAttribute('aria-label', 'Find a chat');
  var list = document.createElement('ul');
  var foot = document.createElement('div'); foot.className = 'mshare-foot';
  var close = document.createElement('button'); close.type = 'button'; close.textContent = 'Close';
  foot.appendChild(close);
  box.append(h, sub, search, list, foot);
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

  function rooms(c) {
    return c.getRooms().filter(function (r) {
      return r.getMyMembership() === 'join' && !(r.isSpaceRoom && r.isSpaceRoom());
    }).sort(function (a, b) { return (b.getLastActiveTimestamp() || 0) - (a.getLastActiveTimestamp() || 0); });
  }

  function show(c) {
    function draw() {
      var q = search.value.trim().toLowerCase();
      list.replaceChildren();
      var shown = rooms(c).filter(function (r) { return !q || (r.name || '').toLowerCase().indexOf(q) !== -1; });
      if (!shown.length) return note('No chats found');
      shown.slice(0, 100).forEach(function (r) {
        var li = document.createElement('li');
        var b = document.createElement('button');
        b.type = 'button';
        b.textContent = r.name || r.roomId;
        b.addEventListener('click', function () { send(c, r, b); });
        li.appendChild(b);
        list.appendChild(li);
      });
    }
    search.addEventListener('input', draw);
    draw();
    search.focus();
  }

  function send(c, room, b) {
    b.disabled = true;
    b.textContent = 'Sending…';
    Promise.resolve(c.sendTextMessage(room.roomId, '🎵 ' + name + ' ' + link)).then(function () {
      sub.textContent = 'Sent to ' + (room.name || 'chat');
      list.replaceChildren();
      search.hidden = true;
      close.textContent = 'Done';
    }, function () {
      b.disabled = false;
      b.textContent = room.name || room.roomId;
      sub.textContent = "Couldn't send. Try again.";
    });
  }

  function start() {
    document.body.appendChild(back);
    note('Loading chats…');
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
