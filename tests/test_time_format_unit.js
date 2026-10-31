import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

for (const file of ['webserver/sexypickleclub/cellar/index.html']) {
  const source = readFileSync(file, 'utf8');
  const context = vm.createContext({ Date });
  vm.runInContext(source.split('\n').find(line => line.startsWith('function fmtTime(ts)')), context);
  for (const [hour, expected] of [[0, '12:05 AM'], [12, '12:05 PM'], [15, '03:05 PM'], [23, '11:05 PM']]) {
    assert.equal(context.fmtTime(new Date(2026, 8, 7, hour, 5).getTime()), expected);
  }
}
const home = readFileSync('webserver/index.html', 'utf8');
assert(!home.includes("_clockFmt === '24'"), 'Old homepage preferences must not restore military time');
const tick = home.slice(home.indexOf('  function tick() {'), home.indexOf('    if (gamesDayEl) {')) + '\n}';
for (const [hour, expected] of [[0, '12:05 AM'], [12, '12:05 PM'], [15, '3:05 PM']]) {
  const clockEl = {};
  const context = vm.createContext({ clockEl, greetEl: null, Date: class { getHours() { return hour; } getMinutes() { return 5; } } });
  vm.runInContext(tick + '\ntick();', context);
  assert.equal(clockEl.textContent, expected);
}
const school = readFileSync('webserver/rjuhsd-assets/app.js', 'utf8');
const schoolContext = vm.createContext({});
vm.runInContext(school.split('\n').find(line => line.startsWith('function time(v)')), schoolContext);
assert.equal(schoolContext.time('00:05'), '12:05 AM');
assert.equal(schoolContext.time('12:05'), '12:05 PM');
assert.equal(schoolContext.time('23:05'), '11:05 PM');
assert(!readFileSync('webserver/preferences/index.html', 'utf8').includes('data-clock="24"'));
for (const file of ['webserver/team/index.html', 'webserver/admin/index.html']) {
  const lines = readFileSync(file, 'utf8').split('\n').filter(line => line.includes('toLocaleTimeString'));
  assert(lines.every(line => /hour12\s*:\s*true/.test(line)), file + ' must explicitly use AM/PM');
}
console.log('US time: chat, homepage, midnight/noon, school times, and old 24-hour preferences passed.');
