import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
const base=process.env.SITE_AUDIT_BASE_URL || 'http://127.0.0.1:4320';
assert(['127.0.0.1','localhost'].includes(new URL(base).hostname),'Use the local fixture.');
const browser=await chromium.launch({executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
try {
 const context=await browser.newContext({serviceWorkers:'block'});
 await context.route('**/*',route=>new URL(route.request().url()).origin===base?route.continue():route.abort());
 const response=await context.request.get(base+'/api/presence/count');
 assert(response.ok());
 const payload=await response.json();
 assert.deepEqual(Object.keys(payload),['count'],'Only the aggregate count is public.');
 assert(Number.isSafeInteger(payload.count)&&payload.count>=0);
 assert(response.headers()['cache-control'].includes('no-store'));
 let count=7,unavailable=false;
 await context.route('**/api/presence/count',route=>route.fulfill({status:unavailable?503:200,json:{count}}));
 const page=await context.newPage();
 for(const path of ['/enroll/','/game-portal/','/rjuhsd/']) {
  await page.goto(base+path);
  await page.waitForFunction(()=>document.getElementById('site-active')?.textContent==='7 active');
  for(const width of [1440,768,390,320]) {
   await page.setViewportSize({width,height:900});
   for(const fraction of [0,1]) {
    await page.evaluate(f=>scrollTo({top:(document.documentElement.scrollHeight-innerHeight)*f,behavior:'instant'}),fraction);
    const credit=await page.locator('#site-credit-copy').boundingBox(),active=await page.locator('#site-active').boundingBox();
    assert(active.x>credit.x+credit.width,path+' credit and count overlap at '+width);
    assert(active.x+active.width<=width && active.y+active.height<=900 && active.y>=870,path+' count must stay at bottom right');
    assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1),path+' horizontal overflow');
   }
  }
 }
 count=0;await page.evaluate(()=>dispatchEvent(new Event('online')));
 await page.waitForFunction(()=>document.getElementById('site-active').textContent==='0 active');
 assert(await page.locator('#site-active').isVisible(),'A real zero is valid.');
 count=12;await page.evaluate(()=>dispatchEvent(new Event('online')));
 await page.waitForFunction(()=>document.getElementById('site-active').textContent==='12 active');
 unavailable=true;await page.evaluate(()=>dispatchEvent(new Event('online')));
 await page.waitForFunction(()=>document.getElementById('site-active').hidden);
 unavailable=false;count=7;await page.evaluate(()=>dispatchEvent(new Event('online')));
 await page.waitForFunction(()=>!document.getElementById('site-active').hidden);
 console.log('PASS: public aggregate, no caching, fixed bottom-right count, mobile spacing, live updates, zero, outage, and recovery.');
}finally{await browser.close();}
