import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync('webserver/mitch-coins.js', 'utf8');
const children = [], listeners = {}, intervals = new Set();
let calls = 0, amount = 1250.25, responseStatus = 200, responseData = null;
const host = {
  classList: { add() {} },
  querySelector: selector => selector === '.mitch-wallet' ? children[0] : null,
  insertBefore: child => children.push(child)
};
const document = {
  hidden: false, readyState: 'complete',
  head: { appendChild() {} },
  body: { classList: { contains: () => false } },
  querySelector: selector => selector.startsWith('link') ? null : host,
  createElement: () => ({ dataset: {}, value: {}, attributes: {}, classList: { add() {} }, querySelector() { return this.value; }, setAttribute(name, value) { this.attributes[name] = value; } }),
  addEventListener: (name, fn) => { listeners[name] = fn; }
};
const nativeFetch = async () => { calls++; return new Response(JSON.stringify(responseData || { coins: amount }), { status: responseStatus }); };
const context = vm.createContext({
  document, location: { href: 'https://mitch.pro/', origin: 'https://mitch.pro', hostname: 'mitch.pro', pathname: '/' },
  URL, Intl, Date, AbortController, Response, Promise,
  fetch: nativeFetch,
  setTimeout, clearTimeout,
  setInterval: fn => { intervals.add(fn); return fn; }, clearInterval: fn => intervals.delete(fn),
  addEventListener: (name, fn) => { listeners[name] = fn; }
});
context.window = context;
vm.runInContext(source, context);
await context.MitchCoins.refresh();
assert.equal(children.length, 1);
assert.equal(children[0].value.textContent, '1,250.25');
assert.equal(children[0].dataset.state, 'ready');
assert(children[0].innerHTML.includes('/mitchcoin.svg'));
assert.equal(children[0].href, '/coins/?v=2');
vm.runInContext(source, context);
assert.equal(children.length, 1, 'Repeated loading must not duplicate wallets');
assert.equal(intervals.size, 1, 'Repeated loading must not duplicate polling');

const before = calls;
amount = 1234567.89;
await Promise.all([context.MitchCoins.refresh(), context.MitchCoins.refresh()]);
assert.equal(calls, before + 1, 'Concurrent refreshes must share a request');
assert.equal(children[0].value.textContent, '1.2M');
assert(children[0].title.includes('1,234,567.89'), 'Accessible balance must retain exact precision');
amount = 0;
await context.MitchCoins.refresh();
assert.equal(children[0].value.textContent, '0', 'A real zero is not an error');
responseStatus = 401;
await context.MitchCoins.refresh();
assert.equal(children[0].href, '/enroll/');
assert.equal(children[0].value.textContent, 'Sign in');
responseStatus = 200; amount = null; responseData = { authenticated: false, coins: null };
await context.MitchCoins.refresh();
assert.equal(children[0].href, '/enroll/');
assert.equal(children[0].value.textContent, 'Sign in', 'Public guest response must not render as a failed wallet');
responseData = null;
responseStatus = 500;
await context.MitchCoins.refresh();
assert.equal(children[0].value.textContent, '—', 'Failures must not invent a zero balance');
responseStatus = 200; amount = null;
await context.MitchCoins.refresh();
assert.equal(children[0].dataset.state, 'unavailable');
document.hidden = true;
const hiddenCalls = calls;
await context.MitchCoins.refresh();
assert.equal(calls, hiddenCalls, 'Hidden pages must not poll');
listeners.pagehide();
assert.equal(intervals.size, 0);
assert.equal(context.fetch, nativeFetch, 'Page teardown restores the original fetch');
listeners.pageshow({ persisted: true });
assert.equal(intervals.size, 1, 'Back navigation resumes exactly one poller');
listeners.pagehide();
console.log('MitchCoins: real balance, formatting, zero, guests, failures, duplicate loading, hidden pages, and lifecycle passed.');
