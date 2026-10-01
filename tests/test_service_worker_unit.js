import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const worker = readFileSync('webserver/sw.js', 'utf8');

assert.match(worker, /if \(e\.request\.method !== 'GET'\) return;/,
  'Service worker must bypass POST and every other non-GET request before Cache Storage');
assert.match(worker, /if \(requestUrl\.origin !== self\.location\.origin\) return;/,
  'Service worker must bypass cross-origin requests so CORP failures are not returned by the worker');
assert.match(worker, /requestUrl\.pathname\.startsWith\('\/_matrix\/'\)/,
  'Service worker must bypass authenticated Matrix traffic');

const registrationFiles = [
  'webserver/app-shell.js',
  'webserver/broadcast.js',
  'webserver/encrypt/index.html',
  'webserver/index.html',
  'webserver/main.js',
  'webserver/matrix/index.html',
  'webserver/web-app/index.html',
  'webserver/sexypickleclub/cellar/index.html',
  'webserver/rjuhsd/index.html',
];

for (const file of registrationFiles) {
  const source = readFileSync(file, 'utf8');
  assert(source.includes('/sw.js?v=46'), `${file} must register the current service worker version`);
  assert(!/\/sw\.js\?v=(?:11|13|37|45)/.test(source), `${file} must not reinstall a stale service worker URL`);
}

console.log('Service worker bypass and version consistency checks passed.');
