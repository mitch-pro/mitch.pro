import assert from 'node:assert/strict';
import {readFile,mkdir} from 'node:fs/promises';
import {createHash,createHmac,randomBytes} from 'node:crypto';
import {execFileSync} from 'node:child_process';
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
  context.setDefaultTimeout(20000);
  await context.route('**/*',route=>new URL(route.request().url()).origin===base?route.continue():route.abort());
  if(email){
   const raw='e'+createHash('sha256').update(email).digest('hex').slice(0,24);
   const sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16);
   const token=randomBytes(32).toString('base64url'),key=createHash('sha256').update(token).digest('hex'),now=Date.now();
   const record={email,normEmail:email,sid,gen:0,createdAt:now,lastSeen:now,expiresAt:now+86400000,userAgent:'Fixture',ip:'127.0.0.1',devSuperuser:false};
   execFileSync('python3',['-c',"import sqlite3,json,sys,time; c=sqlite3.connect(sys.argv[1]); c.execute('BEGIN IMMEDIATE'); row=c.execute('SELECT content FROM json_documents WHERE path = ?', ('data/auth_sessions.json',)).fetchone(); doc=json.loads(row[0]) if row else {}; doc[sys.argv[2]]=json.loads(sys.argv[3]); c.execute('INSERT INTO json_documents(path,content,updated_at) VALUES (?,?,?) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at', ('data/auth_sessions.json',json.dumps(doc),int(time.time()*1000))); c.commit(); c.close()",fixture+'/data/mitchpro.db',key,JSON.stringify(record)]);
   await context.addCookies([{name:'mitch_session',value:token,url:base,httpOnly:true,sameSite:'Lax'}]);
  }
  return context;
 };
 const admin=await makeContext('banadmin@example.com'),student=await makeContext('banstudent@student.rjuhsd.us'),neighbor=await makeContext('banneighbor@student.rjuhsd.us'),anon=await makeContext();
 const headers={'X-Mitch-Requested-With':'1','X-Admin-Passphrase':pass};
 const setup=await admin.request.post(base+'/api/admin/passphrase-status',{headers,data:{passphrase:pass}});assert(setup.ok(),'Fixture admin passphrase setup: '+setup.status()+' '+(setup.ok()?'':await setup.text()));
 await admin.request.post(base+'/api/admin/unban-account',{headers,data:{email:'banstudent@student.rjuhsd.us'}});
 assert.equal((await anon.request.get(base+'/api/admin/bans')).status(),401);
 assert.equal((await neighbor.request.get(base+'/api/admin/bans',{headers})).status(),403);
 const page=await admin.newPage(),errors=[];page.on('pageerror',error=>errors.push(error.message));page.on('dialog',dialog=>dialog.accept());
 await page.goto(base+'/admin/bans/');await page.locator('#passphrase-lock').waitFor();
 await page.locator('#admin-passphrase').fill('incorrect-passphrase');await page.locator('#unlock-button').click();
 await page.waitForFunction(()=>document.getElementById('unlock-status').textContent.includes('Incorrect'));
 assert(!await page.locator('#bans-workspace').isVisible());
 await page.locator('#admin-passphrase').fill(pass);await page.locator('#unlock-button').click();await page.locator('#bans-workspace').waitFor();
 const oldServer=route=>route.fulfill({json:{bans:[]}});
 await page.route('**/api/admin/bans',oldServer);
 await page.locator('#refresh-bans').click();
 await page.waitForFunction(()=>document.getElementById('ban-status').textContent.includes('updating'));
 assert(await page.locator('#ban-form button').isDisabled(),'Old servers must not accept new duration choices.');
 assert(await page.locator('#restore-form button').isEnabled(),'Restoring access should remain available during a rollout.');
 await page.unroute('**/api/admin/bans',oldServer);
 await page.locator('#refresh-bans').click();
 await page.waitForFunction(()=>!document.querySelector('#ban-form button').disabled);
 await page.goto(base+'/members/');await page.locator('#people-staff-links a[href="/admin/bans/"]').waitFor();assert(await page.locator('#people-staff-links a[href="/admin/bans/"]').isVisible());
 await page.locator('#people-staff-links a[href="/admin/bans/"]').click();await page.locator('#bans-workspace').waitFor();
 const otherPage=await neighbor.newPage();await otherPage.goto(base+'/members/');await otherPage.waitForFunction(()=>window.MitchShell?.ready);
 assert(await otherPage.locator('#people-staff-links a[href="/admin/bans/"]').count()===0,'Ordinary members must not see the admin button.');
 const targetPage=await student.newPage();
 await targetPage.route('**/ban-guard.js*',route=>route.fulfill({contentType:'application/javascript',body:''}));
 await targetPage.goto(base+'/enroll/');
 const gamePage=await student.newPage();await gamePage.goto(base+'/games/kodys-keyboard/index.html');
 assert(await gamePage.locator('script[src^="/ban-guard.js"]').count());
 await targetPage.evaluate(()=>new Promise((resolve,reject)=>{
  const socket=new WebSocket(location.origin.replace(/^http/,'ws')+'/ws');
  socket.onopen=()=>resolve(true);socket.onerror=()=>reject(new Error('Fixture WebSocket failed'));
  socket.onmessage=event=>{try{if(JSON.parse(event.data).type==='account_banned')window.__banNotice=true;}catch{}};
  socket.onclose=()=>{window.__banClosed=true;};window.__fixtureSocket=socket;
 }));
 await page.locator('#ban-email').fill('banstudent@student.rjuhsd.us');
 const reason='Repeated spam. <img src=x onerror="window.__injected=1">';
 await page.locator('#ban-reason').fill(reason);await page.locator('#ban-form button').click();
 assert(!await page.locator('#ban-confirm').isVisible(),'Admins must select a duration.');
 await page.locator('#ban-duration').selectOption('1d');await page.locator('#ban-form button').click();
 await page.locator('#ban-confirm').waitFor();
 assert.equal(await page.locator('#confirm-duration').innerText(),'1 day');
 assert.equal(await page.locator('#confirm-reason').innerText(),reason);
 assert.equal(await page.locator('#ban-confirm img').count(),0);
 for(const width of [1440,390,320]){
  await page.setViewportSize({width,height:900});
  assert(await page.locator('#ban-confirm').evaluate(dialog=>dialog.scrollWidth<=dialog.clientWidth+1),'Confirmation overflow at '+width);
 }
 await page.locator('#ban-confirm button[value="cancel"]').click();
 assert((await student.request.get(base+'/api/presence/count')).ok(),'Cancel must not ban the account.');
 await page.setViewportSize({width:1440,height:1000});
 await page.locator('#ban-form button').click();await page.locator('#ban-confirm button[value="ban"]').click();
 await page.waitForFunction(()=>document.getElementById('ban-status').textContent.startsWith('Banned '));
 await page.locator('.ban-entry').waitFor();assert((await page.locator('.ban-entry').innerText()).includes(reason));
 assert.equal(await page.locator('.ban-entry img').count(),0);assert.equal(await page.evaluate(()=>window.__injected),undefined);
 await targetPage.waitForFunction(()=>window.__banClosed&&window.__banNotice);
 await gamePage.waitForURL(base+'/');await gamePage.locator('#ban-title').waitFor();
 for(const path of ['/','/enroll/','/api/madlibs/random','/api/pass','/matrix/config.json','/_matrix/client/v3/sync','/livekit/get_token','/proxy/luma/test.html','/verify-open.json','/games/kodys-keyboard/index.html','/swiftonion.onion']){
  const response=await student.request.get(base+path);assert.equal(response.status(),403,'Banned access to '+path);
 }
 const marker=(await student.cookies()).find(cookie=>cookie.name==='mitch_ban');assert(marker?.httpOnly,'Ban marker must be HttpOnly.');
 const markerOnly=await makeContext();await markerOnly.addCookies([marker]);
 assert.equal((await markerOnly.request.get(base+'/game-portal/')).status(),403,'Removing the login cookie must not clear a ban.');
 assert((await anon.request.get(base+'/game-portal/')).ok(),'Guest access must remain available.');
 const blocked=await student.request.get(base+'/api/presence/count');assert.equal(blocked.status(),403);
 const blockedData=await blocked.json();assert.equal(blockedData.banned,true);assert(blockedData.expiresAt>Date.now());
 assert((await page.locator('.ban-expiry').innerText()).startsWith('Ends '));
 const dialogs=[];targetPage.on('dialog',dialog=>{dialogs.push(dialog.message());dialog.dismiss();});
 const notice=await targetPage.goto(base+'/game-portal/');assert.equal(notice.status(),403);
 assert.equal(await targetPage.locator('#ban-title').innerText(),'Banned for 1 day');
 assert.equal(await targetPage.locator('#ban-reason').innerText(),reason);
 assert.equal(await targetPage.locator('.reason img').count(),0);
 assert((await targetPage.locator('.review').innerText()).startsWith('Reviewed:'));
 assert((await targetPage.locator('.notice').innerText()).includes('Access returns automatically'));
 await mkdir('artifacts/ui-review/bans',{recursive:true});
 for(const width of [1440,768,390,320]){
  await targetPage.setViewportSize({width,height:900});
  assert(await targetPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Ban notice overflow at '+width);
  assert(await targetPage.locator('#site-credit').isVisible());
  if([1440,390].includes(width))await targetPage.screenshot({path:`artifacts/ui-review/bans/notice-${width}.png`});
 }
 assert.deepEqual(dialogs,[],'Ban notices should not trigger browser alerts.');
 const legacyAppeal=await student.request.get(base+'/appeal.html');assert.equal(legacyAppeal.status(),200);assert(legacyAppeal.url().endsWith('/appeal/'));
 await targetPage.getByRole('link',{name:'Appeal ban',exact:true}).click();
 assert.equal(new URL(targetPage.url()).pathname,'/appeal/');
 await targetPage.locator('#email').fill('banstudent@student.rjuhsd.us');await targetPage.locator('#reason').fill('Please review this test ban.');
 await targetPage.locator('#submit-appeal').click();
 await targetPage.waitForFunction(()=>document.getElementById('message').textContent.includes('submitted for staff review'));
 assert.equal((await student.request.get(base+'/api/presence/count')).status(),403,'Submitting an appeal must not remove a ban.');
 for(const width of [1440,390,320]){
  await targetPage.setViewportSize({width,height:900});
  assert(await targetPage.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),'Appeal page overflow at '+width);
 }
 await targetPage.screenshot({path:'artifacts/ui-review/bans/appeal-320.png'});
 assert((await neighbor.request.get(base+'/api/presence/count')).ok(),'An account ban must not block its neighbor on the same IP.');
 await page.locator('#ban-search').fill('not-a-match');assert.equal(await page.locator('.ban-entry').count(),0);
 await page.locator('#ban-search').fill('Repeated spam');assert.equal(await page.locator('.ban-entry').count(),1);
 await page.locator('.ban-entry button').click();await page.waitForFunction(()=>document.getElementById('list-status').textContent.startsWith('Restored access'));
 await page.waitForFunction(()=>document.querySelectorAll('.ban-entry').length===0);
 assert((await student.request.get(base+'/api/presence/count')).ok(),'The same session must work after an unban.');
 assert((await markerOnly.request.get(base+'/game-portal/')).ok(),'The ban marker must stop restricting access after an unban.');
 const noPass=await admin.request.post(base+'/api/admin/ban-account',{data:{email:'banstudent@student.rjuhsd.us',reason:'test'},headers:{'X-Mitch-Requested-With':'1'}});assert.equal(noPass.status(),403);
 const rebanned=await admin.request.post(base+'/api/admin/ban-account',{headers,data:{email:'banstudent@student.rjuhsd.us',reason:'alias restore test'}});assert(rebanned.ok());
 await page.locator('#restore-email').fill('Ban.Student+alias@student.mitch.pro');await page.locator('#restore-form button').click();
 await page.waitForFunction(()=>document.getElementById('restore-status').textContent.startsWith('Restored access'));
 assert((await student.request.get(base+'/api/presence/count')).ok());
 for(const data of [{email:'banstudent@student.rjuhsd.us',reason:'   ',duration:'1d'},{email:'banstudent@student.rjuhsd.us',reason:'Spam',duration:'invalid'}]){
  assert.equal((await admin.request.post(base+'/api/admin/ban-account',{headers,data})).status(),400);
 }
 const timed=await admin.request.post(base+'/api/admin/ban-account',{headers,data:{email:'banstudent@student.rjuhsd.us',reason:'Expiry test',duration:'1h'}});assert(timed.ok());
 assert.equal((await student.request.get(base+'/api/presence/count')).status(),403);
 execFileSync('python3',['-c',"import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('UPDATE blacklist SET expires_at = 1 WHERE email = ?', (sys.argv[2],)); c.commit(); c.close()",fixture+'/data/mitchpro.db','banstudent@student.rjuhsd.us']);
 assert((await student.request.get(base+'/api/presence/count')).ok(),'The same session must work automatically after a temporary ban expires.');
 await page.locator('#refresh-bans').click();await page.waitForFunction(()=>document.querySelectorAll('.ban-entry').length===0);
 await page.locator('#ban-duration').selectOption('permanent');
 await page.locator('#ban-email').fill('banadmin@example.com');await page.locator('#ban-reason').fill('staff protection test');await page.locator('#ban-form button').click();await page.locator('#ban-confirm button[value="ban"]').click();
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
 console.log('PASS: separate admin button, role/passphrase gates, real session cookies, ban/unban, live game disconnects, global route checks, login-cookie removal protection, guest access, shared-network access, safe mixed-version rollout, required duration/reason, confirmation/cancel, Roblox-style notice, working appeal form, automatic expiry, alias restore, staff protection, and responsive themes.');
}finally{await browser.close();}
