import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

const prefix = readFileSync('webserver/theme.js', 'utf8').split('  function clamp(')[0];
function fixture(hostname = 'mitch.pro', pathname = '/', initial = {}) {
  const values = new Map(Object.entries(initial));
  const cookies = new Map([['theme', 'light'], ['bgimg', 'effect%3Astarfield']]);
  const document = {
    get cookie() { return [...cookies].map(([k, v]) => k + '=' + v).join('; '); },
    set cookie(value) { const [key, data] = value.split(';')[0].split('='); cookies.set(key, data); }
  };
  const context = { document, location: { hostname, pathname }, localStorage: { getItem: k => values.get(k) ?? null, setItem: (k, v) => values.set(k, v) } };
  runInNewContext(prefix + 'globalThis.defaults = { applyBackgroundDefaults, applyVFXDefaults, preparePreferenceSnapshot };})();', context);
  return { ...context.defaults, values, cookies };
}
const old = fixture('mitch.pro', '/', { theme_bgimg: 'effect:starfield', theme_accent: '#ff0000', theme_adapt: 'off', theme_bgblur: '22', _prefHomepage: 'keep' });
old.applyBackgroundDefaults();
assert.equal(old.values.get('theme_bgimg'), '/backgrounds/wallhaven-black-mountain.webp');
assert.equal(old.values.get('theme_bgblur'), '8');
assert.equal(old.values.get('theme_accent'), '');
assert.equal(old.values.get('theme_adapt'), 'on');
assert.equal(old.values.get('_prefHomepage'), 'keep');
assert.equal(old.cookies.get('theme'), 'dark');
assert.equal(decodeURIComponent(old.cookies.get('bgimg')), '/backgrounds/wallhaven-black-mountain.webp');
old.values.set('theme_bgimg', 'effect:starfield'); old.values.set('theme_bgblur', '4');
old.applyBackgroundDefaults();
assert.equal(old.values.get('theme_bgimg'), 'effect:starfield');
assert.equal(old.values.get('theme_bgblur'), '4');
const migrated = old.preparePreferenceSnapshot({ theme_bgimg: 'old.webp', theme_bgblur: '0', _prefHomepage: 'keep' });
assert.equal(migrated.theme_bgblur, '8');
assert.equal(migrated.theme_bgimg, '/backgrounds/wallhaven-black-mountain.webp');
assert.equal(migrated._prefHomepage, 'keep');
const current = { theme_backgroundDefaults: 'mountain-2026-09-06', theme_vfxDefaults: 'all-on-2026-09-06', theme_bgimg: 'effect:starfield', theme_bgblur: '3' };
assert.equal(old.preparePreferenceSnapshot(current), current);
old.values.set('_prefVFX', JSON.stringify({ snow: false, stars: false, rain: false, particles: false }));
old.applyVFXDefaults();
assert.deepEqual(JSON.parse(old.values.get('_prefVFX')), { snow: true, stars: true, rain: true, particles: true });
old.values.set('_prefVFX', JSON.stringify({ snow: false, stars: false, rain: false, particles: false }));
old.applyVFXDefaults();
assert.equal(JSON.parse(old.values.get('_prefVFX')).rain, false);
assert.equal(migrated._prefVFX.rain, true);
assert.equal(old.preparePreferenceSnapshot({ ...current, _prefVFX: { rain: false } })._prefVFX.rain, false);
for (const [host, path] of [['rjuhsd.school', '/'], ['woodcreek.rjuhsd.school', '/'], ['mitch.pro', '/rjuhsd/']]) {
  const school = fixture(host, path);
  school.applyBackgroundDefaults();
  assert.equal(school.values.has('theme_backgroundDefaults'), false);
  assert.equal(school.cookies.get('theme'), 'light');
  assert.equal(school.values.get('theme_bgimg'), '/backgrounds/wallhaven-black-mountain.webp');
  assert.equal(school.values.get('theme_bgblur'), '8');
  assert.equal(decodeURIComponent(school.cookies.get('bgimg')), '/backgrounds/wallhaven-black-mountain.webp');
  const snap = { theme_bgimg: 'school.webp' };
  assert.equal(school.preparePreferenceSnapshot(snap), snap);
  assert.equal(school.preparePreferenceSnapshot({}).theme_bgimg, '/backgrounds/wallhaven-black-mountain.webp');
}
console.log('Background defaults: rollout, blur, colors, cookie, later choices, account restore, and school wallpaper passed.');
