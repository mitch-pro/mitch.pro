import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { readFile } from 'node:fs/promises';
import { resolve, extname, sep } from 'node:path';

// A static fixture exercises the public dashboard without live accounts, calendar
// services, weather services, or production data. Install Playwright separately
// and pass PLAYWRIGHT_MODULE when it is not installed in this checkout.
const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
const root = resolve('webserver');
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.json': 'application/json', '.png': 'image/png', '.webp': 'image/webp', '.svg': 'image/svg+xml', '.ico': 'image/x-icon' };
const server = createServer(async (request, response) => {
  try {
    let path = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
    if (path.endsWith('/')) path += 'index.html';
    const file = resolve(root, '.' + path);
    if (!file.startsWith(root + sep)) throw Error('Outside webroot');
    const body = await readFile(file);
    response.writeHead(200, { 'Content-Type': types[extname(file)] || 'application/octet-stream', 'Cache-Control': 'no-store' });
    response.end(body);
  } catch {
    response.writeHead(404);
    response.end('Not found');
  }
});
await new Promise(done => server.listen(0, '127.0.0.1', done));
const base = `http://127.0.0.1:${server.address().port}`;
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH || '/usr/bin/chromium' });
const errors = [];
let cases = 0;

function pacific(value) { return new Date(value + '-07:00'); }
async function fixture(date, school = 'woodcreek', width = 1440, override = null) {
  const context = await browser.newContext({ viewport: { width, height: 1000 }, timezoneId: 'America/Los_Angeles', colorScheme: 'dark', reducedMotion: 'reduce', serviceWorkers: 'block' });
  await context.addInitScript(() => {
    for (const campus of ['woodcreek', 'roseville', 'granitebay', 'antelope', 'westpark', 'oakmont']) {
      if (!localStorage.getItem('rjuhsd_lunch_' + campus)) localStorage.setItem('rjuhsd_lunch_' + campus, '1');
      localStorage.setItem('rjuhsd_onboarded_' + campus, 'yes');
    }
  });
  await context.route('**/*', async route => {
    const url = new URL(route.request().url());
    const json = body => route.fulfill({ json: body });
    if (url.pathname === '/api/school-info') return json({ calendar_verified: true, stale: false, events: [{ date: '2026-10-30', title: 'Campus event' }, { date: '2026-12-17', title: 'Final exams' }] });
    if (url.pathname === '/api/bell/override') return json({ override });
    if (url.pathname === '/api/site-info') return json({ primary: 'https://mitch.pro', alternate: 'https://mitchdog.com' });
    if (url.pathname.startsWith('/api/')) return json({});
    if (url.hostname === 'api.open-meteo.com') return json({ current: { temperature_2m: 72, apparent_temperature: 71, weather_code: 0, wind_speed_10m: 4 }, daily: { time: ['2026-10-26'], weather_code: [0], temperature_2m_max: [75], temperature_2m_min: [52] } });
    return url.origin === base ? route.continue() : route.abort();
  });
  const page = await context.newPage();
  page.setDefaultTimeout(10000);
  page.on('pageerror', error => errors.push(error.message));
  await page.clock.install({ time: pacific(date) });
  await page.clock.pauseAt(pacific(date));
  await page.goto(base + '/rjuhsd/' + (school === 'woodcreek' ? '' : '?school=' + school));
  await page.waitForFunction(() => document.getElementById('current-period').textContent !== 'Loading…');
  await page.waitForFunction(() => !document.getElementById('source-status').textContent.startsWith('Checking'));
  return { page, context };
}
const text = (page, selector) => page.locator(selector).innerText();
async function setTime(page, value) {
  // Advance the interval once while ending at the requested whole second.
  await page.clock.setSystemTime(new Date(pacific(value).getTime() - 1000));
  await page.clock.runFor(1000);
}
async function state(page, value) {
  assert.equal(await page.locator('.live-card').getAttribute('data-countdown-state'), value);
}
async function activePeriod(page, name) {
  const active = page.locator('#timeline .timeline-row.active');
  assert.equal(await active.count(), 1);
  assert((await active.innerText()).includes(name), `Expected active timeline row for ${name}`);
}

try {
  {
    const { page, context } = await fixture('2026-10-26T09:51:59');
    await state(page, 'active');
    assert.equal(await text(page, '#current-period'), 'Period 1');
    assert.equal(await text(page, '#countdown'), '00:01');
    await activePeriod(page, 'Period 1');
    await page.clock.runFor(2000);
    await state(page, 'passing');
    assert.equal(await page.locator('#timeline .active').count(), 0, 'Expired class must stop highlighting at its bell');
    assert.equal(await text(page, '#countdown-end'), '10:00 AM');
    assert.equal(await text(page, '#countdown'), '07:59', 'Passing countdown must lose exactly one second after the bell');
    await setTime(page, '2026-10-26T10:00:00');
    await state(page, 'active');
    assert.equal(await text(page, '#current-period'), 'Period 2');
    await activePeriod(page, 'Period 2');
    assert.equal(await text(page, '#next-period'), 'Lunch');
    assert.equal(await text(page, '#next-time'), '11:22 AM');
    assert.match(await text(page, '#snapshot-progress'), /\d+%/);
    assert.equal(await page.locator('.countdown-dial').evaluate(element => getComputedStyle(element, '::after').content.replaceAll('"', '')), await text(page, '#countdown-percent'), 'The dial must display its actual progress percentage');
    await setTime(page, '2026-10-26T10:00:01');
    assert.equal(await text(page, '#countdown'), '81:59', 'Whole-second class countdown must not round fractional minutes up');
    assert.equal(await text(page, '#countdown-context'), '82 min left');
    cases += 4;

    // Lunch changes alter the actual live schedule and remain campus-specific.
    await setTime(page, '2026-10-26T11:35:00');
    assert.equal(await text(page, '#current-period'), 'Lunch');
    await page.locator('#change-lunch').click();
    await page.locator('[data-choose-lunch="2"]').click();
    assert.equal(await text(page, '#current-period'), 'Period 3');
    assert.equal(await page.evaluate(() => localStorage.getItem('rjuhsd_lunch_woodcreek')), '2');
    assert.match(await text(page, '#schedule-note'), /Lunch 2/);
    await page.reload();
    await page.waitForFunction(() => document.getElementById('current-period').textContent === 'Period 3');
    await page.locator('#change-lunch').click();
    const dialogButtons = page.locator('#onboarding button');
    await dialogButtons.last().focus();
    await page.keyboard.press('Tab');
    assert.equal(await dialogButtons.first().evaluate(button => button === document.activeElement), true, 'Lunch keyboard focus must wrap within its dialog');
    await page.keyboard.press('Escape');
    assert.equal(await page.locator('#onboarding').isVisible(), false);
    assert.equal(await page.evaluate(() => document.activeElement.id), 'change-lunch');
    cases += 2;

    await page.locator('#schedule-mode').selectOption('regular');
    await state(page, 'preview');
    assert.notEqual(await text(page, '#live-mode'), 'LIVE');
    assert.equal(await page.locator('#timeline .active').count(), 0, 'Manual schedule previews must not claim a live class');
    await page.locator('#schedule-mode').selectOption('auto');
    await state(page, 'active');
    cases++;

    await setTime(page, '2026-10-26T15:20:00');
    await state(page, 'complete');
    assert.match(await text(page, '#next-period'), /Tuesday|Oct 27/);
    assert.equal(await text(page, '#next-time'), '8:30 AM');
    assert.match(await text(page, '#snapshot-progress'), /100%|complete/i);
    await context.close();
    cases++;
  }
  {
    const { page, context } = await fixture('2026-10-25T12:00:00');
    await state(page, 'off');
    assert.match(await text(page, '#current-period'), /Weekend|No school/i);
    assert.match(await text(page, '#next-period'), /Monday|Oct 26/);
    assert.equal(await text(page, '#next-time'), '8:30 AM');
    assert.equal(await text(page, '#header-lunch'), 'Lunch 1', 'An empty weekend schedule must not replace the saved lunch with combined lunch');
    await context.close();
    cases++;
  }
  {
    // Explicit campus overrides and legacy Woodcreek-only overrides must never
    // change a neighboring school's bells.
    for (const [campus, overrideSchool, expectedEnd] of [['woodcreek', 'roseville', '9:52 AM'], ['roseville', null, '9:51 AM']]) {
      const override = { date: '2026-10-26', name: 'Assembly schedule', schedule: { lunch1: [{ name: 'Period 1', start: '08:30', end: '09:00' }] } };
      if (overrideSchool) override.school = overrideSchool;
      const { page, context } = await fixture('2026-10-26T08:40:00', campus, 1440, override);
      assert.equal(await text(page, '#countdown-end'), expectedEnd, 'Overrides must be confined to their intended campus');
      await context.close();
      cases++;
    }
  }
  {
    const { page, context } = await fixture('2026-10-30T15:21:00');
    await state(page, 'complete');
    assert.match(await text(page, '#next-period'), /Tuesday|Nov 3/, 'Next bell must skip the Monday district closure and weekend');
    assert.equal(await text(page, '#next-time'), '8:30 AM');
    await context.close();
    cases++;
  }
  {
    const { page, context } = await fixture('2026-10-25T23:59:59');
    await state(page, 'off');
    await page.clock.runFor(2000);
    await state(page, 'before');
    assert.equal(await page.locator('#hero-date').getAttribute('datetime'), '2026-10-26', 'The dashboard must roll into the next Pacific day without reloading');
    assert.equal(await text(page, '#snapshot-first'), '8:30 AM');
    await page.setViewportSize({ width: 320, height: 1000 });
    const [countdown, body] = await Promise.all([page.locator('#countdown').boundingBox(), page.locator('.live-card-body').boundingBox()]);
    assert(countdown.x >= body.x && countdown.x + countdown.width <= body.x + body.width, 'Long before-school countdown must fit the narrow card');
    await context.close();
    cases++;
  }
  {
    const override = { date: '2026-10-26', school: 'woodcreek', name: 'Assembly schedule', schedule: { lunch1: [{ name: 'Period 1', start: '08:30', end: '09:00' }, { name: 'Lunch', start: '09:05', end: '09:35' }] } };
    const { page, context } = await fixture('2026-10-26T08:40:00', 'woodcreek', 1440, override);
    assert.equal(await text(page, '#current-period'), 'Period 1');
    assert.equal(await text(page, '#countdown'), '20:00');
    assert.equal(await text(page, '#header-lunch'), 'Combined lunch');
    assert.match(await text(page, '#source-status'), /Assembly schedule/);
    await page.locator('#school-select').selectOption('roseville');
    assert.equal(await text(page, '#countdown-end'), '9:51 AM', 'A school-specific override must not change another campus');
    await context.close();
    cases++;
  }
  {
    const { page, context } = await fixture('2026-10-26T07:35:00', 'granitebay');
    await state(page, 'before');
    assert.equal(await page.locator('#timeline .period-name').filter({ hasText: 'Period 0' }).count(), 0);
    await page.locator('#include-period0').check();
    await state(page, 'active');
    assert.equal(await text(page, '#current-period'), 'Period 0');
    await activePeriod(page, 'Period 0');
    assert.equal(await text(page, '#snapshot-first'), '7:30 AM');
    await page.locator('#school-select').selectOption('roseville');
    assert.equal(await text(page, '#school-heading'), 'Roseville');
    assert.match(await text(page, '.calendar-toolbar small'), /Roseville/i);
    assert.match(await text(page, '.agenda-heading > span'), /Roseville/i);
    assert.equal(await page.locator('#official-bells').getAttribute('href'), 'https://roseville.rjuhsd.us/about/bell-schedules');
    assert.equal(await page.locator('#include-period0').isChecked(), false);
    assert.equal(await page.locator('#period0-label').isVisible(), false);
    await page.reload();
    await page.waitForFunction(() => document.getElementById('school-heading').textContent === 'Roseville');
    assert.equal(await page.locator('#school-select').inputValue(), 'roseville');
    await page.locator('[data-calendar-date="2026-10-30"]').click();
    assert.equal(await page.locator('[data-calendar-date="2026-10-30"]').getAttribute('aria-pressed'), 'true');
    assert.match(await text(page, '#event-list'), /Campus event/);
    await page.locator('[data-calendar-date="2026-10-29"]').click();
    assert.match(await text(page, '#event-list'), /No events on this date/);
    await page.locator('#calendar-today').click();
    assert.match(await text(page, '.agenda-heading > span'), /Roseville/);
    assert.match(await text(page, '#event-list'), /Campus event/);
    await context.close();
    cases += 4;
  }
  {
    const { page, context } = await fixture('2026-10-26T10:25:00');
    for (const width of [1440, 820, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      for (const mode of ['dark', 'light']) {
        await page.evaluate(mode => window.__theme.apply(mode), mode);
        await page.evaluate(() => { document.activeElement?.blur(); scrollTo(0, 0); });
        const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
        assert.equal(overflow, false, `${mode} layout overflows at ${width}px`);
        const countdown = await page.locator('#countdown').boundingBox();
        assert(countdown && countdown.width > 30 && countdown.x >= 0 && countdown.x + countdown.width <= width, `Countdown must fit at ${width}px`);
        if ([1440, 390].includes(width)) await page.screenshot({ path: `/tmp/rjuhsd-refresh-${mode}-${width}.png`, fullPage: false });
        if (process.env.RJUHSD_FULL_SCREENSHOTS && mode === 'dark' && [1440, 390].includes(width)) await page.screenshot({ path: `/tmp/rjuhsd-refresh-full-${width}.png`, fullPage: true });
        if (process.env.RJUHSD_FULL_SCREENSHOTS && mode === 'dark' && width === 1440) {
          for (const [section, file] of [['#schedule-panel', 'schedule'], ['#events', 'calendar']]) {
            await page.locator(section).evaluate(element => element.scrollIntoView({ block: 'start', behavior: 'instant' }));
            await page.screenshot({ path: `/tmp/rjuhsd-${file}-1440.png` });
          }
          await page.evaluate(() => scrollTo(0, 0));
        }
        cases++;
      }
    }
    assert.equal(await page.locator('script[type="application/ld+json"]').count(), 1);
    await page.locator('#theme-btn').click();
    assert.equal(await page.evaluate(() => document.documentElement.classList.contains('theme-light')), false, 'The visible theme control must change themes');
    await page.locator('#study-timer-toggle').click();
    await page.clock.runFor(2000);
    assert.equal(await text(page, '#study-timer-clock'), '24:58');
    await page.locator('#study-timer-toggle').click();
    await page.clock.runFor(2000);
    assert.equal(await text(page, '#study-timer-clock'), '24:58', 'Pause must stop the study timer');
    await page.locator('#study-timer-reset').click();
    assert.equal(await text(page, '#study-timer-clock'), '25:00');
    await context.close();
    cases += 2;
  }
  assert.deepEqual(errors, [], 'Dashboard must not throw browser errors');
  console.log(`RJUHSD refresh: ${cases} bell, lunch, school, keyboard and responsive checks passed.`);
  console.log('Screenshots: /tmp/rjuhsd-refresh-{dark,light}-{1440,390}.png');
} finally {
  await browser.close();
  await new Promise(done => server.close(done));
}
