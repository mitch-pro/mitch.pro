// Real Chromium, simulated Blooket pages. Every network request is intercepted.
import assert from 'node:assert/strict';
import { JoinScheduler } from '../integrations/woodcreek/backend/join-scheduler.mjs';
import { BlooketInstanceManager } from '../integrations/woodcreek/backend/blooket-manager.mjs';
import { toPlaywrightCookies } from '../integrations/woodcreek/backend/flaresolverr-client.mjs';
import { normalizePin, readJoinScreen } from '../integrations/woodcreek/backend/blooket-connection.mjs';
const manager = new BlooketInstanceManager(); manager.useFlareSolverr=false;manager.joinScheduler=new JoinScheduler({spacingMs:0,capacity:async()=>true});
const wait=ms=>new Promise(r=>setTimeout(r,ms));
let scenario='normal',hits=0,launches=0;const navigations=[];
const originalLaunch=manager.launchBrowser.bind(manager);
manager.launchBrowser=async()=>{launches++;return originalLaunch()};
const html=()=>`<!doctype html><html><body><h1>Blooket</h1><div class="blookBox" hidden>Hidden old lobby</div><input placeholder="Nickname" hidden>
<form id="pin"><input placeholder="game id" aria-label="Game ID"><button type="submit">Continue</button></form>
<form id="nickname" hidden><input placeholder="nickname"><button type="submit">Join</button></form><p id="error" role="alert"></p><main id="lobby" hidden>Waiting for the host</main>
<script>window.pinSubmits=0;window.nameSubmits=0;
document.getElementById('pin').onsubmit=e=>{e.preventDefault();pinSubmits++;${scenario==='invalid'?"document.getElementById('error').textContent='Invalid Game ID';":scenario==='full'?"document.getElementById('error').textContent='Game lobby is full';":"document.getElementById('pin').hidden=true;document.getElementById('nickname').hidden=false;"}};
document.getElementById('nickname').onsubmit=e=>{e.preventDefault();nameSubmits++;if(${scenario==='duplicate'}&&nameSubmits===1){document.getElementById('error').textContent='That name is already in use';return}document.getElementById('error').textContent='';document.getElementById('nickname').hidden=true;document.getElementById('lobby').hidden=false;${scenario==='inprogress'?`document.getElementById('lobby').innerHTML='<div class=questionText>Question</div><div class=answersHolder><button>A</button><button>B</button></div>'`:''};};</script></body></html>`;
try {
  for(const input of ['','12345','1234567890','ab123456','123.456'])assert.throws(()=>normalizePin(input));
  assert.equal(toPlaywrightCookies([{name:'one',value:'1',domain:'.blooket.com'},{name:'two',value:'2',domain:'notblooket.com'},{name:'three',value:'3',domain:'blooket.com.evil.test'}]).length,1);
  assert.equal(normalizePin(' 123 456 '),'123456');assert.equal(normalizePin('123-456-789'),'123456789');
  const browsers=await Promise.all([manager.ensureBrowser(),manager.ensureBrowser(),manager.ensureBrowser()]);assert.equal(launches,1);assert(browsers.every(b=>b===browsers[0]),'Concurrent joins share one browser');
  const browser=browsers[0],newContext=browser.newContext.bind(browser);
  browser.newContext=async options=>{const context=await newContext(options);await context.route('**/*',route=>{const url=new URL(route.request().url());assert.equal(url.hostname,'play.blooket.com','No real game or external request');if(route.request().isNavigationRequest()){hits++;navigations.push({scenario,url:route.request().url(),method:route.request().method()});}return route.fulfill({contentType:'text/html',body:html()})});return context};
  manager.startBotLoop=instance=>{instance.loopStarted=true};
  for(const mode of ['normal','duplicate','invalid','full','inprogress']) {
    scenario=mode;const instance=manager.createBotInstance(mode,'Alex','cat','123456');
    await manager.runBotLifecycle(instance);
    if(mode==='invalid'||mode==='full'){
      assert.equal(instance.status,'error');assert.match(instance.diagnostic.error,mode==='invalid'?/Invalid Game PIN/:/lobby is full/,JSON.stringify(instance.diagnostic));assert.equal(instance.page,null,'Failed joins release their browser page');assert.equal(instance.context,null);
    } else {
      assert.equal(instance.status,mode==='inprogress'?'game':'lobby',JSON.stringify(instance.diagnostic));assert(instance.loopStarted);
      const counts=await instance.page.evaluate(()=>({pin:pinSubmits,name:nameSubmits}));assert.equal(counts.pin,1,'PIN must submit once');assert.equal(counts.name,mode==='duplicate'?2:1,'Nickname must submit once per attempt');
      const state=await readJoinScreen(instance.page);assert.equal(state.hasPin,false);assert.equal(state.hasName,false,'Hidden stale inputs cannot block lobby detection');
      assert.equal(instance.pingMs,null,'Unmeasured latency must not be fabricated');
    }
  }
  assert.equal(hits,5,JSON.stringify({navigations,timelines:[...manager.instances.values()].map(i=>({name:i.id,events:i.timeline.map(e=>e.msg)}))}));
  // A stop during newContext must close the late context instead of leaking it.
  const late=await newContext({});let closed=false;late.on('close',()=>closed=true);let release;
  const pending=new Promise(resolve=>release=resolve);browser.newContext=()=>pending;
  const joining=manager.createBotInstance('pending','Robin','cat','123456');const lifecycle=manager.runBotLifecycle(joining);
  await wait(0);await manager.retryBot(joining.id);await manager.stopBot(joining.id);release(late);await lifecycle;
  assert(closed,'Stop closes contexts created after cancellation');assert(!manager.instances.has(joining.id));
  browser.newContext=newContext;
  const retained=manager.instances.size;await browser.close();
  assert.equal(manager.instances.size,retained,'Browser loss must preserve entries for retry');
  assert.equal(manager.instances.get('normal').status,'error');
  await manager.stopFleet();
  // Stop must cancel queued joins, including while the concurrency gate is full.
  const releases=[];let created=0;
  manager.executeBotJoin=async(instance,active)=>{created++;await new Promise(resolve=>releases.push(resolve));assert(!active())};
  await manager.startFleet({pin:'123456',count:12,useFlareSolverr:false});
  for(let i=0;i<100&&created<2;i++)await wait(20);
  assert.equal(created,2,'Only two joins may run across the service');assert.equal(manager.instances.size,12);
  assert.equal([...manager.instances.values()].filter(i=>i.status==='queued').length,12);
  await manager.stopFleet();releases.forEach(resolve=>resolve());
  for(let i=0;i<100&&manager.joinScheduler.running;i++)await wait(20);
  assert.equal(created,2,'Stopped queues cannot launch remaining bots');assert.equal(manager.instances.size,0);
  console.log('PASS: current join selectors, hidden inputs, one submission per stage, duplicate-name retry, fatal errors, browser launch race, cancellation, resource cleanup, and queued-stop behavior. Simulated pages only.');
} finally { await manager.stopFleet(); }
