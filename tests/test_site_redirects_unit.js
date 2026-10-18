import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { bellScheduleRedirect, blooketBotRedirect, RJUHSD_ORIGIN, BLOOKET_BOT_ORIGIN } from '../lib/site_redirects.js';

for (const host of ['mitch.pro', 'rjuhsd.school', 'woodcreek.rjuhsd.school']) {
  for (const path of ['/bell', '/bell/', '/bell.html', '/bell/index', '/bell/index/', '/bell/index.html', '/bell/index.htm', '/rjuhsd/bell/']) {
    for (const method of ['GET', 'HEAD']) {
      const target = bellScheduleRedirect(new URL(`https://${host}${path}?school=oakmont&utm_source=shortcut`), method);
      assert.equal(target, RJUHSD_ORIGIN + '/?school=oakmont&utm_source=shortcut');
      assert.equal(bellScheduleRedirect(new URL(target), method), null, 'Redirect must not loop');
    }
  }
}
for (const path of ['/', '/bell/schedule.js?v=5', '/api/bell/override', '/rjuhsd-assets/app.js', '/bellringer/', '/encrypt/']) {
  assert.equal(bellScheduleRedirect(new URL('https://mitch.pro' + path)), null, `${path} must remain available`);
}
assert.equal(bellScheduleRedirect(new URL('https://mitch.pro/bell/'), 'POST'), null);

for (const path of ['/blooket-bot', '/blooket-bot.html', '/blooket-bot/index', '/blooket-bot/index/', '/blooket-bot/index.html']) {
  for (const method of ['GET', 'HEAD']) {
    assert.equal(blooketBotRedirect(new URL('https://mitch.pro' + path + '?pin=123456'), method), '/blooket-bot/?pin=123456');
  }
}
for (const path of ['/', '/blooket-bot/', '/api/blooket-bot/status', '/previousblooket/', '/blooket-bot.css']) {
  assert.equal(blooketBotRedirect(new URL('https://mitch.pro' + path)), null, `${path} must not redirect`);
}
assert.equal(blooketBotRedirect(new URL('https://mitch.pro/blooket-bot/'), 'POST'), null);

for (const file of ['webserver/index.html', 'webserver/index-sales.html', 'webserver/app-shell.js']) {
  const source = readFileSync(file, 'utf8');
  assert(source.includes('https://rjuhsd.school/'), `${file} must link to the school hub`);
  assert(!/href\s*[:=]\s*['"]\/bell(?:\/|['"])/.test(source), `${file} still links to the old page`);
}

const home = readFileSync('webserver/index.html', 'utf8');
const homeFriends = readFileSync('webserver/home-friends.js', 'utf8');
const homeCss = readFileSync('webserver/home.css', 'utf8');
for (const destination of ['/game-portal/', '/matrix/', '/vms/', '/members/', '/preferences/', '/shop/', '/casino/']) {
  assert(home.includes(`href="${destination}"`), `Homepage must preserve ${destination} navigation`);
}
for (const id of ['home-search', 'links', 'hub-daily-chip', 'member-side-rail', 'home-search-results']) {
  assert(home.includes(`id="${id}"`), `Homepage must preserve the ${id} integration`);
}
assert(/\/home\.css\?v=\d+/.test(home) && /\/home-friends\.js\?v=\d+/.test(home), 'Homepage assets need cache-busted URLs');
assert(homeFriends.includes("hero.after(section)"), 'Friends activity should live inside the dashboard');
assert(homeCss.includes('prefers-reduced-motion') && homeCss.includes('--mh-bg: var(--t-bg)') && readFileSync('webserver/app.css', 'utf8').includes('prefers-color-scheme'), 'Homepage styles must inherit the shared theme and respect motion preferences');
const location = { href: 'https://mitch.pro/' };
const context = vm.createContext({ URL, location, window: { location }, localStorage: { getItem() { throw new Error('School links must bypass game launch preferences'); } } });
vm.runInContext(home.slice(home.indexOf('function launchSite('), home.indexOf('function openInNewTab(')), context);
context.launchSite('iframe', 'https://rjuhsd.school/?school=oakmont');
assert.equal(location.href, 'https://rjuhsd.school/?school=oakmont');
context.launchSite('iframe', 'https://woodcreek.site/?pin=123456');
assert.equal(location.href, '/blooket-bot/?pin=123456', 'Blooket Bot must use the connected Mitch dashboard');

// Installed Mitch PWAs require a same-origin shortcut; the server redirects it.
const shortcut = JSON.parse(readFileSync('webserver/manifest.json', 'utf8')).shortcuts.find(item => item.name === 'Bell Schedule');
assert.equal(bellScheduleRedirect(new URL(shortcut.url, 'https://mitch.pro')), RJUHSD_ORIGIN + '/?utm_source=pwa-shortcut');
assert(readFileSync('data/sites', 'utf8').includes('url /blooket-bot/ Blooket Bot'), 'Blooket Bot must open through the Mitch proxy');

// Top-left brand logo on rjuhsd.school must use mitch.pro logo (/icon-192.png)
const rjuhsdHtml = readFileSync('webserver/rjuhsd/index.html', 'utf8');
const rjuhsdRedesign = readFileSync('webserver/rjuhsd-assets/redesign.css', 'utf8');
const rjuhsdApp = readFileSync('webserver/rjuhsd-assets/app.js', 'utf8');
const preferencesHtml = readFileSync('webserver/preferences/index.html', 'utf8');
const preferencesSchoolCss = readFileSync('webserver/preferences/preferences-school.css', 'utf8');
assert(rjuhsdHtml.includes('<a class="brand" href="/" aria-label="rjuhsd.school home"><span class="brand-logo"><img class="site-logo" src="/icon-192.png"'), 'rjuhsd top left brand must use mitch.pro logo');
assert(rjuhsdHtml.includes('/rjuhsd-assets/redesign.css?v=3'), 'rjuhsd must load the schedule-first redesign');
assert(rjuhsdHtml.includes('/rjuhsd-assets/reference-theme.css?v=12'), 'rjuhsd must load updated reference-theme.css?v=12');
const referenceThemeCss = readFileSync('webserver/rjuhsd-assets/reference-theme.css', 'utf8');
assert(referenceThemeCss.includes('.calendar-days>button:hover:not(.today)'), 'dark calendar date hover must be styled without white box');
assert(referenceThemeCss.includes('body.dark .calendar-days>button:not(.today):nth-child(7n+1)'), 'weekend styling must not clobber today highlight');
assert(rjuhsdRedesign.includes('.schedule-zone { order: 2; }'), 'full schedule must appear before secondary school content');
assert(rjuhsdRedesign.includes('.live-card {') && rjuhsdRedesign.includes('.countdown-dial {'), 'active countdown must have the redesigned live surface');
assert(!rjuhsdApp.includes('js-signin-link'), 'rjuhsd.school must not prompt a cross-domain SSO sign-in');
assert(!rjuhsdHtml.includes('sso/bridge'), 'rjuhsd.school must not offer the mitchdog.com SSO bridge');
assert(preferencesHtml.includes('/preferences/preferences-school.css?v=4') || preferencesHtml.includes('/preferences/preferences-school.css?v=3') || preferencesHtml.includes('/preferences/preferences-school.css?v=2'), 'preferences must load its rjuhsd redesign');
assert(preferencesHtml.includes("classList.add('school-preferences')"), 'preferences must detect the school host');
assert(preferencesSchoolCss.includes('background: linear-gradient(110deg'), 'school preferences header must reveal the active background');
assert(preferencesSchoolCss.includes('.school-preferences.theme-light') || preferencesSchoolCss.includes(':is(.school-preferences.theme-light'), 'school preferences must support light mode');
assert(preferencesSchoolCss.includes('.prefs-page .page-head h1') && preferencesSchoolCss.includes('var(--t-fg'), 'school preferences light mode header must use readable ink color');
const handlerRs = readFileSync('rust/crates/mitch-server/src/handler.rs', 'utf8');
assert(handlerRs.includes('"/rjuhsd-assets/redesign.css"') && handlerRs.includes('"/preferences/preferences-school.css"'), 'school redesign stylesheets must remain public assets');
assert(!/<a class="brand"[^>]*><span class="brand-logo"><img[^>]*src="\/rjuhsd-assets\//.test(rjuhsdHtml), 'rjuhsd brand must not use school-based logo');

// Favicon on rjuhsd.school must use mitch.pro favicon (/favicon.ico)
assert(rjuhsdHtml.includes('<link rel="icon" href="/favicon.ico">'), 'rjuhsd favicon must use mitch.pro /favicon.ico');
assert(!/<link\s+rel="icon"[^>]*href="\/rjuhsd-assets\//.test(rjuhsdHtml), 'rjuhsd must not use rjuhsd-assets favicon');

const appJs = readFileSync('webserver/rjuhsd-assets/app.js', 'utf8');
assert(!appJs.includes("href=brand.logo"), 'app.js must not overwrite favicon with school logo');

// SEO & All-Schools bell schedules verification
const schools = ['woodcreek', 'roseville', 'granitebay', 'antelope', 'westpark', 'oakmont'];
const schoolDisplayNames = [
  'Woodcreek High School Bell Schedule',
  'Roseville High School Bell Schedule',
  'Granite Bay High School Bell Schedule',
  'Antelope High School Bell Schedule',
  'West Park High School Bell Schedule',
  'Oakmont High School Bell Schedule'
];

for (const name of schoolDisplayNames) {
  assert(rjuhsdHtml.includes(`<h3>${name}</h3>`), `rjuhsd/index.html must include heading for ${name}`);
}

for (const s of schools) {
  assert(rjuhsdHtml.includes(`data-school-card="${s}"`), `rjuhsd/index.html must have school card for ${s}`);
  assert(rjuhsdHtml.includes(`data-switch-school="${s}"`), `rjuhsd/index.html must have school switcher for ${s}`);
}

assert(rjuhsdHtml.includes('"@type": "ItemList"'), 'rjuhsd/index.html must have Schema.org ItemList');
assert(rjuhsdHtml.includes('"@type": "FAQPage"'), 'rjuhsd/index.html must have Schema.org FAQPage');
assert(rjuhsdHtml.includes('id="schools"'), 'rjuhsd/index.html must have #schools section');
assert(rjuhsdHtml.includes('id="faq"'), 'rjuhsd/index.html must have #faq section');

const sitemap = readFileSync('webserver/sitemap.xml', 'utf8');
const schoolSitemap = readFileSync('webserver/rjuhsd/sitemap.xml', 'utf8');
assert(sitemap.includes('https://mitchdog.com/'), 'Main sitemap must use the public canonical host');
assert(!sitemap.includes('https://rjuhsd.school/'), 'School URLs belong in the school host sitemap');
assert(schoolSitemap.includes('https://rjuhsd.school/'), 'School sitemap must include its bare root');
for (const school of schools.filter(school => school !== 'woodcreek')) {
  assert(schoolSitemap.includes(`https://rjuhsd.school/${school}/`), `School sitemap must include ${school}`);
}
assert(!schoolSitemap.includes('?school='), 'School sitemap must use canonical paths rather than duplicate query URLs');

const robots = readFileSync('webserver/robots.txt', 'utf8');
assert(robots.includes('Allow: /?school=*'), 'robots.txt must allow school queries');
assert(robots.includes('Sitemap: https://rjuhsd.school/sitemap.xml'), 'robots.txt must reference sitemap.xml');

console.log('Bell and Blooket links, legacy redirects, query preservation, assets, PWA shortcuts, all-school SEO, sitemaps, and direct navigation passed.');
