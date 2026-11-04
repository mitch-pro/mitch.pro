// Run: node tests/test_holiday_theme.mjs
// Checks the holiday date windows in webserver/site-theme.js: Halloween Oct 15-31,
// Thanksgiving Nov 1 through Thanksgiving Day, Christmas the day after through
// Dec 26, New Year's Dec 27 - Jan 3, Valentine's Feb 7-14.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('../webserver/site-theme.js', import.meta.url), 'utf8');

function load(search = '', stored = {}, pinned) {
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
    window: { dispatchEvent() {}, matchMedia: () => ({ matches: false }), addEventListener() {}, __holidayForce: pinned },
    location: { search },
    localStorage: { getItem: k => stored[k] ?? null, setItem: (k, v) => { stored[k] = String(v); }, removeItem: k => { delete stored[k]; } },
    getComputedStyle: () => ({ getPropertyValue: () => '' }),
    CustomEvent: class {},
    Object, Math, Date, RegExp, parseInt, String
  };
  sandbox.window.document = sandbox.document;
  vm.createContext(sandbox);
  vm.runInContext(source, sandbox);
  return { theme: sandbox.window.__siteTheme, attrs, stored };
}

const { theme } = load();
const day = (y, m, d) => theme.holidayFor(new Date(y, m - 1, d));

// Thanksgiving is the fourth Thursday of November.
// Halloween, New Year's and Valentine's windows.
assert.equal(day(2026, 10, 14), '');
assert.equal(day(2026, 10, 15), 'halloween');
assert.equal(day(2026, 10, 31), 'halloween');
assert.equal(day(2026, 12, 27), 'newyear');
assert.equal(day(2026, 12, 31), 'newyear');
assert.equal(day(2027, 1, 1), 'newyear');
assert.equal(day(2027, 1, 3), 'newyear');
assert.equal(day(2027, 1, 4), '');
assert.equal(day(2027, 2, 6), '');
assert.equal(day(2027, 2, 7), 'valentines');
assert.equal(day(2027, 2, 14), 'valentines');
assert.equal(day(2027, 2, 15), '');
assert.equal(day(2027, 7, 4), '');

assert.equal(day(2026, 11, 1), 'thanksgiving');
assert.equal(day(2026, 11, 26), 'thanksgiving'); // Thanksgiving 2026
assert.equal(day(2026, 11, 27), 'christmas');    // the day after
assert.equal(day(2026, 12, 25), 'christmas');
assert.equal(day(2026, 12, 26), 'christmas');
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
for (const name of ['halloween', 'newyear', 'valentines']) assert.equal(load('?holiday=' + name).attrs['data-holiday'], name);
assert.equal(load('?holiday=none').attrs['data-holiday'], undefined);
assert.equal(load('', { theme_holiday: 'off' }).attrs['data-holiday'], undefined);

// A server run can pin the season; the Preferences opt-out and ?holiday= still apply.
assert.equal(load('', {}, 'christmas').attrs['data-holiday'], 'christmas');
assert.equal(load('', {}, 'thanksgiving').theme.holidaySeason(), 'thanksgiving');
assert.equal(load('', {}, 'none').attrs['data-holiday'], undefined);
assert.equal(load('', { theme_holiday: 'off' }, 'christmas').attrs['data-holiday'], undefined);
assert.equal(load('?holiday=thanksgiving', {}, 'christmas').attrs['data-holiday'], 'thanksgiving');
assert.equal(load('', {}, 'bogus').theme.holidaySeason(), load().theme.holidaySeason());

// The homepage pill turns it off for every page by writing the same setting.
const toggled = load();
assert.equal(toggled.theme.holidayOff(), false);
toggled.theme.setHolidayOff(true);
assert.equal(toggled.stored.theme_holiday, 'off');
assert.equal(toggled.theme.holidayOff(), true);
assert.equal(toggled.attrs['data-holiday'], undefined);
toggled.theme.setHolidayOff(false);
assert.equal(toggled.stored.theme_holiday, undefined);
assert.equal(toggled.theme.holidayOff(), false);

console.log('holiday theme: ok');
