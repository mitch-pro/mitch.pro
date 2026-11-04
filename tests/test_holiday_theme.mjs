// Run: node tests/test_holiday_theme.mjs
// Checks the holiday date windows in webserver/site-theme.js: autumn colors from
// Nov 1 through Thanksgiving Day, winter colors the day after through Dec 26.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('../webserver/site-theme.js', import.meta.url), 'utf8');

function load(search = '', stored = {}) {
  const attrs = {};
  const root = {
    style: { setProperty() {}, removeProperty() {}, getPropertyValue: () => '' },
    dataset: {},
    classList: { toggle() {} },
    setAttribute: (k, v) => { attrs[k] = v; },
    removeAttribute: k => { delete attrs[k]; },
    getAttribute: k => attrs[k] ?? null
  };
  const sandbox = {
    document: { documentElement: root, cookie: '', head: { appendChild() {} }, createElement: () => ({}), querySelector: () => null },
    window: { dispatchEvent() {}, matchMedia: () => ({ matches: false }), addEventListener() {} },
    location: { search },
    localStorage: { getItem: k => stored[k] ?? null },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    CustomEvent: class {},
    Object, Math, Date, RegExp, parseInt, String
  };
  sandbox.window.document = sandbox.document;
  vm.createContext(sandbox);
  vm.runInContext(source, sandbox);
  return { theme: sandbox.window.__siteTheme, attrs };
}

const { theme } = load();
const day = (y, m, d) => theme.holidayFor(new Date(y, m - 1, d));

// Thanksgiving is the fourth Thursday of November.
assert.equal(day(2026, 10, 31), '');
assert.equal(day(2026, 11, 1), 'thanksgiving');
assert.equal(day(2026, 11, 26), 'thanksgiving'); // Thanksgiving 2026
assert.equal(day(2026, 11, 27), 'christmas');    // the day after
assert.equal(day(2026, 12, 25), 'christmas');
assert.equal(day(2026, 12, 26), 'christmas');
assert.equal(day(2026, 12, 27), '');
assert.equal(day(2027, 1, 15), '');
// Other years move with the calendar: Thanksgiving 2025 is Nov 27, 2027 is Nov 25.
assert.equal(day(2025, 11, 27), 'thanksgiving');
assert.equal(day(2025, 11, 28), 'christmas');
assert.equal(day(2027, 11, 25), 'thanksgiving');
assert.equal(day(2027, 11, 26), 'christmas');
// A November that starts on a Thursday has Thanksgiving on the 22nd (2029).
assert.equal(day(2029, 11, 22), 'thanksgiving');
assert.equal(day(2029, 11, 23), 'christmas');

// The page picks it up: preview parameter and the Preferences opt-out.
assert.equal(load('?holiday=christmas').attrs['data-holiday'], 'christmas');
assert.equal(load('?holiday=thanksgiving').attrs['data-holiday'], 'thanksgiving');
assert.equal(load('?holiday=none').attrs['data-holiday'], undefined);
assert.equal(load('', { theme_holiday: 'off' }).attrs['data-holiday'], undefined);

console.log('holiday theme: ok');
