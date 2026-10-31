const { chromium } = await import(process.env.PLAYWRIGHT_MODULE || '@playwright/test');
import { readdir, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
const base = process.env.SITE_AUDIT_BASE_URL || 'http://127.0.0.1:4320';
const phase = process.argv[2] || 'before';
const out = resolve('artifacts/ui-review/site-' + phase);
await mkdir(out, { recursive: true });
const browser = await chromium.launch({ executablePath: process.env.PLAYWRIGHT_EXECUTABLE_PATH });
try {
 const context = await browser.newContext({ viewport: {width:1440,height:1000}, colorScheme:'dark', serviceWorkers:'block' });
 await context.addInitScript(() => { try { if (location.hostname === '127.0.0.1') localStorage.setItem('_mitch_cookie_consent','accepted'); } catch {} });
 await context.route('**/*', route => new URL(route.request().url()).origin === base ? route.continue() : route.abort());
 const dirs = await readdir('webserver', {withFileTypes:true});
 const excluded = new Set(['games','novnc','matrix','sexypickleclub','ttygames','unsubscribe','bell','redirect','prox','msn-games']);
 const routes = ['/', '/index-sales.html', '/maintenance.html', '/faq/', '/games/'];
 for(const dir of dirs.filter(d => d.isDirectory() && !excluded.has(d.name))) {
  try { if ((await readdir('webserver/'+dir.name)).includes('index.html')) routes.push('/'+dir.name+'/'); }catch{}
 }
 routes.push('/admin/vms/', '/vms/desktop/');
 const results=[];
 for(const path of (process.argv.length > 3 ? process.argv.slice(3) : routes)) {
  const page=await context.newPage();
  const errors=[], failedAssets=[];
  page.on('pageerror',e=>errors.push(e.message));
  page.on('response',response=>{const u=new URL(response.url());if(u.origin===base && response.status()>=400 && !u.pathname.startsWith('/api/'))failedAssets.push({path:u.pathname,status:response.status()});});
  const result={path};
  try{
   const response=await page.goto(base+path,{waitUntil:'domcontentloaded',timeout:15000});
   result.status=response.status();
   await page.locator('body').waitFor();
   await page.waitForTimeout(400);
   result.title=await page.title();
   result.text=(await page.locator('body').innerText()).slice(0,180);
   result.views=[];
   for(const width of [1440,768,390,320]){
    await page.setViewportSize({width,height:width>768?1000:844});
    await page.waitForTimeout(60);
    const geometry=await page.evaluate(()=>({width:innerWidth,scrollWidth:document.documentElement.scrollWidth, background:getComputedStyle(document.body).backgroundColor,overflow:[...document.querySelectorAll('body *')].filter(el=>{const r=el.getBoundingClientRect(),s=getComputedStyle(el);return r.width>0&&s.position!=='fixed'&&s.visibility!=='hidden'&&r.right>innerWidth+2&&!el.closest('pre, [hidden], iframe, .app-nav, .home-masthead-links, .prefs-jump-nav, .sidebar, .tabs, table, .calendar-days');}).slice(0,6).map(el=>({tag:el.tagName,id:el.id,class:typeof el.className==='string'?el.className:'',width:Math.round(el.getBoundingClientRect().width)}))}));
    result.views.push(geometry);
    if([1440,390].includes(width))await page.screenshot({path:`${out}/${path.replaceAll('/','_')||'home'}-${width}.png`,timeout:5000});
   }
   result.links=await page.locator('a[href]').evaluateAll(links=>[...new Set(links.map(a=>a.getAttribute('href')).filter(h=>h.startsWith('/')&&!h.startsWith('//')))]);
  }catch(e){result.error=e.message.split('\n')[0];}
  result.errors=[...new Set(errors)];result.failedAssets=[...new Map(failedAssets.map(a=>[a.path,a])).values()];
  results.push(result);await page.close();
 }
 await writeFile(out+'/audit.json',JSON.stringify(results,null,2));
 console.log(JSON.stringify(results.map(r=>({path:r.path,status:r.status,error:r.error,errors:r.errors,missing:r.failedAssets,overflow:r.views?.filter(v=>v.scrollWidth>v.width+1).map(v=>({width:v.width,scrollWidth:v.scrollWidth,elements:v.overflow}))})),null,2));
 const failures = results.filter(r => r.error || r.errors.length || r.failedAssets.length || r.views?.some(v => v.scrollWidth > v.width + 1));
 console.log('Audited',results.length,'pages; report:',out+'/audit.json');
 if (failures.length) process.exitCode = 1;
}finally{await browser.close();}
