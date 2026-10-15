import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
const base=process.env.SITE_AUDIT_BASE_URL || 'http://127.0.0.1:4320';
assert(['127.0.0.1','localhost'].includes(new URL(base).hostname),'Use isolated fixture accounts only.');
const browser=await chromium.launch({executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
try {
 const context=await browser.newContext({viewport:{width:1440,height:1000},serviceWorkers:'block'});
 await context.route('**/*',route=>new URL(route.request().url()).origin===base?route.continue():route.abort());
 const page=await context.newPage();const errors=[];page.on('pageerror',e=>errors.push(e.message));
 const get=async path=>{const r=await context.request.get(base+path);assert(r.ok(),path+' '+r.status());return r.json();};
 const post=async(path,data)=>{const r=await context.request.post(base+path,{data});const body=await r.json();assert(r.ok(),path+' '+JSON.stringify(body));return body;};
 await page.goto(base+'/');await page.waitForFunction(()=>window.MitchShell?.ready);
 assert.equal(await page.locator('#home-brand').innerText(),'hello');
 assert.equal((await page.locator('.home-hello').innerText()).replace(/\s+/g,' '),'hello admin@mitch.pro');
 assert.equal(await page.locator('#site-presence,#unsubscribe-btn,.hud-system-status,.hud-pane-aside').count(),0);
 await page.mouse.move(2,400);await page.waitForFunction(()=>document.querySelector('#member-side-rail').classList.contains('is-open'));
 assert.equal(await page.locator('#owner-members-wrap,#premium-members-wrap,#admin-members-wrap').count(),0);
 await page.keyboard.press('Escape');assert(!await page.locator('#member-side-rail').evaluate(el=>el.classList.contains('is-open')));
 await page.locator('#sw-notif-btn').click();assert.equal(await page.locator('#sw-notif-btn').getAttribute('aria-expanded'),'true');
 assert(await page.locator('.sw-notif-tools').isVisible());await page.keyboard.press('Escape');assert.equal(await page.locator('#sw-notif-btn').getAttribute('aria-expanded'),'false');
 assert.equal(await page.locator('#sw-push-prompt').count(),0,'Push permission must not interrupt a visit.');
 for(const path of ['/','/coins/?v=2','/casino/','/enroll/','/game-portal/','/team/','/friends/','/members/','/rjuhsd/']) {
  await page.goto(base+path);assert.equal(await page.locator('#site-credit').count(),1,path);
  assert.equal(await page.locator('#site-credit').innerText(),path==='/rjuhsd/'?'Made by Tyler and Mitch.':'Created by Mitch and Tyler');
  for(const width of [1440,390]) {
   await page.setViewportSize({width,height:844});
   for(const fraction of [0,.5,1]) {
    await page.evaluate(fraction=>window.scrollTo({top:(document.documentElement.scrollHeight-innerHeight)*fraction,behavior:'instant'}),fraction);
    const rect=await page.locator('#site-credit').boundingBox();
    assert(rect && Math.abs(rect.x)<1 && Math.abs(rect.width-width)<1 && Math.abs(rect.y+rect.height-844)<1,`${path}: credit must stay across the viewport at scroll ${fraction}, width ${width}`);
   }
   for(const selector of ['#mobile-dock','.mobile-nav-dock']) {
    const dock=page.locator(selector);
    if(await dock.count() && await dock.isVisible()) {
     const nav=await dock.boundingBox(),credit=await page.locator('#site-credit').boundingBox();
     assert(nav.y+nav.height<=credit.y,`${path}: navigation must stay above the credit`);
    }
   }
  }
 }
 await page.setViewportSize({width:1440,height:1000});
 await page.goto(base+'/enroll/');assert(await page.locator('video').evaluate(el=>Boolean(el.controls&&el.compareDocumentPosition(document.querySelector('#account-access'))&Node.DOCUMENT_POSITION_FOLLOWING)));
 await page.goto(base+'/coins/?v=2');assert.equal(await page.title(),'MitchCoins — mitch.pro');assert.equal(await page.locator('.coin-destination').count(),4);
 const inventory=await get('/api/me/inventory');
 if(!inventory.cosmetics?.colors?.includes('neon_purple')) {
  const before=(await get('/api/me/coins')).coins;
  const purchase=await post('/api/shop/buy',{itemId:'neon_purple'});
  assert.equal((await get('/api/me/coins')).coins, before-purchase.cost,'A purchase must charge the server wallet.');
 }
 await post('/api/me/cosmetics/equip',{type:'name_color',itemId:'neon_purple'});
 const members=(await get('/api/members')).members;
 assert(members.some(m=>m.displayName==='Admin'&&m.color==='neon_purple'),'Equipped purchased color must be returned by People API.');
 const friends=(await get('/api/friends/list')).friends;
 assert(friends.some(f=>f.displayName==='Casey'&&f.color==='neon_purple'),'Friends API must include equipped colors.');
 await page.goto(base+'/friends/');await page.locator('.friend-name.neon_purple').first().waitFor();
 assert.notEqual(await page.locator('.friend-name.neon_purple').first().evaluate(el=>getComputedStyle(el).color),await page.locator('body').evaluate(el=>getComputedStyle(el).color));
 await page.goto(base+'/');await page.locator('.home-friend strong.neon_purple').first().waitFor();
 await page.locator('.hub-action#hub-friends-btn').click();await page.locator('.member-presence-name.neon_purple').first().waitFor();
 // User text must remain text even inside names, attributes, and activity lines.
 await page.evaluate(()=>renderOnlineMembers([{email:'member@example.com',displayName:'" onmouseover="window.__injected=1',activity:'<img src=x onerror="window.__injected=1">',online:true,color:'neon_purple'}]));
 assert.equal(await page.locator('#online-members-list [onmouseover],#online-members-list img[onerror]').count(),0);
 assert.equal(await page.evaluate(()=>window.__injected),undefined);
 await page.goto(base+'/casino/');await page.waitForFunction(()=>document.querySelector('#balance').textContent.includes('coins'));
 await page.locator('[data-filter="cards"]').click();assert.equal(await page.locator('.games>.card:visible').count(),4);
 await page.locator('[data-filter="all"]').click();
 for(const id of await page.locator('.games>.card').evaluateAll(els=>els.map(el=>el.id))) {
  await page.locator('#'+id+' .controls>button').click();assert(await page.locator('#fullscreen-overlay').evaluate(el=>el.classList.contains('active')),id);
  assert(await page.locator('.fs-controls-box .btn.primary').last().isVisible(),id);
  await page.locator('#fs-close-btn').click();
 }
 const before=(await get('/api/me/coins')).coins;
 const round=await post('/api/casino/coinflip',{amount:5,side:'heads'});
 assert.equal(round.newBalance,Number((before+round.net).toFixed(4)),'Casino result and wallet must settle together.');
 assert.equal((await get('/api/me/coins')).coins,round.newBalance);
 const bad=await context.request.post(base+'/api/casino/coinflip',{data:{amount:round.newBalance+1,side:'heads'}});assert.equal(bad.status(),400);
 assert.equal((await get('/api/me/coins')).coins,round.newBalance,'Rejected bets must not debit coins.');
 const catalog=(await get('/api/shop/items')).items;assert(!catalog.some(item=>(item.costType||'').startsWith('happy_hour')));
 assert(!('happyHour' in await get('/api/me')));
 assert.deepEqual(errors,[]);
 console.log('PASS: credits, greeting, hover panel, notifications, video order, coin hub, actual cosmetic purchase/equip, friend colors, escaped user text, all casino rooms, wallet settlement, rejected bets, and retired rewards.');
} finally {await browser.close();}
