import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const friends = readFileSync('webserver/friends/index.html', 'utf8');
const coins = readFileSync('webserver/coins/index.html', 'utf8');
const coinWidget = readFileSync('webserver/mitch-coins.js', 'utf8');
const casino = readFileSync('webserver/casino/index.html', 'utf8');
const notifications = readFileSync('webserver/notifications/index.html', 'utf8');
const broadcast = readFileSync('webserver/broadcast.js', 'utf8');
const shellCss = readFileSync('webserver/mitch-ui.css', 'utf8');
const home = readFileSync('webserver/index.html', 'utf8');
const members = readFileSync('webserver/members/index.html', 'utf8');
const server = readFileSync('server.js', 'utf8');

assert(friends.includes('class="friends-summary"') && friends.includes('class="friend-actions"'), 'Friends needs the game-style connections layout');
assert(friends.includes("document.getElementById('onlineCount')"), 'Friends needs a live online count');
assert((coins.match(/class="earn-card"/g) || []).length >= 10, 'Coin wallet needs at least ten visible earning paths');
assert(coinWidget.includes("widget.href = status === 'guest' ? '/enroll/' : '/coins/'"), 'Global wallet must open the MitchCoins hub');

for (const game of ['rock-paper-scissors', 'lucky-seven', 'color-card', 'triple-dice', 'plinko']) {
  assert(server.includes(`/api/casino/${game}`), `${game} needs a server-side casino route`);
}
assert((casino.match(/data-category=/g) || []).length >= 15, 'Casino needs a large filterable game library');
assert(casino.includes("$$('.casino-filter')") && casino.includes('id="card-plinko"'), 'Casino filters and new games must be wired');

assert(notifications.includes('id="loadError"') && notifications.includes('async function boot()'), 'Notification settings need distinct login and load failures');
assert(broadcast.includes('aria-controls="sw-notif-panel"') && broadcast.includes('sw-notif-type'), 'Notification tray needs accessible state and notification types');
assert(broadcast.includes("type: 'presence_ping'") && broadcast.includes("page: path.slice(0, 180)") && broadcast.includes("fetch('/api/presence/heartbeat'"), 'Every authenticated page needs detailed WebSocket presence with an HTTP fallback');
assert(broadcast.includes("root.id = 'site-presence'") && broadcast.includes("member.page || '/'") && broadcast.includes("' is-chat'"), 'The global online panel must show exact pages and prioritize chat activity');
assert(server.includes("touchUserPresence(activeMatrixNorm, 'Chatting in Matrix'"), 'Matrix traffic must keep chat users online in site presence');
assert(server.includes('Date.now() - matrixSeen < PRESENCE_FALLBACK_TTL_MS'), 'Recent Matrix activity must outrank competing background-tab presence');
assert(server.includes('MATRIX_MESSAGE_EMAIL_INTERVAL_MS = 7 * 24 * 60 * 60 * 1000') && server.includes('MATRIX_MESSAGE_EMAIL_DELAY_MS = 10 * 60 * 1000'), 'Ordinary Matrix email alerts must be delayed and limited to a weekly cadence');
assert(!server.includes("if (amount > 1_000_000_000) adminActionError(400, 'amount is too large')") && !server.includes("if (amount > 1_000_000_000) {\n        return jsonResp(400, { error: 'amount is too large' });"), 'Higher-up coin gifts must not have an artificial maximum');
assert(members.includes('presence-detail') && members.includes("member.activity || member.playing"), 'Members directory must display detailed live activity');
assert(shellCss.includes('(max-height: 820px)'), 'Shared UI needs Chromebook-height density rules');
assert(home.includes('me.isCoOwner') && home.includes('/admin/#approval-tools'), 'Staff shortcuts must work for co-owners and open a real admin section');

console.log('Friends, MitchCoins, casino, notifications, staff shortcuts, and Chromebook layout checks passed.');
