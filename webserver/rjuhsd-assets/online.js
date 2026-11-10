// Live visitor count for rjuhsd.school. Each ping carries a proof of work the
// server checks (see docs/rjuhsd-online.md): find n so that
// SHA-256(token:vid:n) has `bits` leading zero bits.
(function () {
  'use strict';
  var host = location.hostname;
  if (!/(^|\.)rjuhsd\.school$/.test(host) && !/^(localhost|127\.0\.0\.1)$/.test(host)) return;
  // The footer counter (#site-active). site-status.js leaves it alone on this
  // page because of the data-visitor-count attribute on <html>.
  var el = document.getElementById('site-active');
  if (!el || !window.crypto || !crypto.subtle) return;

  var API = '/api/rjuhsd/online';
  var PING_MS = 30000;
  var enc = new TextEncoder();
  var vid = '';
  try { vid = sessionStorage.getItem('rjuhsd-online-id') || ''; } catch (e) {}
  if (!/^[A-Za-z0-9_-]{16,40}$/.test(vid)) {
    var raw = crypto.getRandomValues(new Uint8Array(18));
    vid = Array.prototype.map.call(raw, function (b) { return (b % 36).toString(36); }).join('') + 'xx';
    try { sessionStorage.setItem('rjuhsd-online-id', vid); } catch (e) {}
  }

  function leadingZeroBits(bytes) {
    var bits = 0;
    for (var i = 0; i < bytes.length; i++) {
      if (bytes[i] === 0) { bits += 8; continue; }
      bits += Math.clz32(bytes[i]) - 24;
      break;
    }
    return bits;
  }

  async function solve(challenge) {
    var prefix = challenge.token + ':' + vid + ':';
    var BATCH = 512;
    for (var start = 0; ; start += BATCH) {
      var tries = [];
      for (var i = 0; i < BATCH; i++) {
        tries.push(crypto.subtle.digest('SHA-256', enc.encode(prefix + (start + i))));
      }
      var digests = await Promise.all(tries);
      for (var j = 0; j < BATCH; j++) {
        if (leadingZeroBits(new Uint8Array(digests[j])) >= challenge.bits) return String(start + j);
      }
    }
  }

  function show(count) {
    if (typeof count !== 'number') return;
    el.textContent = count.toLocaleString() + ' active';
    el.title = 'People on rjuhsd.school right now';
    el.hidden = false;
  }

  var challenge = null;
  var timer = 0;

  function schedule(ms) {
    clearTimeout(timer);
    timer = setTimeout(tick, ms);
  }

  async function tick() {
    if (document.hidden) return; // resumes on visibilitychange
    try {
      if (!challenge) {
        var first = await fetch(API, { cache: 'no-store', credentials: 'omit' });
        if (!first.ok) throw new Error('challenge');
        var data = await first.json();
        show(data.count);
        challenge = data.challenge;
      }
      var n = await solve(challenge);
      var res = await fetch(API, {
        method: 'POST',
        cache: 'no-store',
        credentials: 'omit',
        headers: { 'Content-Type': 'application/json', 'X-Mitch-Requested-With': '1' },
        body: JSON.stringify({ vid: vid, token: challenge.token, n: n })
      });
      challenge = null;
      if (!res.ok) throw new Error('ping');
      var out = await res.json();
      show(out.count);
      challenge = out.challenge || null;
      schedule(PING_MS);
    } catch (e) {
      challenge = null;
      schedule(PING_MS * 2);
    }
  }

  document.addEventListener('visibilitychange', function () {
    if (!document.hidden) schedule(500);
  });
  tick();
})();
