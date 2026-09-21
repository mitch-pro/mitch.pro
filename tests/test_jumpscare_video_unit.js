import assert from 'node:assert/strict';
import { existsSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';

const root = join(import.meta.dir, '..');
const videoPath = join(root, 'webserver', 'media', 'admin-jumpscare-krupp-1935.webm');
const broadcast = readFileSync(join(root, 'webserver', 'broadcast.js'), 'utf8');
const admin = readFileSync(join(root, 'webserver', 'admin', 'index.html'), 'utf8');
const server = readFileSync(join(root, 'server.js'), 'utf8');
const home = readFileSync(join(root, 'webserver', 'index.html'), 'utf8');
const serviceWorker = readFileSync(join(root, 'webserver', 'sw.js'), 'utf8');

assert.ok(existsSync(videoPath), 'The local jumpscare video must be deployed');
assert.ok(statSync(videoPath).size > 1_000_000, 'The jumpscare MP4 must not be an empty placeholder');
assert.match(broadcast, /video\.src = '\/media\/admin-jumpscare-krupp-1935\.webm'/);
assert.match(broadcast, /overlay\.append\(video, close, caption\)/);
assert.doesNotMatch(broadcast, /Tap for sound/, 'The overlay should not show a sound button');
assert.match(broadcast, /video\.muted = true/, 'Muted fallback is required when autoplay audio is blocked');
assert.match(broadcast, /video\.addEventListener\('ended', remove/, 'Overlay must clean itself up');
assert.doesNotMatch(broadcast, /myinstants\.com/, 'The old third-party screamer must be removed');
assert.match(broadcast, /fetch\('\/api\/broadcast\/latest'/, 'Clients need an HTTP fallback when school networks block WebSockets');
assert.match(broadcast, /setInterval\(pollLatestBroadcast, 3000\)/, 'Fallback delivery should check promptly');
assert.match(broadcast, /sessionStorage\.setItem\(LAST_BROADCAST_KEY/, 'WebSocket and polling delivery must be deduplicated per tab');
assert.match(broadcast, /pageOpenedAt = Date\.now\(\)/, 'Refreshed pages must not replay an older broadcast');
assert.match(broadcast, /Number\(data\.createdAt\) < pageOpenedAt/, 'Only pages open when the broadcast was sent may play it');
assert.match(broadcast, /scheduleReconnect\(\)/, 'WebSocket delivery must recover after a dropped connection');
assert.match(admin, /value="jumpscare">Video Jumpscare/);
assert.match(admin, /Play video for everyone/);
assert.match(server, /type === 'normal' && !msg/, 'Normal alerts still require a message');
assert.match(server, /function publishAdminBroadcast\(type, message\)/);
assert.match(server, /ADMIN_BROADCAST_TTL_MS = 5 \* 60 \* 1000/);
assert.match(server, /path === '\/api\/broadcast\/latest'/);
assert.match(server, /'Cache-Control', 'private, no-store, max-age=0'/);
assert.match(home, /broadcast\.js\?v=9/);
assert.match(server, /broadcast\.js\?v=9/);
assert.match(serviceWorker, /requestUrl\.pathname === '\/media\/admin-jumpscare-krupp-1935\.webm'/, 'Service Worker must not cache the large authenticated video stream');

console.log('Video jumpscare asset, resilient multi-PC delivery, deduplication, cleanup, and cache-version checks passed.');
