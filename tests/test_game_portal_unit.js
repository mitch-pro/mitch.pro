import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import {
  GAME_PORTAL_DAILY_CAP,
  GAME_PORTAL_REWARD_PER_MINUTE,
  normalizeGamePortalTitle,
  settleGamePortalHeartbeat,
} from '../lib/game_portal_rewards.js';

assert.equal(GAME_PORTAL_REWARD_PER_MINUTE, 2, 'Portal reward should match the marketplace economy');
assert.equal(GAME_PORTAL_DAILY_CAP, 240, 'Portal rewards need a daily anti-idle cap');
assert.equal(normalizeGamePortalTitle('<b>Game</b>\nName'), 'bGame/bName');
assert.equal(normalizeGamePortalTitle('x'.repeat(100)).length, 60);

let state;
let result = settleGamePortalHeartbeat(state, 0, { active: true, game: 'Slope', dailyEarned: 0 });
assert.equal(result.earned, 0, 'Opening a game must not grant coins immediately');
state = result.next;
for (const now of [20_000, 40_000, 60_000]) {
  result = settleGamePortalHeartbeat(state, now, { active: true, game: 'Slope', dailyEarned: 0 });
  state = result.next;
}
assert.equal(result.earned, 2, 'One active minute should grant two MitchCoins');
assert.equal(result.minutes, 1);

result = settleGamePortalHeartbeat(state, 60_100, { active: true, game: 'Slope', dailyEarned: 2 });
assert.equal(result.earned, 0, 'Heartbeat spam must not grant coins');
result = settleGamePortalHeartbeat(state, 120_000, { active: true, game: 'Slope', dailyEarned: 2 });
assert.equal(result.earned, 0, 'Idle gaps must reset earning time');
result = settleGamePortalHeartbeat(state, 80_000, { active: true, game: 'Run 3', dailyEarned: 2 });
assert.equal(result.earned, 0, 'Switching games must not carry an old session reward');

state = { active: true, game: 'Slope', lastSeen: 0, accruedMs: 60_000 };
result = settleGamePortalHeartbeat(state, 20_000, { active: true, game: 'Slope', dailyEarned: GAME_PORTAL_DAILY_CAP });
assert.equal(result.earned, 0, 'The daily cap must be enforced server-side');

const html = readFileSync('webserver/game-portal/index.html', 'utf8');
const client = readFileSync('webserver/game-portal/ui/app.js', 'utf8');
const styles = readFileSync('webserver/game-portal/ui/style.css', 'utf8');
const proxy = readFileSync('rust/crates/mitch-server/src/routes/proxy.rs', 'utf8');
const handler = readFileSync('rust/crates/mitch-server/src/handler.rs', 'utf8');
const catalog = JSON.parse(readFileSync('webserver/game-portal/games.json', 'utf8'));
const catalogEntries = catalog.links.flatMap((section) => section.games || []);

assert(catalogEntries.length >= 300, 'The redesigned portal must include the full open-source game catalog');
assert(html.includes('id="game-grid"') && html.includes('id="player"'), 'Portal needs one catalog and one integrated player');
assert(html.includes('id="quick-grid"') && html.includes('id="shuffle"'), 'Portal needs an immediate featured shelf and shuffle action');
assert(html.includes('id="wallet-balance"') && html.includes('id="player-reward"'), 'Portal needs balance and earning status');
assert(html.includes('id="reward-progress"') && client.includes("'/api/game-portal/status'"), 'Portal needs the existing account balance and daily reward progress');
assert(!html.includes('/msn-games/'), 'The obsolete two-option chooser must be removed');
assert(client.includes("'/api/game-portal/heartbeat'"), 'Portal must report active gameplay to the authenticated backend');
assert(client.includes("'X-Mitch-Requested-With': '1'"), 'Reward heartbeat must include the application CSRF header');
assert(client.includes("'/proxy/luma'") && client.includes("'/proxy/calculated2'"), 'Integrated games must use the fixed same-origin game proxy');
assert(client.includes('window.MitchPresence?.setActivity'), 'Game activity must use the shared opt-in presence reporter');
assert(proxy.includes('("/proxy/luma/", "https://lumassets.pages.dev")') && proxy.includes('("/proxy/calculated2/", "https://calculated2.github.io")') && proxy.includes('("/proxy/gamemonetize/", "https://html5.gamemonetize.co")'), 'The game proxy must only use fixed upstream origins');
assert(handler.includes('!path.starts_with("/game-portal/")'), 'Portal assets must load before sign-in so SSO can complete cleanly');
assert(client.includes('function categoryFor(') && client.includes('renderFeatured();'), 'Catalog must provide useful genres and a popular shelf');
assert(client.includes('function startRewardLoop(') && client.includes('function stopRewardLoop('), 'Rewards must track active play only');
assert(styles.includes('.quick-grid') && styles.includes('@media(max-width:680px)'), 'Portal redesign must include its featured shelf and mobile layout');

for (const entry of catalogEntries) {
  if (String(entry[1] || '').startsWith('/img/games/')) {
    const filename = String(entry[1]).slice('/img/games/'.length);
    assert(existsSync(`webserver/game-portal/icons/${filename}`), `Missing game icon: ${filename}`);
  }
}

await import('../webserver/game-portal/ui/popularity.js');
const discovery = globalThis.MitchGameDiscovery;
assert.equal(discovery.rank('MotoX3M'), discovery.rank('Moto X3M'));
assert.equal(discovery.rank('Run Run 3D Challenge'), Number.MAX_SAFE_INTEGER, 'Clones must not inherit the rank of a similarly named game');
assert.equal(discovery.rank('Cookie Clicker Jigsaw Puzzle'), Number.MAX_SAFE_INTEGER);
assert(discovery.compare({title:'Cookie Clicker',url:'/games/cookie-clicker/'}, {title:'Cookie Clicker',url:'https://example.test/game',external:true}) < 0, 'An available local copy must take precedence over a same-title external copy');
const ranked = catalogEntries.map(entry => ({title: entry[0]})).sort(discovery.compare);
assert.deepEqual(ranked.slice(0, 6).map(game => game.title), ['Run 3', 'Snow Rider 3D', 'Retro Bowl', 'Subway Surfers', 'Drift Boss', 'MotoX3M']);
assert(html.includes('id="sort-games"'), 'The library must retain an explicit alternate sort');
assert(html.includes('class="player-home" href="/" target="_top"'), 'Home must leave an embedded game portal, not nest the homepage in it');
console.log('Game catalog, researched ordering, homepage navigation and secure play rewards passed.');
