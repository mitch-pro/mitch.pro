import assert from 'node:assert/strict';
import {readFile,mkdir} from 'node:fs/promises';
import {createHash,createHmac} from 'node:crypto';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
const base=process.env.SITE_AUDIT_BASE_URL || 'http://127.0.0.1:4320';
const fixture=process.env.BANS_TEST_FIXTURE || '/tmp/mitch-bans-audit';
assert(['127.0.0.1','localhost'].includes(new URL(base).hostname)&&fixture.startsWith('/tmp/mitch-bans-'),'Ban tests must use isolated local accounts.');
const secret=await readFile(fixture+'/data/id_secret.key');
const browser=await chromium.launch({executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
const pass='FixtureBansPassphrase37';
try {
 const makeContext=async email=>{
  const context=await browser.newContext({serviceWorkers:'block',viewport:{width:1440,height:1000}});
  await context.route('**/*',route=>new URL(route.request().url()).origin===base?route.continue():route.abort());
  if(email){
   const raw='e'+createHash('sha256').update(email).digest('hex').slice(0,24);
   const sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16);
   await context.addCookies([{name:'studentId',value:sid,url:base}]);
  }
  return context;
 };
 const admin=await makeContext('banadmin@example.com'),student=await makeContext('banstudent@student.rjuhsd.us'),neighbor=await makeContext('banneighbor@student.rjuhsd.us'),anon=await makeContext();
 const headers={'X-Mitch-Requested-With':'1','X-Admin-Passphrase':pass};
 const setup=await admin.request.post(base+'/api/admin/passphrase-status',{headers,data:{passphrase:pass}});assert(setup.ok(),'Fixture admin passphrase setup: '+setup.status()+' '+(setup.ok()?'':await setup.text()));
 assert.equal((await anon.request.get(base+'/api/admin/bans')).status(),401);
 assert.equal((await neighbor.request.get(base+'/api/admin/bans',{headers})).status(),403);
 const page=await admin.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));page.on('dialog',dialog=>dialog.accept());
 await page.goto(base+'/admin/bans/');await page.locator('#passphrase-lock').waitFor();
 await page.locator('#admin-passphrase').fill('incorrect-passphrase');await page.locator('#unlock-button').click();
 await page.waitForFunction(()=>document.getElementById('unlock-status').textContent.includes('Incorrect'));
 assert(!await page.locator('#bans-workspace').isVisible());
 await page.locator('#admin-passphrase').fill(pass);await page.locator('#unlock-button').click();await page.locator('#bans-workspace').waitFor();
 await page.goto(base+'/');await page.locator('#bans-action-btn').waitFor();assert(await page.locator('#bans-action-btn').isVisible());
 await page.locator('#bans-action-btn').click();await page.locator('#bans-workspace').waitFor();
 const otherPage=await neighbor.newPage();await otherPage.goto(base+'/');await otherPage.waitForFunction(()=>window.MitchShell?.ready);
 assert(!await otherPage.locator('#bans-action-btn').isVisible(),'Ordinary members must not see the admin button.');
 const targetPage=await student.newPage();await targetPage.goto(base+'/enroll/');
 await targetPage.evaluate(()=>new Promise((resolve,reject)=>{
  const socket=new WebSocket(location.origin.replace(/^http/,'ws')+'/ws');
  socket.onopen=()=>resolve(true);socket.onerror=()=>reject(new Error('Fixture WebSocket failed'));
  socket.onmessage=event=>{try{if(JSON.parse(event.data).type==='account_banned')window.__banNotice=true;}catch{}};
  socket.onclose=()=>{window.__banClosed=true;};window.__fixtureSocket=socket;
 }));
 await page.locator('#ban-email').fill('banstudent@student.rjuhsd.us');
 const reason='Repeated spam. <img src=x onerror="window.__injected=1">';
 await page.locator('#ban-reason').fill(reason);await page.locator('#ban-form button').click();
 await page.waitForFunction(()=>document.getElementById('ban-status').textContent.startsWith('Banned '));
 await page.locator('.ban-entry').waitFor();assert((await page.locator('.ban-entry').innerText()).includes(reason));
 assert.equal(await page.locator('.ban-entry img').count(),0);assert.equal(await page.evaluate(()=>window.__injected),undefined);
 await targetPage.waitForFunction(()=>window.__banClosed&&window.__banNotice);
 const blocked=await student.request.get(base+'/api/presence/count');assert.equal(blocked.status(),403);assert.equal((await blocked.json()).banned,true);
 assert((await neighbor.request.get(base+'/api/presence/count')).ok(),'An account ban must not block its neighbor on the same IP.');
 await page.locator('#ban-search').fill('not-a-match');assert.equal(await page.locator('.ban-entry').count(),0);
 await page.locator('#ban-search').fill('Repeated spam');assert.equal(await page.locator('.ban-entry').count(),1);
 await page.locator('.ban-entry button').click();await page.waitForFunction(()=>document.getElementById('list-status').textContent.startsWith('Restored access'));
 await page.waitForFunction(()=>document.querySelectorAll('.ban-entry').length===0);
 assert((await student.request.get(base+'/api/presence/count')).ok(),'The same session must work after an unban.');
 const noPass=await admin.request.post(base+'/api/admin/ban-account',{data:{email:'banstudent@student.rjuhsd.us',reason:'test'},headers:{'X-Mitch-Requested-With':'1'}});assert.equal(noPass.status(),403);
 const rebanned=await admin.request.post(base+'/api/admin/ban-account',{headers,data:{email:'banstudent@student.rjuhsd.us',reason:'alias restore test'}});assert(rebanned.ok());
 await page.locator('#restore-email').fill('Ban.Student+alias@student.mitch.pro');await page.locator('#restore-form button').click();
 await page.waitForFunction(()=>document.getElementById('restore-status').textContent.startsWith('Restored access'));
 assert((await student.request.get(base+'/api/presence/count')).ok());
 await page.locator('#ban-email').fill('banadmin@example.com');await page.locator('#ban-reason').fill('staff protection test');await page.locator('#ban-form button').click();
 await page.waitForFunction(()=>document.getElementById('ban-status').textContent.includes('cannot ban admins'));
 await page.locator('#ban-search').fill('');
 await mkdir('artifacts/ui-review/bans',{recursive:true});
 for(const scheme of ['light','dark']){
  await page.evaluate(scheme=>document.documentElement.dataset.theme=scheme,scheme);
  for(const width of [1440,768,390,320]){
   await page.setViewportSize({width,height:900});
   assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Bans page overflow at '+width);
   if([1440,390].includes(width))await page.screenshot({path:`artifacts/ui-review/bans/${scheme}-${width}.png`});
  }
 }
 await page.setViewportSize({width:1440,height:900});
 await page.goto(base+'/admin/');
 await page.locator('.staff-panel-switcher a[href="/admin/bans/"]').waitFor();
 assert(await page.locator('.staff-panel-switcher a[href="/admin/bans/"]').isVisible());
 assert.equal(await page.locator('#ban-email').count(),0,'The main panel should link to the dedicated ban page.');
 assert.deepEqual(errors,[]);
 console.log('PASS: separate admin button, role/passphrase gates, real ban/unban, live disconnect, shared-network access, escaped reasons, search, row restore, alias restore, staff protection, and responsive themes.');
}finally{await browser.close();}
