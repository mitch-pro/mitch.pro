// Full account handoff against a real local Mitch server in production auth mode.
import assert from 'node:assert/strict';
import { createAccountAuth } from '../integrations/woodcreek/backend/woodcreek-auth.mjs';
import { createServer } from 'node:http';
import { spawn } from 'node:child_process';
import { createHash, createHmac, randomBytes } from 'node:crypto';
import { mkdir, mkdtemp, readFile, rm, symlink } from 'node:fs/promises';
import { DatabaseSync } from 'node:sqlite';
import { resolve } from 'node:path';
const root = resolve(import.meta.dirname, '..'), fixture = await mkdtemp('/tmp/tylerium-accounts-');
await mkdir(fixture+'/data'); await symlink(root+'/webserver',fixture+'/webserver');
const reserve=createServer();await new Promise(r=>reserve.listen(0,'127.0.0.1',r));const port=reserve.address().port;await new Promise(r=>reserve.close(r));
const provider='http://127.0.0.1:'+port;
let server, wood;
const start=()=>spawn(root+'/rust/target/debug/mitch-server',[],{cwd:fixture,stdio:'ignore',env:{...process.env,MITCH_BASE:fixture,DATA_DIR:fixture+'/data',MITCH_DB_PATH:fixture+'/data/mitchpro.db',PORT:String(port),NODE_ENV:'production',GOD_MODE:'0',DEV_TEST_ACCESS:'0',VAPID_PUBLIC_KEY:'',RECAPTCHA_SITE_KEY:'',PVE_URL:'https://127.0.0.1:1',PROXMOX_HOST:'127.0.0.1',PROXMOX_PORT:'1'}});
const wait=ms=>new Promise(r=>setTimeout(r,ms));
async function ready(){for(let i=0;i<150;i++){try{if((await fetch(provider+'/enroll/')).ok)return}catch{}await wait(100)}throw new Error('Mitch startup');}
const hash=value=>createHash('sha256').update(value).digest('hex');
const cookieHeaders=response=>response.headers.getSetCookie();
const cookieValue=(response,name)=>cookieHeaders(response).find(value=>value.startsWith(name+'='))?.split(';')[0];
const post=(path,body={},token)=>fetch(provider+'/api/woodcreek/auth/'+path,{method:'POST',headers:{'Content-Type':'application/json',...(token?{Authorization:'Bearer '+token}:{})},body:JSON.stringify(body)});
try {
  server=start();await ready();
  const secret=await readFile(fixture+'/data/id_secret.key');
  const email='tylerium-alice@example.test',raw='e'+hash(email).slice(0,24),sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16),session=randomBytes(32).toString('hex');
  const db=new DatabaseSync(fixture+'/data/mitchpro.db');
  db.prepare('INSERT INTO names(sid,email,norm_email,created_at) VALUES(?,?,?,?)').run(sid,email,email,Date.now());
  db.prepare('INSERT INTO passwords(email,hash,updated_at) VALUES(?,?,?)').run(email,'fixture-password-hash',Date.now());
  db.prepare('INSERT INTO profiles(email,data,updated_at) VALUES(?,?,?)').run(email,JSON.stringify({username:'alice',displayName:'Alice Example',pfp:'https://example.test/alice.png',totpSecret:'must-not-leak'}),Date.now());
  db.prepare('INSERT INTO json_documents(path,content,updated_at) VALUES(?,?,?)').run('data/auth_sessions.json',JSON.stringify({[hash(session)]:{email,normEmail:email,sid,gen:0,createdAt:Date.now(),lastSeen:Date.now(),expiresAt:Date.now()+86400000}}),Date.now());db.close();
  const auth=createAccountAuth({provider});
  wood=createServer(async(req,res)=>{const url=new URL(req.url,'https://woodcreek.site');if(await auth.handle(req,res,url))return;res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({ok:true}));});
  await new Promise(r=>wood.listen(0,'127.0.0.1',r));const base='http://127.0.0.1:'+wood.address().port;
  assert.equal((await fetch(base+'/api/state')).status,401);
  const begin=await fetch(base+'/auth/login?next=%2F%3Ftab%3Dnames',{redirect:'manual'});
  assert.equal(begin.status,303);
  const flow=cookieValue(begin,'__Host-tylerium_flow'); assert(flow);
  assert(cookieHeaders(begin).every(header=>header.includes('HttpOnly')&&header.includes('Secure')&&header.includes('SameSite=Lax')&&!header.includes('Domain=')));
  const authorize=new URL(begin.headers.get('location'));
  const anonymous=await fetch(authorize,{redirect:'manual'});assert.equal(anonymous.status,302);assert(anonymous.headers.get('location').startsWith('/enroll/?next='));assert(decodeURIComponent(anonymous.headers.get('location')).includes('/api/woodcreek/auth/authorize?'));
  const forged=await fetch(authorize,{headers:{Cookie:'id='+sid},redirect:'manual'});assert(forged.headers.get('location').startsWith('/enroll/'),'Display-only IDs cannot authenticate production');
  const approved=await fetch(authorize,{headers:{Cookie:'mitch_session='+session},redirect:'manual'});assert.equal(approved.status,302);
  const callback=new URL(approved.headers.get('location'));assert.equal(callback.origin,'https://woodcreek.site');
  const code=callback.searchParams.get('code');assert(code);
  const flowData=JSON.parse(Buffer.from(flow.split('=')[1],'base64url').toString());
  assert.equal((await post('token',{code,code_verifier:randomBytes(32).toString('base64url')})).status,400,'PKCE mismatch must fail');
  const badState=new URL(callback);badState.searchParams.set('state','wrong');
  const mismatched=await fetch(base+badState.pathname+badState.search,{headers:{Cookie:flow},redirect:'manual'});assert.equal(mismatched.headers.get('location'),'/?signin=expired');
  // Codes survive a normal production deployment/restart.
  server.kill('SIGTERM');await new Promise(r=>server.once('exit',r));server=start();await ready();
  const returned=await fetch(base+callback.pathname+callback.search,{headers:{Cookie:flow},redirect:'manual'});assert.equal(returned.status,303);assert.equal(returned.headers.get('location'),'/?tab=names');
  const ownCookie=cookieValue(returned,'__Host-tylerium_session');assert(ownCookie);
  const token=ownCookie.slice(ownCookie.indexOf('=')+1);assert(token.startsWith('wc1.'));assert(!token.includes(session)&&!token.includes(sid));
  assert.equal((await post('token',{code,code_verifier:flowData.verifier})).status,400,'Codes must be one-use');
  let account=await fetch(base+'/api/account',{headers:{Cookie:ownCookie}});assert.equal(account.status,200);account=await account.json();assert.equal(account.account.displayName,'Alice Example');assert.equal(account.account.pfp,'https://example.test/alice.png');assert(!JSON.stringify(account).includes('totpSecret'));
  assert.equal((await fetch(base+'/api/state',{headers:{Cookie:ownCookie}})).status,200);
  assert.equal((await fetch(base+'/api/start',{method:'POST',headers:{Cookie:ownCookie,Origin:'https://evil.example','X-Mitch-Requested-With':'1'}})).status,403);
  assert.equal((await post('session',{},token.slice(0,-3)+'bad')).status,401,'Tampered grants must fail');
  assert.equal((await post('session',{},'wc1.AA')).status,401,'Malformed grants cannot panic');
  assert.equal((await fetch(base+'/auth/logout',{method:'POST',headers:{Cookie:ownCookie,Origin:'https://evil.example'}})).status,403);
  const signedOut=await fetch(base+'/auth/logout',{method:'POST',headers:{Cookie:ownCookie,Origin:'https://woodcreek.site'}});assert.equal(signedOut.status,200);assert(cookieHeaders(signedOut)[0].includes('Max-Age=0'));
  assert.equal((await post('session',{},token)).status,401,'Woodcreek sign-out revokes its grant');
  // Issue two codes then race exchanges: exactly one exchange wins for each code.
  const authorizeAgain=await fetch(authorize,{headers:{Cookie:'mitch_session='+session},redirect:'manual'});const again=new URL(authorizeAgain.headers.get('location'));
  const exchanges=await Promise.all([post('token',{code:again.searchParams.get('code'),code_verifier:flowData.verifier}),post('token',{code:again.searchParams.get('code'),code_verifier:flowData.verifier})]);assert.deepEqual(exchanges.map(r=>r.status).sort(),[200,400]);
  const activeGrant=await exchanges.find(r=>r.status===200).json();
  assert.equal((await post('session',{},activeGrant.access_token)).status,200);
  const logout=await fetch(provider+'/api/logout',{method:'POST',headers:{Cookie:'mitch_session='+session,Origin:provider,'X-Mitch-Requested-With':'1'}});assert.equal(logout.status,200);
  assert.equal((await post('session',{},activeGrant.access_token)).status,401,'Mitch logout invalidates the linked Woodcreek session');
  console.log('PASS: production-mode shared accounts, real profile, PKCE/state, restart persistence, atomic single-use codes, scoped grants, cookie isolation, CSRF, tamper rejection, and linked logout. Local fixtures only.');
} finally {
  if(server&&!server.killed){server.kill('SIGTERM');await new Promise(r=>server.once('exit',r));}
  if(wood){wood.closeAllConnections();await new Promise(r=>wood.close(r));}
  await rm(fixture,{recursive:true,force:true});
}
