import assert from 'node:assert/strict';
const {chromium}=await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
const base=process.env.SITE_AUDIT_BASE_URL || 'http://127.0.0.1:4320';
assert(['localhost','127.0.0.1'].includes(new URL(base).hostname),'Cache tests must run against a local fixture.');
const browser=await chromium.launch({executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
try {
 const context=await browser.newContext({serviceWorkers:'allow'});
 const page=await context.newPage();page.setDefaultTimeout(20000);
 await page.goto(base+'/game-portal/');
 await page.evaluate(async()=>{
  const registration=await navigator.serviceWorker.register('/sw.js?ban-cache-test=53',{scope:'/',updateViaCache:'none'});
  const worker=registration.installing || registration.waiting;
  if(worker&&worker.state!=='activated')await new Promise(resolve=>worker.addEventListener('statechange',()=>{if(worker.state==='activated')resolve();}));
  await navigator.serviceWorker.ready;
 });
 await page.waitForFunction(()=>!!navigator.serviceWorker.controller);
 await page.evaluate(async()=>{
  const cache=await caches.open('mitch-pro-cache-v53');
  await cache.put('/games/offline-fixture/index.html',new Response('<!doctype html><body>Cached private game</body>',{headers:{'Content-Type':'text/html'}}));
 });
 await context.setOffline(true);
 const response=await page.goto(base+'/games/offline-fixture/index.html');
 assert.equal(response.status(),503);
 assert((await page.innerText('body')).includes('Connect to the internet'));
 assert(!(await page.content()).includes('Cached private game'),'Cached HTML must not bypass server access checks.');
 await context.setOffline(false);
 assert((await page.goto(base+'/game-portal/')).ok(),'Guest access must still work online.');
 console.log('PASS: offline cached HTML cannot bypass server access checks; guest access remains available online.');
}finally{await browser.close();}
