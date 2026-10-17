import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

// Exercise the shipped controller without a network, account, or browser process.
// Layout and CSS still require a real desktop/mobile browser review.
const code = readFileSync(new URL('../webserver/leaderboard/leaderboard.js', import.meta.url), 'utf8');
const profile = {
  name: 'Nova <script>alert(1)</script>', handle: 'nova', profileId: 'nova', rank: 1, coins: 12500,
  wins: 12, puzzles: 8, pfp: 'data:image/png;base64,aGk=', background: 'https://example.test/cover.jpg',
  activeColor: 'gold_glow', activeBadge: 'artist_badge', activeProfileEffect: 'profile_neon_frame', isPremium: true,
};
const hero = { name: 'Hero', handle: 'hero', profileId: 'hero', rank: 147, coins: 125, wins: 80, isMe: true };
const players = [profile, { name: 'Milo', handle: 'milo', profileId: 'milo', rank: 2, coins: 9000 }, { name: 'Aria', handle: 'aria', profileId: 'aria', rank: 3, coins: 8000 }];
const tick = () => new Promise(resolve => setImmediate(resolve));

function boot(initialMode = 'success') {
  const elements = new Map(), timers = new Map(), intervals = [], requests = [];
  let timerId = 0, mode = initialMode, delayNext = false, delayed = null;
  class Element {
    constructor(id, dataset = {}) { this.id = id; this.dataset = dataset; this.attributes = new Map(); this.events = new Map(); this.value = ''; this.hidden = false; this.textContent = ''; this.classes = new Set(); this.classList = { add: c => this.classes.add(c), remove: c => this.classes.delete(c) }; }
    set innerHTML(value) {
      this.html = value;
      for (const match of value.matchAll(/id="([^"]+)"/g)) elements.set(match[1], new Element(match[1]));
      if (this.id === 'metric-tabs') this.tabs = [...value.matchAll(/data-metric="([^"]+)" aria-pressed="([^"]+)"/g)].map(([, metric, pressed]) => { const tab = new Element(metric, { metric }); tab.setAttribute('aria-pressed', pressed); return tab; });
    }
    get innerHTML() { return this.html || ''; }
    setAttribute(key, value) { this.attributes.set(key, value); }
    getAttribute(key) { return this.attributes.get(key); }
    addEventListener(type, handler) { this.events.set(type, handler); }
    closest() { return this.dataset.metric ? this : null; }
    focus() { this.focused = true; }
    emit(type, target = this) { this.events.get(type)?.({ target }); }
  }
  for (const id of ['metric-tabs', 'rankings-title', 'my-rank', 'standings-head', 'rank-list', 'rank-count', 'board-notice', 'board-updated', 'board-status', 'board-content', 'refresh-board', 'rank-search']) elements.set(id, new Element(id));
  const document = { getElementById: id => elements.get(id) || null, querySelectorAll: () => elements.get('metric-tabs').tabs, hidden: false };
  const fetch = async (url, options) => {
    requests.push({ url, options });
    const params = new URL(url, 'http://localhost').searchParams;
    if (delayNext) {
      delayNext = false;
      await new Promise(resolve => { delayed = resolve; });
    }
    if (mode === 'error') return { ok: false, status: 503 };
    let result = { metric: params.get('metric'), players, top: players, me: hero, total: 147, matchCount: 147 };
    if (params.get('metric') === 'wins') result = { ...result, players: [{ ...hero, rank: 1 }], top: [{ ...hero, rank: 1 }], me: { ...hero, rank: 1 } };
    if (params.get('q')) result = { ...result, players: params.get('q') === 'hero' ? [hero] : [], matchCount: params.get('q') === 'hero' ? 1 : 0 };
    if (mode === 'tester') result = { ...result, isTester: true, me: null };
    if (mode === 'empty') result = { ...result, players: [], top: [], me: null, total: 0, matchCount: 0 };
    if (mode === 'unsafe') result = { ...result, players: [{ ...profile, pfp: 'javascript:alert(1)', background: 'data:text/html;base64,aGk=', activeColor: '__proto__', activeProfileEffect: 'constructor', activeBadge: 'constructor' }] };
    return { ok: true, json: async () => result };
  };
  runInNewContext(code, {
    document, fetch, URL, URLSearchParams, AbortController, HTMLImageElement: class {},
    setTimeout: fn => { timers.set(++timerId, fn); return timerId; }, clearTimeout: id => timers.delete(id),
    setInterval: fn => intervals.push(fn),
  });
  return {
    get: id => elements.get(id), requests, document, intervals,
    mode: value => { mode = value; }, delay: () => { delayNext = true; }, release: () => delayed?.(),
    search: value => { const input = elements.get('rank-search'); input.value = value; input.emit('input'); },
    debounce: () => { const callbacks = [...timers.values()]; timers.clear(); callbacks.forEach(fn => fn()); },
    metric: key => { const tabs = elements.get('metric-tabs'); tabs.emit('click', tabs.tabs.find(tab => tab.dataset.metric === key)); },
    refresh: () => elements.get('refresh-board').emit('click'),
  };
}

const app = boot();
await tick();
assert.match(app.get('rank-list').innerHTML, /data:image\/png;base64,aGk=/);
assert.match(app.get('rank-list').innerHTML, /https:\/\/example.test\/cover.jpg/);
assert.match(app.get('rank-list').innerHTML, /data-color="gold_glow"/);
assert.match(app.get('rank-list').innerHTML, /data-effect="profile_neon_frame"/);
assert.match(app.get('rank-list').innerHTML, /Artist Badge/);
assert.match(app.get('rank-list').innerHTML, /Premium member/);
assert.match(app.get('rank-list').innerHTML, /Nova &lt;script&gt;/);
assert.ok(!app.get('rank-list').innerHTML.includes('<script>alert'));
assert.match(app.get('rank-list').innerHTML, /Neon frame/);
assert.match(app.get('my-rank').innerHTML, /#147/);
assert.equal(app.get('board-content').getAttribute('aria-busy'), 'false');
for (const id of ['my-rank', 'rank-list']) {
  const html = app.get(id).innerHTML;
  assert.equal((html.match(/<span(?:\s|>)/g) || []).length, (html.match(/<\/span>/g) || []).length, `${id} must have balanced profile markup`);
}

app.search('hero'); app.debounce(); await tick();
assert.match(app.requests.at(-1).url, /q=hero/);
assert.match(app.get('rank-list').innerHTML, /aria-label="Rank 147"/);
assert.match(app.get('rank-list').innerHTML, /player-you/);
assert.match(app.get('rank-count').textContent, /1 match/);
app.search('nobody'); app.debounce(); await tick();
assert.match(app.get('rank-list').innerHTML, /No players found/);
app.get('clear-search').emit('click'); await tick();
assert.equal(app.get('rank-search').value, '');

app.metric('wins'); await tick();
assert.match(app.requests.at(-1).url, /metric=wins/);
assert.match(app.get('my-rank').innerHTML, /#1</);
assert.equal(app.get('rankings-title').textContent, 'Chess wins rankings');
assert.equal(app.get('metric-tabs').tabs.find(tab => tab.dataset.metric === 'wins').getAttribute('aria-pressed'), 'true');

// Late responses must not overwrite newer searches or categories.
app.delay(); app.metric('pixels');
app.metric('wins'); await tick();
app.release(); await tick();
assert.equal(app.get('rankings-title').textContent, 'Chess wins rankings');
app.search('hero'); app.debounce(); await tick();
const beforeAutoRefresh = app.requests.length;
app.intervals[0](); await tick();
assert.equal(app.requests.length, beforeAutoRefresh + 1, 'Auto refresh must still run after a debounced search');
app.document.hidden = true; app.intervals[0](); await tick();
assert.equal(app.requests.length, beforeAutoRefresh + 1, 'Hidden pages must not refresh');

app.mode('error'); app.refresh(); await tick();
assert.match(app.get('board-updated').textContent, /previous results/);
assert.match(app.get('rank-list').innerHTML, /Hero/);
assert.equal(app.get('board-content').getAttribute('aria-busy'), 'false');
app.mode('tester'); app.refresh(); await tick();
assert.match(app.get('my-rank').innerHTML, /Test accounts are excluded/);
app.mode('empty'); app.search(''); app.debounce(); await tick();
assert.match(app.get('rank-list').innerHTML, /No rankings yet/);
assert.equal(app.get('my-rank').hidden, true);
app.mode('unsafe'); app.search(''); app.debounce(); await tick();
assert.ok(!app.get('rank-list').innerHTML.includes('javascript:'));
assert.ok(!app.get('rank-list').innerHTML.includes('data:text/html'));
assert.ok(!app.get('rank-list').innerHTML.includes('data-color="__proto__"'));
assert.ok(!app.get('rank-list').innerHTML.includes('data-effect="constructor"'));

const failed = boot('error'); await tick();
assert.match(failed.get('rank-list').innerHTML, /Try again/);
failed.mode('success'); failed.get('retry-board').emit('click'); await tick();
assert.match(failed.get('rank-list').innerHTML, /Nova &lt;script&gt;/);
console.log('Leaderboard UI checks passed: profiles, cosmetics, ranks, categories, search, races, refresh, empty/error/tester states, and unsafe input.');
