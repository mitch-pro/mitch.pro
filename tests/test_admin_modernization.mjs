import assert from 'node:assert/strict';
import { randomBytes, randomUUID, createHmac, createHash } from 'node:crypto';
import { createServer as httpServer } from 'node:http';
import { createServer as httpsServer } from 'node:https';
import { mkdtemp, mkdir, symlink, readFile, writeFile, rm } from 'node:fs/promises';
import { execFileSync, spawn } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { resolve, join } from 'node:path';
import { once } from 'node:events';
import { chromium } from '@playwright/test';

// All services, accounts, keys and recipients are disposable local fixtures.
const repo = resolve(import.meta.dirname, '..');
const fixture = await mkdtemp('/tmp/mitch-modernization-');
await mkdir(join(fixture,'data'));
await symlink(join(repo,'webserver'),join(fixture,'webserver'));
const secret = randomBytes(32), pass = randomUUID();
await writeFile(join(fixture,'data/id_secret.key'),secret);
const emails = {owner:'owner@example.test',admin:'admin@example.test',coowner:'coowner@example.test',moderator:'moderator@example.test',member:'member@example.test'};
await writeFile(join(fixture,'data/admins.json'),JSON.stringify({owners:[emails.owner],coOwners:[emails.coowner],admins:[emails.admin]}));
await writeFile(join(fixture,'data/moderators.json'),JSON.stringify([emails.moderator]));
execFileSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',join(fixture,'key.pem'),'-out',join(fixture,'cert.pem'),'-days','1','-subj','/CN=localhost'],{stdio:'ignore'});
let vmState='stopped', failUpdate=false, updates=[], policyGuests=null, startupLists=0;
const createdGuests=new Set(), deletedGuests=[], powerCommands=[];
let config={cores:2,memory:4096,scsi0:'fixture:vm-140-disk-0,size=64G',ide2:'fixture:iso/windows.iso,media=cdrom'};
const proxmox=httpsServer({key:await readFile(join(fixture,'key.pem')),cert:await readFile(join(fixture,'cert.pem'))},async(req,res)=>{
  const path = new URL(req.url,'https://localhost').pathname;
  let raw='';for await(const chunk of req)raw+=chunk;
  const form=Object.fromEntries(new URLSearchParams(raw));
  let data={};
  const guestId=Number(path.match(/\/(?:qemu|lxc)\/(\d+)/)?.[1]);
  if(path.endsWith('/qemu') && req.method==='POST') {createdGuests.add(Number(form.vmid));data=null;}
  else if(path.endsWith('/clone')) {createdGuests.add(Number(form.newid));data=null;}
  else if(req.method==='DELETE' && /\/(?:qemu|lxc)\/\d+$/.test(path)) {deletedGuests.push(guestId);createdGuests.delete(guestId);data=null;}
  else if(/\/status\/(start|shutdown|stop|reboot)$/.test(path)) {powerCommands.push({id:guestId,action:path.split('/').at(-1)});data=null;}
  else if(path.endsWith('/config') && guestId===9010) data={template:1,ide2:'fixture:cloudinit',scsi0:'fixture:base-disk,size=64G',net0:'virtio,bridge=vmbr0'};
  else if(path.endsWith('/status/current'))data={status:vmState,maxmem:4294967296,maxdisk:68719476736,cpus:config.cores,uptime:0};
  else if(path.endsWith('/config')){
    if(req.method==='PUT'){
      updates.push(form);
      if(failUpdate){res.writeHead(500,{'content-type':'application/json'});res.end(JSON.stringify({errors:{fixture:'Host rejected update'}}));return;}
      config={...config,...form};data=null;
    }else data=config;
  }else if(path.endsWith('/lxc')) {startupLists++;data=[];}
  else if(path.endsWith('/qemu')) {startupLists++;data=policyGuests || [{vmid:140,name:'Fixture VM',status:vmState,cpus:config.cores,maxmem:Number(config.memory)*1048576,maxdisk:68719476736}];}
  else if(path.endsWith('/status'))data={cpu:0,memory:{used:0,total:34359738368},rootfs:{used:0,total:1099511627776},cpuinfo:{cpus:8}};
  else if(path.endsWith('/resize')){updates.push(form);data=null;}
  else if(path.endsWith('/resources'))data=policyGuests || [{vmid:140,type:'qemu',node:'fixture',name:'Fixture VM',status:vmState},...Array.from(createdGuests,id=>({vmid:id,type:'qemu',node:'fixture',name:'Created fixture',status:'stopped'}))];
  res.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify({data}));
});
proxmox.listen(0,'127.0.0.1');await once(proxmox,'listening');
const probe=httpServer();probe.listen(0,'127.0.0.1');await once(probe,'listening');const port=probe.address().port;await new Promise(r=>probe.close(r));
const base=`http://127.0.0.1:${port}`;
const startServer=()=>spawn(join(repo,'rust/target/debug/mitch-server'),[],{cwd:fixture,stdio:['ignore','ignore','pipe'],env:{...process.env,NODE_ENV:'production',GOD_MODE:'0',MITCH_BASE:fixture,DATA_DIR:join(fixture,'data'),MITCH_DB_PATH:join(fixture,'data/test.db'),PORT:String(port),PROXMOX_HOST:'127.0.0.1',PROXMOX_PORT:String(proxmox.address().port),PROXMOX_NODE:'fixture',PVE_URL:`https://127.0.0.1:${proxmox.address().port}/api2/json`,PVE_NODE:'fixture',PVE_TOKEN:randomUUID(),PVE_SSH_HOST:'127.0.0.1',PVE_SSH_PORT:'1',PROXMOX_TOKEN_ID:randomUUID(),PROXMOX_TOKEN_SECRET:randomUUID(),PROXMOX_VERIFY_TLS:'false',PROXMOX_DESKTOP_TEMPLATES:'9010',CONDUIT_HOST:'127.0.0.1',CONDUIT_PORT:'1',MAIL_RS_URL:'http://127.0.0.1:1',NTFY_TOPIC:'',NTFY_URL:'',VAPID_PRIVATE_KEY:'',RUST_LOG:'error'}});
let child=startServer();
let stderr='';child.stderr.on('data',data=>{stderr+=data});
let browser,db;
const delay=ms=>new Promise(r=>setTimeout(r,ms));
try{
  for(let i=0;;i++){
    try{if((await fetch(base+'/api/access/status')).ok)break;}catch{}
    assert(i<120,stderr);await delay(100);
  }
  db=new DatabaseSync(join(fixture,'data/test.db'));
  const doc=(name,value)=>db.prepare('INSERT INTO json_documents(path,content,updated_at) VALUES(?,?,?) ON CONFLICT(path) DO UPDATE SET content=excluded.content,updated_at=excluded.updated_at').run('data/'+name,JSON.stringify(value),Date.now());
  const sessions={},users={};
  for(const [role,email]of Object.entries(emails)){
    const raw='e'+createHash('sha256').update(email).digest('hex').slice(0,24);
    const sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16),token=randomBytes(32).toString('base64url');
    users[role]={email,sid,token};
    db.prepare('INSERT INTO passwords(email,hash,updated_at) VALUES(?,?,?)').run(email,randomBytes(32).toString('hex'),Date.now());
    db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid,email,email,Date.now());
    db.prepare('INSERT INTO tokens(token,data,updated_at) VALUES(?,?,?)').run(sid,JSON.stringify({email,claimed:true}),Date.now());
    db.prepare('INSERT INTO profiles(email,data,updated_at) VALUES(?,?,?)').run(email,JSON.stringify({username:role,displayName:'Fixture '+role,hasCompletedTutorial:true}),Date.now());
    db.prepare('INSERT INTO coins(email,balance,lifetime_earned,updated_at) VALUES(?,1000,0,?)').run(email,Date.now());
    sessions[createHash('sha256').update(token).digest('hex')]={email,normEmail:email,sid,gen:0,createdAt:Date.now(),lastSeen:Date.now(),expiresAt:Date.now()+86400000,ip:'127.0.0.1',userAgent:'Fixture',devSuperuser:false};
  }
  doc('auth_sessions.json',sessions);
  db.prepare('INSERT INTO virtual_machines(id,owner_email,proxmox_vmid,proxmox_node,hostname,friendly_name,operating_system,cpu_cores,memory_mb,disk_gb,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)').run('fixture-vm',emails.member,140,'fixture','fixture-vm','Fixture VM','Windows',2,4096,64,Date.now(),Date.now());
  browser=await chromium.launch({executablePath:process.env.PLAYWRIGHT_EXECUTABLE_PATH});
  const contexts={};
  for(const [role,user]of Object.entries(users)){
    const context=await browser.newContext({viewport:{width:1440,height:1000},colorScheme:'dark'});
    await context.addCookies([{name:'mitch_session',value:user.token,url:base,httpOnly:true,sameSite:'Lax'}]);
    await context.addInitScript(pass=>{if(window.top!==window)return;sessionStorage.setItem('admin_passphrase',pass);localStorage.setItem('_mitch_cookie_consent','accepted');},pass);
    await context.route('**/*',route=>new URL(route.request().url()).origin===base?route.continue():route.abort());
    await context.route(base+'/sw.js*',route=>route.abort());
    await context.route(base+'/api/matrix/**',route=>route.fulfill({json:{ok:true,rooms:[],staff:[]}}));
    contexts[role]=context;
  }
  const headers={Origin:base,'X-Mitch-Requested-With':'1','X-Admin-Passphrase':pass};
  const post=(role,path,data,extra={})=>contexts[role].request.post(base+path,{headers:{...headers,...extra},data});
  for(const role of ['owner','admin']){const response=await post(role,'/api/admin/passphrase-status',{passphrase:pass});assert(response.ok(),await response.text());}
  const canonical=await contexts.admin.request.get(base+'/admin?tab=access',{maxRedirects:0});assert.equal(canonical.status(),302);assert.equal(canonical.headers().location,'/admin/?tab=access');
  assert.equal((await contexts.member.request.get(base+'/admin/email/')).status(),403);
  assert.equal((await contexts.moderator.request.get(base+'/admin/email/')).status(),403);
  assert.equal((await contexts.member.request.get(base+'/api/admin/newsletters/recipients',{headers})).status(),403);
  assert.equal((await contexts.moderator.request.get(base+'/api/admin/newsletters/recipients',{headers})).status(),403);
  let response=await post('admin','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:6,memoryMb:8192});assert.equal(response.status(),200,await response.text());
  assert.equal(db.prepare('SELECT cpu_cores FROM virtual_machines WHERE id=?').get('fixture-vm').cpu_cores,6);
  vmState='running';response=await post('admin','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:8});assert.equal(response.status(),409);
  vmState='unknown';assert.equal((await post('admin','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:8})).status(),503);
  vmState='stopped';failUpdate=true;assert.equal((await post('admin','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:8})).status(),502);failUpdate=false;
  assert.equal(db.prepare('SELECT cpu_cores FROM virtual_machines WHERE id=?').get('fixture-vm').cpu_cores,6);
  assert.equal((await post('admin','/api/admin/vms/resources',{id:'fixture-vm',diskGb:40})).status(),400);
  assert.equal((await post('member','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:8})).status(),403);
  assert.equal((await post('admin','/api/admin/vms/resources',{id:'fixture-vm',cpuCores:8},{Origin:'https://foreign.example.test'})).status(),403);
  assert.equal((await post('admin','/api/admin/restricted-mode',{email:emails.member})).status(),410);
  assert.equal((await post('member','/api/vm/byo-os/windows-compatibility',{id:'missing-vm',confirmInstallerRepair:true})).status(),404);
  response=await post('member','/api/vm/byo-os/windows-compatibility',{id:'fixture-vm',confirmInstallerRepair:true});assert.equal(response.status(),200,await response.text());
  assert.equal(config.bios,'ovmf');assert.equal(config.machine,'q35');assert.match(config.tpmstate0,/version=v2\.0/);assert.match(config.efidisk0,/pre-enrolled-keys=1/);
  assert.equal(config.scsi0,'fixture:vm-140-disk-0,size=64G');
  console.log('PASS: canonical admin redirect, newsletter role gates, stopped/running/unknown/failed-host resource updates, CSRF, retired shadow bans, Windows repair preserving disks.');
  const errors=[];
  for(const width of [1440,820,390,320]){
    console.log("Browser review",width);
    const page=await contexts.admin.newPage();await page.setViewportSize({width,height:1000});page.on('pageerror',error=>errors.push(error.message));
    await page.goto(base+'/admin/');await page.locator('#newsletter-management').waitFor();assert(await page.locator('#newsletter-management').isVisible());
    if (await page.locator('#admin-passphrase-input').isVisible()) { await page.locator('#admin-passphrase-input').fill(pass); await page.locator('#admin-passphrase-btn').click(); }
    await page.waitForTimeout(300);
    if (await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth)) console.log('Overflow nodes',await page.evaluate(()=>[...document.querySelectorAll('body *')].filter(el=>el.getBoundingClientRect().right>innerWidth+1).slice(0,16).map(el=>({tag:el.tagName,id:el.id,cls:el.className,right:el.getBoundingClientRect().right}))));
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'admin overflow '+width);
    if(width===1440)await page.screenshot({path:'/tmp/mitch-admin-modern.png',fullPage:true});
    await page.goto(base+'/admin/vms/');
    await page.locator('[data-resources]').first().waitFor({state:'attached',timeout:8000}).catch(async error=>{ console.log('VM failure',width,errors,await page.locator('#service-state').innerText(),await page.locator('#fleet-list').innerText()); throw error; });await page.locator('.vm-actions-menu summary').first().click();await page.locator('[data-resources]').first().click();await page.locator('#resource-dialog').waitFor();
    assert.equal(await page.locator('#resource-cpu').inputValue(),'6');await page.locator('#resource-cpu').fill('7');await page.getByRole('button',{name:'Save resources',exact:true}).click();await page.locator('#resource-dialog').waitFor({state:'hidden'});
    assert.equal(db.prepare('SELECT cpu_cores FROM virtual_machines WHERE id=?').get('fixture-vm').cpu_cores,7);
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'VM admin overflow '+width);
    if(width===390)await page.screenshot({path:'/tmp/mitch-admin-vms-modern.png',fullPage:true});
    if (width===390) {
      await page.goto(base+'/admin/bans/');await page.locator('#bans-workspace').waitFor();
      await page.locator('#restriction-account').fill('member');await page.locator('#restriction-reason').fill('Local fixture restriction');await page.locator('#restriction-form button').click();
      await page.locator('#restriction-list').getByRole('button',{name:'Restore VM access'}).waitFor();await page.locator('#restriction-list').getByRole('button',{name:'Restore VM access'}).click();await page.locator('#restriction-list').getByText('No VM restrictions.').waitFor();
      await page.locator('#ban-email').fill('member');await page.locator('#ban-duration').selectOption('1h');await page.locator('#ban-reason').fill('Local fixture account ban');await page.locator('#ban-form button').click();await page.locator('#ban-confirm button[value="ban"]').click();await page.locator('#ban-list').getByRole('button',{name:'Restore access'}).waitFor();await page.locator('#ban-list').getByRole('button',{name:'Restore access'}).click();await page.locator('#ban-list').getByText('No banned accounts.').waitFor();
      assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'ban management overflow');
    }
    await page.close();
    // Reset the fixture VM after checking a real editor submission.
    db.prepare('UPDATE virtual_machines SET cpu_cores=6 WHERE id=?').run('fixture-vm');config.cores=6;
  }
  doc('friend_requests.json',[{from:emails.admin,to:emails.member,timestamp:Date.now()}]);
  const friendsPage=await contexts.member.newPage();await friendsPage.setViewportSize({width:390,height:900});friendsPage.on('pageerror',error=>errors.push(error.message));
  await friendsPage.goto(base+'/friends/');await friendsPage.locator('[data-friend-action="accept"]').click();await friendsPage.locator('.friend-item').waitFor();
  const mutual=JSON.parse(db.prepare('SELECT data FROM friend_lists WHERE email=?').get(emails.member).data);assert(mutual.includes(emails.admin));
  assert.equal((await post('member','/api/friends/request/respond',{email:emails.admin,action:'accept'})).status(),200);
  assert.equal(await friendsPage.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'friends mobile overflow');await friendsPage.close();
  const now=Date.now();
  const firstMessage={id:'notice-one',kind:'dm',from:emails.admin,to:emails.member,text:'Local fixture',ts:now-1000,read:false};
  doc('dms.json',[firstMessage]);
  let inbox=await(await contexts.member.request.get(base+'/api/me/notifications')).json();
  let notice=inbox.notifications.find(n=>n.type==='dm');
  assert(notice.url.includes('message=notice-one'));assert.equal(notice.fromKey,emails.admin);
  doc('dms.json',[firstMessage,{...firstMessage,id:'notice-two',ts:now,read:false}]);
  assert.equal((await post('member','/api/me/notifications/read',{dmFroms:[notice.fromKey],beforeTs:notice.ts})).status(),200);
  inbox=await(await contexts.member.request.get(base+'/api/me/notifications')).json();
  assert(inbox.notifications.find(n=>n.type==='dm').url.includes('message=notice-two'));
  assert.equal((await contexts.member.request.get(base+'/api/me/notifications/read')).status(),405);
  assert.equal((await post('admin','/api/admin/vm-ban',{email:emails.member,banned:true})).status(),200);
  assert.equal((await post('admin','/api/admin/vm-ban',{email:emails.member,banned:true})).status(),200);
  const restrictions=await(await contexts.admin.request.get(base+'/api/admin/bans',{headers})).json();
  assert(restrictions.restrictions.find(row=>row.email===emails.member).vm);
  assert.equal((await post('admin','/api/admin/vm-ban',{email:emails.owner,banned:true})).status(),403);
  assert.equal((await post('admin','/api/admin/vm-ban',{email:emails.member,banned:false})).status(),200);
  doc('applications.json',[{email:emails.member,status:'approved',type:'premium'}]);
  const computersFor=async(role)=>(await(await contexts[role].request.get(base+'/api/vm/computers')).json());
  for(const role of ['owner','admin','coowner']) {
    assert.equal((await computersFor(role)).canCreateAdditional,true);
    const seedId=150+['owner','admin','coowner'].indexOf(role);
    db.prepare('INSERT INTO virtual_machines(id,owner_email,proxmox_vmid,proxmox_node,hostname,friendly_name,operating_system,cpu_cores,memory_mb,disk_gb,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)').run('existing-'+role,emails[role],seedId,'fixture','seed-fixture','Existing fixture','Linux',2,4096,64,1,1);
    const created=await post(role,'/api/vm/my-computer/create',{});
    assert.equal(created.status(),201,await created.text());
    assert.equal((await computersFor(role)).computers.length,2);
  }
  assert.equal((await computersFor('member')).canCreateAdditional,false);
  assert.equal((await computersFor('moderator')).canCreateAdditional,false);
  assert.equal((await post('member','/api/vm/my-computer/create',{})).status(),409);
  const ownerComputers=(await computersFor('owner')).computers;
  const adminComputers=(await computersFor('admin')).computers;
  assert.equal((await post('admin','/api/vm/my-computer/recreate',{})).status(),409);
  assert.equal((await post('admin','/api/vm/my-computer/recreate',{id:ownerComputers[0].id})).status(),404);
  assert.equal(deletedGuests.length,0,'Adding staff computers must never delete existing guests');
  doc('byo_os_isos.json',{[emails.coowner]:{filename:'fixture.iso',proxmoxFilename:'fixture.iso',sizeBytes:1048576}});
  const fromIso=await post('coowner','/api/vm/byo-os/create',{});
  assert.equal(fromIso.status(),201,await fromIso.text());
  assert.equal((await computersFor('coowner')).computers.length,3);
  assert.equal(deletedGuests.length,0,'Staff ISO creation must preserve other computers');

  const replacement=await post('admin','/api/vm/my-computer/recreate',{id:adminComputers[0].id});
  assert.equal(replacement.status(),201,await replacement.text());
  assert.equal(deletedGuests.length,1);
  assert(db.prepare('SELECT id FROM virtual_machines WHERE id=?').get(adminComputers[1].id),'Recreation must preserve the other VM');
  assert.equal((await computersFor('admin')).computers.length,2);
  assert.equal((await post('owner','/api/vm/owner-resources',{category:'cpu',value:3})).status(),409);
  const untouched=ownerComputers[1];
  const changed=await post('owner','/api/vm/owner-resources',{id:ownerComputers[0].id,category:'cpu',value:3});
  assert.equal(changed.status(),200,await changed.text());
  assert.equal(db.prepare('SELECT cpu_cores FROM virtual_machines WHERE id=?').get(untouched.id).cpu_cores,untouched.cpuCores);
  for(const [role,width]of [['admin',390],['owner',1440],['member',390]]) {
    const page=await contexts[role].newPage();await page.setViewportSize({width,height:950});
    page.on('pageerror',error=>errors.push(error.message));await page.goto(base+'/vms/');
    await page.locator('#computer-grid .computer-card').first().waitFor();
    assert.equal(await page.locator('#add-computer-button').isVisible(),role!=='member');
    if(role!=='member') {
      await page.locator('#add-computer-button').click();assert(await page.locator('#provision-dialog').isVisible());await page.locator('#provision-cancel').click();
      const card=page.locator('.computer-card').first();
      await card.locator('.more-menu summary').click();await card.locator('[data-action="recreate"]').click();
      const request=page.waitForRequest(req=>req.url().endsWith('/api/vm/my-computer/recreate'));
      await page.route(base+'/api/vm/my-computer/recreate',route=>route.fulfill({status:409,json:{error:'Fixture: recreation paused'}}));
      await page.locator('#provision-submit').click();
      assert.equal((await request).postDataJSON().id,await card.getAttribute('data-id'),'Recreate UI must send the selected VM ID');
    }
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth),false,'Multi-VM portal overflow');await page.close();
  }
  // Old, unattended guests exercise the actual 15-second watchdog, including
  // the original 30-minute staff timeout. Every guest is a local mock.
  const oldGuests=[['owner',181],['admin',182],['member',183],['coowner',184]];
  for(const [role,id]of oldGuests) db.prepare('INSERT INTO virtual_machines(id,owner_email,proxmox_vmid,proxmox_node,hostname,friendly_name,operating_system,cpu_cores,memory_mb,disk_gb,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)').run('unattended-'+role,emails[role],id,'fixture','old-fixture','Old fixture','Linux',2,4096,64,1,1);
  policyGuests=oldGuests.map(([role,id])=>({vmid:id,node:'fixture',type:'qemu',name:'Old '+role,status:'running',uptime:86400}));
  const deadline=Date.now()+20000;
  while(!powerCommands.some(command=>command.id===183&&command.action==='shutdown')&&Date.now()<deadline)await delay(100);
  assert(powerCommands.some(command=>command.id===183&&command.action==='shutdown'),'Ordinary idle VM should still be shut down');
  assert(!powerCommands.some(command=>[181,182,184].includes(command.id)),'Owner and admin VMs must stay running while unattended');
  console.log('PASS: staff multiple VMs, additive creation, targeted recreation/resources, portal controls, and real watchdog exemptions.');
  const registered=db.prepare('SELECT proxmox_vmid AS vmid FROM virtual_machines WHERE proxmox_vmid BETWEEN 200 AND 209').all();
  assert(registered.length>0,'Restart fixture must cover overlapping persistent/disposable VM IDs');
  policyGuests=registered.map(({vmid})=>({vmid,type:'qemu',node:'fixture',name:'Registered fixture',status:'stopped'}));
  const deletesBeforeRestart=deletedGuests.length, listsBeforeRestart=startupLists;
  let stopped=once(child,'exit');child.kill('SIGTERM');await stopped;
  child=startServer();child.stderr.on('data',data=>{stderr+=data});
  const restartDeadline=Date.now()+10000;
  while(startupLists<listsBeforeRestart+2&&Date.now()<restartDeadline)await delay(100);
  assert(startupLists>=listsBeforeRestart+2,'Restart must exercise both legacy guest lists');
  await delay(500);
  assert.equal(deletedGuests.length,deletesBeforeRestart,'Application restart must never delete a registered VM');
  console.log('PASS: application restart preserves registered computers in the former ephemeral range.');
  assert.deepEqual(errors,[]);
  console.log('PASS: admin dashboard and resource editor at desktop/tablet/mobile sizes; bans, friends, and notifications. No delivery endpoints called; notification fixtures stay in the local database.');
}finally{
  db?.close();await browser?.close();const stopped=once(child,'exit');child.kill('SIGTERM');await stopped;
  await new Promise(r=>proxmox.close(r));await rm(fixture,{recursive:true,force:true});
}
