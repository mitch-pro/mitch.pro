// Real local server and isolated fixture accounts; never production data.
import assert from 'node:assert/strict';
import { chromium } from '@playwright/test';
import { spawn } from 'node:child_process';
import { createHash, createHmac } from 'node:crypto';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { createServer } from 'node:net';
import { createServer as httpServer } from 'node:http';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const data = await mkdtemp('/tmp/mitch-presence-browser-');
const socket = createServer();
await new Promise(resolve => socket.listen(0, '127.0.0.1', resolve));
const port = socket.address().port; await new Promise(resolve => socket.close(resolve));
const base = 'http://127.0.0.1:' + port;
// Independent local service fixture; commands never reach real Blooket bots.
const shared = { instances: [], logs: [], accuracyRate: 95, answerSpeedMode:'human', answerDelayMinMs:1200, answerDelayMaxMs:2800, gameMode:'gold_quest', autoAnswer:false, autoMinigames:true, accuracyPct:100 }, requests = [], commands = [], sockets = new Set();
function sendFrame(socket, value) {
  const data = Buffer.from(JSON.stringify(value));
  const header = data.length < 126 ? Buffer.from([129, data.length]) : Buffer.from([129,126,data.length >> 8,data.length & 255]);
  socket.write(Buffer.concat([header,data]));
}
function broadcastState() { shared.sessionId='fixture-session';shared.revision=(shared.revision||0)+1;for (const socket of sockets) sendFrame(socket,{type:'state',state:shared}); }
let rejectNextSettings = false;
const uiRoot = resolve(root,'integrations/woodcreek/public');
const upstream = httpServer(async (req,res) => {
  requests.push({path:req.url, cookie:req.headers.cookie,authorization:req.headers.authorization});
  if(req.method==='POST') {
    const chunks=[];for await(const chunk of req)chunks.push(chunk);
    const body=JSON.parse(Buffer.concat(chunks));commands.push(body);
    if (rejectNextSettings && body.op==='settings') { rejectNextSettings=false;res.writeHead(400,{'content-type':'application/json'});return res.end(JSON.stringify({error:'Settings were rejected.'})); }
    if (body.op==='stop-bot') shared.instances=shared.instances.filter(bot=>bot.id!==body.id);
    if('accuracy'in body)shared.accuracyRate=body.accuracy;if('autoAnswer'in body)shared.autoAnswer=body.autoAnswer;broadcastState();
    res.writeHead(200,{'content-type':'application/json'});return res.end(JSON.stringify({success:true}));
  }
  const pathname=new URL(req.url,'http://fixture').pathname;
  if (pathname==='/' || ['woodcreek.js','woodcreek.css','components.css','assets/tylerium-logo.svg','assets/clever-portal.ico','assets/manrope.woff2','assets/jetbrains-mono.woff2'].includes(pathname.slice(1))) {
    if(pathname==='/'&&!req.headers.authorization?.startsWith('Bearer wc1.')){res.writeHead(303,{location:'https://mitchdog.com/blooket-bot/'});return res.end();}
    const relative=pathname==='/'?'index.html':pathname.slice(1);
    const body=await readFile(resolve(uiRoot,relative));
    const mime=relative.endsWith('.html')?'text/html':relative.endsWith('.js')?'text/javascript':relative.endsWith('.css')?'text/css':relative.endsWith('.woff2')?'font/woff2':'image/svg+xml';
    res.writeHead(200,{'content-type':mime});return res.end(body);
  }
  if(pathname==='/api/account'){res.writeHead(200,{'content-type':'application/json'});return res.end(JSON.stringify({account:{username:'alice',displayName:'Alice',pfp:'',activeColor:'electric_blue',activeBadge:'verified_badge',activeProfileEffect:'profile_gold_frame'}}));}
  res.writeHead(200,{'content-type':'application/json','set-cookie':'fixture_cookie=must-not-reach-browser'});res.end(JSON.stringify(shared));
});
upstream.on('upgrade',(req,socket)=>{
  const accept=createHash('sha1').update(req.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
  socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: '+accept+'\r\n\r\n');
  sockets.add(socket);sendFrame(socket,{type:'state',state:shared});
  let buffer=Buffer.alloc(0);
  socket.on('data',chunk=>{
    buffer=Buffer.concat([buffer,chunk]);
    while(buffer.length>=2){
      const op=buffer[0]&15,masked=!!(buffer[1]&128);let length=buffer[1]&127,offset=2;
      if(length===126){if(buffer.length<4)return;length=buffer.readUInt16BE(2);offset=4}
      if(length===127){socket.destroy();return}
      const header=offset+(masked?4:0);if(buffer.length<header+length)return;
      const payload=Buffer.from(buffer.subarray(header,header+length));
      if(masked)for(let i=0;i<payload.length;i++)payload[i]^=buffer[offset+i%4];
      buffer=buffer.subarray(header+length);
      if(op===8){socket.end();return}
      if(op===1){const value=JSON.parse(payload);commands.push(value);if(value.op==='launch')sendFrame(socket,{type:'command-result',op:'launch',result:{success:true}});if(value.op==='settings'){if('accuracy'in value)shared.accuracyRate=value.accuracy;if('autoAnswer'in value)shared.autoAnswer=value.autoAnswer;broadcastState()}}
    }
  });
  socket.on('close',()=>sockets.delete(socket));socket.on('error',()=>sockets.delete(socket));
});
await new Promise(resolve=>upstream.listen(0,'127.0.0.1',resolve));
const backendBase='http://127.0.0.1:'+upstream.address().port;

const server = spawn(root + '/rust/target/debug/mitch-server', [], {
  cwd: root, stdio: 'ignore', env: { ...process.env, DATA_DIR: data, MITCH_BASE: root,
    WOODCREEK_BACKEND_URL:backendBase, NODE_ENV: 'test', GOD_MODE: '0', PORT: String(port), VAPID_PUBLIC_KEY: '', RECAPTCHA_SITE_KEY: '',
    PVE_URL: 'https://127.0.0.1:1', PROXMOX_HOST: '127.0.0.1', PROXMOX_PORT: '1' }
});
const delay = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, message, timeout = 4000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await check()) return; await delay(80); }
  throw new Error(message);
}
let aliceBrowser, watcherBrowser;
try {
  await until(async () => { try { return (await fetch(base + '/enroll/')).ok; } catch { return false; } }, 'Server startup', 15000);
  const secret = await readFile(data + '/id_secret.key');
  const db = new DatabaseSync(data + '/mitchpro.db');
  const accounts = ['alice', 'bob', 'carol'].map(name => {
    const email = 'presence-' + name + '@example.test';
    const raw = 'e' + createHash('sha256').update(email).digest('hex').slice(0, 24);
    const sid = raw + '.' + createHmac('sha256', secret).update(raw).digest('hex').slice(0, 16);
    db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid, email, email, Date.now());
    db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run(sid, JSON.stringify({ email, used: true }), Date.now());
    return { email, sid };
  });
  const [alice, bob, carol] = accounts;
  db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run('alias-token',JSON.stringify({email:'pre.sence-alice@example.test'}),Date.now());
  for (const [a, b] of [[alice, carol], [carol, alice]]) db.prepare('INSERT INTO friend_lists(email,data,updated_at) VALUES(?,?,?)').run(a.email, JSON.stringify([b.email]), Date.now());
  db.close();
  [aliceBrowser, watcherBrowser] = await Promise.all([chromium.launch({ headless: true }), chromium.launch({ headless: true })]);
  async function contextFor(browser, user) {
    const context = await browser.newContext({ serviceWorkers: 'block', reducedMotion: 'reduce' });
    await context.addCookies([{ name: 'id', value: user.sid, url: base }]);
    await context.addInitScript(() => localStorage.setItem('_mitch_cookie_consent', 'accepted'));
    await context.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
    return context;
  }
  const actor = await contextFor(aliceBrowser, alice), observer = await contextFor(watcherBrowser, bob);
  await actor.addInitScript(() => {
    window.WebSocket = new Proxy(window.WebSocket, { construct(Native, args) {
      const socket = new Native(...args); (window.__testSockets ||= []).push(socket); return socket;
    } });
  });
  const carolApi = await watcherBrowser.newContext();
  await carolApi.addCookies([{ name: 'id', value: carol.sid, url: base }]);
  async function actorStatus() {
    const response = await observer.request.get(base + '/api/members');
    assert.equal(response.status(), 200);
    const body = await response.json();
    assert.equal(body.members.filter(member=>String(member.handle||'').includes('alice')).length,1,'Email aliases must not duplicate People');
    return body.members.find(member => String(member.email || '').includes('alice') || String(member.handle || '').includes('alice'));
  }
  const page=await actor.newPage(),errors=[];
  page.on('pageerror',error=>errors.push(error.message));
  const direct=new WebSocket(backendBase.replace('http:','ws:')+'/socket');
  let directState;
  direct.onmessage=event=>{const message=JSON.parse(event.data);directState=message.state};
  await page.goto(base+'/blooket-bot/');
  await page.waitForFunction(()=>document.getElementById('status-text')?.textContent==='Connected' && document.getElementById('bulk-editor-textarea')?.value.length>0, null, {timeout:8000}).catch(async error => { throw new Error(JSON.stringify({errors, requests:requests.map(r=>r.path), status:await page.locator('#status-text').textContent(),notice:await page.locator('#account-notice-text').textContent()})+' '+error.message); });
  assert(!commands.length,'Opening the shared dashboard must not send settings or commands');
  assert.equal(page.url(),base+'/blooket-bot/');
  assert.equal(await page.locator('html').getAttribute('data-app-base'),'/blooket-bot');
  assert.equal(await page.locator('.workspace-heading h1').innerText(),'Blooket');
  assert.equal(await page.locator('#account-name').innerText(),'Alice');
  assert.equal(await page.locator('#account-name').getAttribute('data-color'),'electric_blue');
  assert.equal(await page.locator('#account-badge').innerText(),'✔');
  assert.equal(await page.locator('#account-menu summary').getAttribute('data-effect'),'profile_gold_frame');
  assert.equal(await page.title(),'Clever Portal');
  assert.equal(await page.locator('link[rel=icon]').getAttribute('href'),'/blooket-bot/assets/clever-portal.ico');
  assert.equal(await page.getByText('Server status',{exact:true}).count(),0);
  assert.equal(await page.locator('#btn-answering').innerText(),'Start answering');
  rejectNextSettings=true;
  await page.locator('#btn-answering').click();
  await page.getByText('Settings were rejected.',{exact:true}).waitFor();
  assert.equal(await page.locator('#btn-answering').innerText(),'Start answering');
  assert.equal(await page.locator('#btn-answering').isDisabled(),false,'A failed command must release the button');
  await page.locator('#dismiss-connection-error').click();
  await page.locator('#btn-answering').click();
  await page.getByRole('button',{name:'Stop answering',exact:true}).waitFor();
  assert.equal(shared.autoAnswer,true);
  for(const socket of sockets) sendFrame(socket,{type:'state',state:{...shared,revision:shared.revision-1,autoAnswer:false}});
  await page.waitForTimeout(50);
  assert.equal(await page.locator('#btn-answering').innerText(),'Stop answering','An older socket snapshot cannot overwrite an acknowledged HTTP result');
  direct.send(JSON.stringify({op:'settings',autoAnswer:false}));
  await page.getByRole('button',{name:'Start answering',exact:true}).waitFor();
  shared.instances=[{id:'fixture-bot',name:'Fixture',blook:'cat',status:'game',statusText:'Game connected.',questionsAnswered:0,streak:0,timeline:[],technicalTrace:[]}];broadcastState();
  await page.waitForFunction(()=>!document.getElementById('btn-tool-master-solver').disabled);
  await page.locator('#btn-open-cheats').click();
  await page.locator('#btn-tool-master-solver').click();
  await page.getByRole('button',{name:'Stop answering',exact:true}).waitFor();
  await page.locator('#tools-dialog [data-close]').click();
  assert(!requests.some(request=>request.path==='/api/fleet/inject'),'Tools answering must use the shared controller rather than inject another loop');
  direct.send(JSON.stringify({op:'settings',autoAnswer:false}));
  await page.getByRole('button',{name:'Start answering',exact:true}).waitFor();
  const previousCommands=commands.length;
  await page.locator('#btn-force-answer').click();
  await until(async()=>!await page.locator('#btn-force-answer').isDisabled(),'Manual answer acknowledgement releases the control');
  assert.equal(commands.slice(previousCommands).filter(command=>command.op==='force-answer').length,1,'Answer once sends a single command');
  await page.locator('.bot-row').getByRole('button',{name:'Stop',exact:true}).click();
  await page.locator('.bot-row').waitFor({state:'detached'});
  assert(commands.some(command=>command.op==='stop-bot'&&command.id==='fixture-bot'),'Individual stop reaches the service through the authenticated bridge');
  shared.instances=[];broadcastState();
  const fonts=await page.evaluate(async()=>{await document.fonts.ready;return [...document.fonts].filter(font=>font.status==='loaded').map(font=>font.family)});
  assert(fonts.includes('Manrope')&&fonts.includes('JetBrainsMono'),'Tylerium fonts must load through the Mitch mount');
  assert(!(await page.locator('body').innerText()).includes('↗'));
  for(const socket of sockets)sendFrame(socket,{type:'error',message:'Game lobby is full.'});
  await page.locator('#connection-error').waitFor({state:'visible'});
  assert.equal(await page.locator('#connection-error-text').innerText(),'Game lobby is full.');
  await page.locator('#dismiss-connection-error').click();
  assert(await page.locator('#connection-error').isHidden());
  assert.equal(await page.locator('#accuracy-badge-box').textContent(),'95%');
  for(const width of [1440,768,390,320]) {await page.setViewportSize({width,height:1000});assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'Mounted dashboard must fit '+width+' '+JSON.stringify(await page.evaluate(()=>[...document.querySelectorAll('body *')].filter(el=>el.getBoundingClientRect().right>innerWidth+1&&el.getBoundingClientRect().width>0).slice(0,15).map(el=>({tag:el.tagName,id:el.id,class:el.className,right:el.getBoundingClientRect().right})))));}
  await page.setViewportSize({width:1440,height:1050});
  await page.screenshot({path:'/tmp/mitch-blooket-dashboard.png',fullPage:true});
  assert(await page.locator('script[src="/presence.js?v=1"]').count());
  assert((await actor.request.head(base+'/blooket-bot/')).ok());
  await page.locator('.site-nav [href="#gameplay-settings"]').click();
  await page.locator('#range-accuracy').focus();
  await page.locator('#range-accuracy').evaluate(input=>{input.value='92';input.dispatchEvent(new Event('input',{bubbles:true}));});
  await until(async()=>shared.accuracyRate===92,'WebSocket command and state round trip');
  assert.deepEqual(Object.keys(commands.findLast(command=>command.op==='settings'&&command.accuracy===92)).sort(),['accuracy','op'],'Editing accuracy cannot overwrite other settings');
  await page.locator('#input-pin').focus();
  await until(async()=>directState?.accuracyRate===92,'Direct Woodcreek viewer must receive changes sent through Mitch');
  const response=await actor.request.post(base+'/blooket-bot/api/fleet/settings',{
    headers:{Origin:base,'X-Mitch-Requested-With':'1',Authorization:'Bearer must-stay-on-mitch'},data:{accuracy:93}
  });
  assert.equal(response.status(),200);
  await until(async()=>(await page.locator('#accuracy-badge-box').innerText())==='93%','HTTP command must update live Mitch viewer');
  assert.equal(directState.accuracyRate,93);
  direct.send(JSON.stringify({op:'settings',accuracy:94}));
  await until(async()=>(await page.locator('#accuracy-badge-box').innerText())==='94%','Woodcreek-side changes must reach the Mitch viewer');
  await page.locator('#input-pin').fill('123456');
  await page.locator('#btn-launch').click();
  await until(async()=>commands.some(command=>command.op==='launch'&&command.pin==='123456'),'Real dashboard launch control must reach the local fixture');
  await until(async()=>!await page.locator('#btn-launch').isDisabled(),'Join acknowledgement releases the button');
  const before=commands.length;
  assert.equal((await actor.request.post(base+'/blooket-bot/api/fleet/settings',{headers:{Origin:base},data:{accuracy:80}})).status(),403);
  assert.equal((await actor.request.post(base+'/blooket-bot/api/fleet/settings',{headers:{Origin:'https://external.example','X-Mitch-Requested-With':'1'},data:{accuracy:80}})).status(),403);
  assert.equal(commands.length,before,'CSRF requests must never reach Woodcreek');
  assert.equal((await actor.request.get(base+'/blooket-bot/socket',{headers:{Origin:'https://external.example'}})).status(),403);
  assert.equal((await actor.request.get(base+'/blooket-bot/server.mjs')).status(),404);
  const anonymous=await watcherBrowser.newContext();
  assert.equal((await anonymous.request.get(base+'/blooket-bot/api/state')).status(),401);
  assert.equal((await anonymous.request.get(base+'/blooket-bot/',{maxRedirects:0})).status(),302);
  assert(requests.every(request=>!request.cookie && (!request.authorization || request.authorization.startsWith('Bearer wc1.'))),'Only scoped grants may reach Woodcreek; Mitch cookies and incoming authorization stay on Mitch');
  assert(!(await actor.cookies()).some(cookie=>cookie.name==='fixture_cookie'),'Upstream cookies must never reach Mitch users');
  const revokeDb=new DatabaseSync(data+'/mitchpro.db');
  revokeDb.prepare('INSERT INTO revoked_ids(sid,data,updated_at) VALUES(?,?,?)').run(alice.sid,JSON.stringify({email:alice.email}),Date.now());revokeDb.close();
  assert.equal((await actor.request.get(base+'/blooket-bot/api/state')).status(),401);
  direct.close();
  assert.deepEqual(errors,[]);
  console.log('PASS: authenticated mounted UI, HTTP and WebSocket relay, shared live state in both directions, CSRF/origin checks, cookie isolation, route restrictions, and revocation. Local fixtures only.');
} finally {
  await Promise.allSettled([aliceBrowser?.close(), watcherBrowser?.close()]);
  server.kill('SIGTERM');
  await new Promise(resolve => server.once('exit', resolve));
  for(const socket of sockets)socket.destroy();
  await new Promise(resolve=>upstream.close(resolve));
  await rm(data, { recursive: true, force: true });
}
