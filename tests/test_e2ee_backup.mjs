// Run: node tests/test_e2ee_backup.mjs
// Checks the pure helpers in webserver/matrix/e2ee-backup.js: recovery-key
// encoding, transfer-request validation, key chunking and device naming.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const h = createRequire(import.meta.url)('../webserver/matrix/e2ee-backup.js');
const now = 1_800_000_000_000;

// Recovery key round trip, and rejection of damaged keys.
const key = Uint8Array.from({ length: 32 }, (_, i) => (i * 7 + 3) & 0xff);
const encoded = h.encodeRecoveryKey(key);
assert.match(encoded, /^(\S{1,4} )+\S{1,4}$/);
assert.deepEqual(h.decodeRecoveryKey(encoded), key);
assert.deepEqual(h.decodeRecoveryKey(encoded.replace(/ /g, '')), key);
assert.equal(h.decodeRecoveryKey(encoded.slice(0, -1) + (encoded.endsWith('A') ? 'B' : 'A')), null);
assert.equal(h.decodeRecoveryKey('not a recovery key'), null);
assert.equal(h.decodeRecoveryKey(''), null);

// base58 keeps leading zero bytes.
assert.deepEqual(h.base58Decode(h.base58Encode(Uint8Array.from([0, 0, 1, 2]))), Uint8Array.from([0, 0, 1, 2]));

// Transfer request validation.
const good = { request_id: 'abc123', device_id: 'NEWDEVICE', client_ts: now - 1000, summary: 'Chrome on Windows' };
assert.deepEqual(h.validateRequest(good, 'OLDDEVICE', now), { request_id: 'abc123', device_id: 'NEWDEVICE', summary: 'Chrome on Windows', client_ts: now - 1000 });
assert.equal(h.validateRequest(good, 'NEWDEVICE', now), null, 'own device');
assert.equal(h.validateRequest({ ...good, client_ts: now - 15 * 86400000 }, 'X', now), null, 'expired');
assert.equal(h.validateRequest({ ...good, client_ts: now + 3600000 }, 'X', now), null, 'future');
assert.equal(h.validateRequest({ ...good, device_id: 'bad id!' }, 'X', now), null, 'device id charset');
assert.equal(h.validateRequest({ ...good, request_id: '' }, 'X', now), null);
assert.equal(h.validateRequest(null, 'X', now), null);
assert.equal(h.validateRequest({ ...good, summary: 'a\u0000b\n' + 'x'.repeat(200) }, 'X', now).summary.length, 80);

// Chunking stays under the limit and keeps every session in order.
const sessions = Array.from({ length: 50 }, (_, i) => ({ session_id: 's' + i, session_key: 'k'.repeat(500) }));
const chunks = h.chunkSessions(sessions, 4000);
assert.ok(chunks.length > 1);
assert.deepEqual(chunks.flat(), sessions);
for (const c of chunks) assert.ok(JSON.stringify(c).length <= 4000 + 600);
assert.deepEqual(h.chunkSessions([], 4000), []);
assert.equal(h.chunkSessions([{ big: 'x'.repeat(9000) }], 4000).length, 1, 'oversized session still sent');

// Device naming and times.
assert.equal(h.describeBrowser('Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/120 Safari/537.36'), 'Chrome on Windows');
assert.equal(h.describeBrowser('Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit Version/17 Safari/604'), 'Safari on iOS');
assert.equal(h.describeBrowser('Mozilla/5.0 (Linux; Android 14) Chrome/120 Mobile Safari/537'), 'Chrome on Android');
assert.equal(h.describeBrowser('Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0'), 'Firefox on Linux');
assert.equal(h.describeBrowser(''), 'Browser');
assert.equal(h.timeAgo(now - 30000, now), 'just now');
assert.equal(h.timeAgo(now - 5 * 60000, now), '5 min ago');
assert.equal(h.timeAgo(now - 3 * 3600000, now), '3 h ago');

// base64 round trip for the backup key.
const bytes = Uint8Array.from({ length: 32 }, (_, i) => 255 - i);
assert.deepEqual(h.base64ToBytes(h.bytesToBase64(bytes)), bytes);

console.log('e2ee-backup helpers ok');
