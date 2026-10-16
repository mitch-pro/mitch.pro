import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {createHash,createHmac,randomBytes} from 'node:crypto';
import {execFileSync} from 'node:child_process';
const {chromium} = await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');

const base = process.env.VM_PORTAL_TEST_URL || 'http://127.0.0.1:8765/vms/';
const browser = await chromium.launch({ headless: true, executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH || process.env.CHROME_PATH });
const vm = {
  id: 'test-vm', name: 'My Computer', operatingSystem: 'Ubuntu Desktop 24.04 LTS',
  status: 'stopped', cpuCores: 8, cpuUsage: 0, memoryUsed: 0,
  memoryTotal: 16 * 1073741824, diskUsed: 34 * 1073741824,
  diskTotal: 128 * 1073741824, ipAddress: '', uptime: 0,
  managedDesktop: true, desktopAvailable: true, adminAccessAllowed: false, adminAccessRequested: false,
  cooldownRemainingSeconds: 0, upgrades: { dailyMaxSeconds: 21600 },
  lease: { isExempt: false, remainingSeconds: 21600, canExtend: true, extended: false, dailyExtensionUsed: false }
};

try {
  for (const width of [1920, 1440, 1280, 820, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 950 }, deviceScaleFactor: 1, serviceWorkers: 'block' });
    if (process.env.BANS_TEST_FIXTURE) {
      const fixture=process.env.BANS_TEST_FIXTURE;
      assert(fixture.startsWith('/tmp/mitch-bans-') && ['127.0.0.1','localhost'].includes(new URL(base).hostname));
      const email='banneighbor@student.rjuhsd.us',secret=readFileSync(fixture+'/data/id_secret.key');
      const raw='e'+createHash('sha256').update(email).digest('hex').slice(0,24);
      const sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16);
      const token=randomBytes(32).toString('base64url'),key=createHash('sha256').update(token).digest('hex'),now=Date.now();
      const record={email,normEmail:email,sid,gen:0,createdAt:now,lastSeen:now,expiresAt:now+86400000,userAgent:'Fixture',ip:'127.0.0.1',devSuperuser:false};
      execFileSync('python3',['-c',"import sqlite3,json,sys,time; c=sqlite3.connect(sys.argv[1]); c.execute('BEGIN IMMEDIATE'); row=c.execute('SELECT content FROM json_documents WHERE path = ?', ('data/auth_sessions.json',)).fetchone(); doc=json.loads(row[0]) if row else {}; doc[sys.argv[2]]=json.loads(sys.argv[3]); c.execute('INSERT INTO json_documents(path,content,updated_at) VALUES (?,?,?) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at', ('data/auth_sessions.json',json.dumps(doc),int(time.time()*1000))); c.commit(); c.close()",fixture+'/data/mitchpro.db',key,JSON.stringify(record)]);
      await page.context().addCookies([{name:'mitch_session',value:token,url:new URL(base).origin,httpOnly:true,sameSite:'Lax'}]);
    }
    const errors = [], actions = [];
    let current = { ...vm }, delayRefresh = false, desktopAttempts = 0;
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/api/vm/**', async route => {
      const url = new URL(route.request().url());
      if (url.pathname === '/api/vm/computers' && delayRefresh) await new Promise(resolve => setTimeout(resolve, 250));
      if (route.request().method() === 'POST') actions.push({ path: url.pathname, body: route.request().postDataJSON() });
      if (url.pathname.endsWith('/desktop-session')) {
        desktopAttempts++;
        return route.fulfill({status:desktopAttempts===1?409:200,contentType:'application/json',body:JSON.stringify(desktopAttempts===1?{error:'Preparing desktop',code:'desktop_preparing'}:{socketPath:'/api/vm/desktop/ws?session=fixture',credentials:{password:'fixture'}})});
      }
      if (url.pathname === '/api/vm/computers/test-vm') return route.fulfill({json:{computer:current}});
      const data = url.pathname === '/api/vm/computers' ? { computers: [current], isEligible: true }
        : url.pathname === '/api/vm/upgrades' ? { catalog: { cpu: [] }, current: {}, coins: 100 }
        : { success: true, message: 'Done' };
      await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(data) });
    });
    await page.goto(base, { waitUntil: 'domcontentloaded' });
    await page.locator('.computer-card').waitFor();
    assert.equal(await page.locator('.main-action').innerText(), 'Start Computer');
    assert.equal(await page.locator('.status-pill').last().innerText(), 'Offline');
    assert.equal(await page.locator('.desktop-preview').count(), 0);
    assert.equal(await page.locator('[data-action="restart"]').count(), 0);
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, `horizontal overflow at ${width}px`);
    if (width === 1440 || width === 390) {
      delayRefresh = true;
      await page.locator('#refresh-button').click();
      await page.locator('#refresh-button.is-loading').waitFor();
      assert.equal(await page.locator('#refresh-button').getAttribute('aria-label'), 'Refreshing computer status');
      await page.locator('#refresh-button:not(.is-loading)').waitFor();
      delayRefresh = false;
      if (process.env.VM_SCREENSHOT_DIR) await page.screenshot({ path: `${process.env.VM_SCREENSHOT_DIR}/vm-offline-${width}.png`, fullPage: true });
      await page.locator('.main-action').click();
      assert.equal(actions.at(-1)?.body.action, 'start');
      assert.match(await page.locator('.main-action').innerText(), /Starting/);
      await page.locator('#refresh-button').click();
      current = { ...vm, status: 'running', ipAddress: '10.1.2.3', uptime: 3600, cpuUsage: .25, memoryUsed: 4 * 1073741824, adminAccessRequested: true };
      await page.locator('#refresh-button').click();
      await page.locator('.main-action').filter({ hasText: 'Open Desktop' }).waitFor();
      assert.equal(await page.locator('.preview-overlay').count(), 0);
      assert.equal(await page.locator('.resource-meter').count(), 3);
      assert.equal(await page.locator('.admin-request-banner').count(), 0);
      assert.equal(await page.locator('[data-action="toggle-admin-access"]').count(), 0);
      await page.locator('.more-menu summary').click();
      assert.equal(await page.locator('[data-action="restart"]').isVisible(), true);
      await page.locator('[data-action="restart"]').click();
      assert.equal(await page.locator('#confirm-dialog[open]').count(), 1);
      await page.locator('#confirm-dialog [value="cancel"]').click();
      await page.locator('.more-menu summary').click();
      await page.locator('[data-action="repair-desktop"]').click();
      assert(actions.some(action=>action.path.endsWith('/repair-desktop')));
      await page.locator('[data-action="open-upgrade"]').click();
      assert.equal(await page.locator('#upgrade-dialog[open]').count(), 1);
      await page.locator('#upgrade-close-btn').click();
      await page.locator('.more-menu summary').click();
      await page.locator('[data-action="recreate"]').click();
      assert.equal(await page.locator('#provision-dialog[open]').count(), 1);
      assert.equal(await page.locator('#provision-dialog input[type="password"]').count(),0);
      await page.locator('#provision-cancel').click();
      await page.locator('.more-menu summary').click();
      await page.locator('[data-action="shutdown"]').click();
      const shutdownRequest = page.waitForRequest(request => request.url().endsWith('/power') && request.postDataJSON()?.action === 'shutdown');
      await page.locator('#confirm-action').click();
      assert.equal((await shutdownRequest).postDataJSON().action, 'shutdown');
      assert.match(await page.locator('.main-action').innerText(), /Shutting down/);
      if (process.env.VM_SCREENSHOT_DIR) await page.screenshot({ path: `${process.env.VM_SCREENSHOT_DIR}/vm-online-${width}.png`, fullPage: true });
    }
    if (width===1440) {
      current={...vm,status:'running'};
      await page.route('**/novnc/core/rfb.js',route=>route.fulfill({contentType:'text/javascript',body:`export default class RFB extends EventTarget {
        constructor(screen){super();window.fixtureRfbCount=(window.fixtureRfbCount||0)+1;screen.replaceChildren(document.createElement('canvas'));queueMicrotask(()=>this.dispatchEvent(new Event('connect')));}
        focus(){} disconnect(){}
      }`}));
      await page.goto(new URL('/vms/desktop/?id=test-vm',base).href,{waitUntil:'domcontentloaded'});
      await page.getByRole('heading',{name:'Preparing your desktop'}).waitFor();
      await page.waitForFunction(()=>document.getElementById('machine-state').textContent.includes('Connected')).catch(async error=>{console.error(await page.evaluate(()=>({state:document.getElementById('machine-state').textContent,title:document.getElementById('connection-title').textContent,copy:document.getElementById('connection-copy').textContent,rfb:window.fixtureRfbCount})));console.error(errors);throw error;});
      assert(await page.locator('#site-credit').isVisible(),'Desktop retains the fixed site credit.');
      const bottom=await page.locator('#display-frame').evaluate(el=>el.getBoundingClientRect().bottom);
      const creditTop=await page.locator('#site-credit').evaluate(el=>el.getBoundingClientRect().top);
      assert(bottom<=creditTop+1,'Credit must not cover the remote display.');
      await page.locator('#more-button').click();await page.locator('#reconnect-button').click();
      await page.waitForFunction(()=>window.fixtureRfbCount===2);
      assert.equal(desktopAttempts,3,'Initial preparation should retry, followed by a fresh reconnect ticket.');
    }
    assert.deepEqual(errors, []);
    await page.close();
  }
  console.log('VM portal layout and controls passed at five viewport widths.');
} finally {
  await browser.close();
}
