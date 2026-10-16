import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {spawn,execFileSync} from 'node:child_process';
import {mkdtemp,mkdir,copyFile,symlink,readFile,writeFile,rm,access} from 'node:fs/promises';
import {createHash,createHmac,randomBytes} from 'node:crypto';
import {resolve} from 'node:path';
import {openSync} from 'node:fs';

const source=process.env.BANS_TEST_FIXTURE || '/tmp/mitch-bans-audit';
assert(source.startsWith('/tmp/mitch-bans-'),'Use an isolated ban fixture.');
const root=await mkdtemp('/tmp/mitch-bans-matrix-');
await mkdir(root+'/data/e2e_attachments',{recursive:true});
await copyFile(source+'/data/id_secret.key',root+'/data/id_secret.key');
await copyFile(source+'/data/admins.json',root+'/data/admins.json');
await symlink(resolve('webserver'),root+'/webserver');
const sql=(script,...args)=>execFileSync('python3',['-c','import sqlite3,json,sys,time; c=sqlite3.connect(sys.argv[1]); '+script,root+'/data/mitchpro.db',...args],{encoding:'utf8'}).trim();
execFileSync('python3',['-c','import sqlite3,sys; a=sqlite3.connect(sys.argv[1]); b=sqlite3.connect(sys.argv[2]); a.backup(b); b.close(); a.close()',source+'/data/mitchpro.db',root+'/data/mitchpro.db']);
const email='banstudent@student.rjuhsd.us',uid='@fixture_student:mitch.pro',now=Date.now();
const secret=await readFile(root+'/data/id_secret.key');
const raw='e'+createHash('sha256').update('banadmin@example.com').digest('hex').slice(0,24);
const sid=raw+'.'+createHmac('sha256',secret).update(raw).digest('hex').slice(0,16);
const token=randomBytes(32).toString('base64url'),key=createHash('sha256').update(token).digest('hex');
const account={email:'banadmin@example.com',normEmail:'banadmin@example.com',sid,gen:0,createdAt:now,lastSeen:now,expiresAt:now+86400000,devSuperuser:false};
const docs={
 'data/matrix_users.json':{[email]:'fixture_student'},
 'data/dms.json':[{from:email,ts:1,image:'private.gif'},{from:'neighbor@example.com',ts:1,text:'Keep this'}],
 'data/pickle_dms.json':[{from:email,ts:1,text:'Remove this'}],
 'data/gif_favorites.json':{[email]:['private.gif'],'neighbor@example.com':['keep.gif']},
 'data/e2e_attachments.json':{bad:{user:email,file:'private.gif',createdAt:1},keep:{user:'neighbor@example.com',file:'keep.gif',createdAt:1}},
 'data/auth_sessions.json':{[key]:account}
};
sql("c.execute('DELETE FROM blacklist'); c.execute('DELETE FROM account_chat_removals'); c.execute('DELETE FROM account_chat_removal_ids'); c.execute('DELETE FROM moderated_media'); c.execute('DELETE FROM chat_files_to_remove'); docs=json.loads(sys.argv[2]); c.executemany('INSERT OR REPLACE INTO json_documents(path,content,updated_at) VALUES(?,?,?)', [(k,json.dumps(v),1) for k,v in docs.items()]); c.commit(); c.close()",JSON.stringify(docs));
await writeFile(root+'/data/e2e_attachments/private.gif','remove');
await writeFile(root+'/data/e2e_attachments/keep.gif','keep');
let failLeftRoom=true,redactions=[],requests=[];
const event=(id,ts,content={body:id})=>({event_id:id,sender:uid,origin_server_ts:ts,type:'m.room.message',content});
const history=[event('$gif',1,{body:'private.gif',msgtype:'m.image',url:'mxc://mitch.pro/private-gif'}),event('$future',now+3600000),{...event('$neighbor',1),sender:'@neighbor:mitch.pro'}];
const mock=createServer(async(req,res)=>{
 let body='';for await(const chunk of req)body+=chunk;
 const path=decodeURIComponent(new URL(req.url,'http://localhost').pathname);requests.push(path);
 const send=(status,value)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));};
 if(path.endsWith('/login'))return send(200,{access_token:JSON.parse(body).identifier.user==='mitch_admin'?'admin-fixture':'user-fixture',user_id:uid});
 if(path.endsWith('/sync'))return send(200,{next_batch:'fixture',rooms:{join:{'!joined:mitch.pro':{}},leave:{'!left:mitch.pro':{}}}});
 if(path.endsWith('/messages'))return send(200,{chunk:history,end:'end'});
 if(path.includes('/redact/')){
  if(path.includes('!left')&&failLeftRoom)return send(403,{errcode:'M_FORBIDDEN'});
  redactions.push(path);return send(200,{event_id:'$redaction'});
 }
 if(path.endsWith('/whoami'))return send(200,{user_id:'@fixture_admin:mitch.pro'});
 if(path.includes('/download/'))return send(200,{unexpected:'Media should have been blocked'});
 return send(404,{errcode:'M_NOT_FOUND'});
});
await new Promise(resolve=>mock.listen(0,'127.0.0.1',resolve));
const mockPort=String(mock.address().port),port=process.env.CHAT_CLEANUP_TEST_PORT || '4331',base='http://127.0.0.1:'+port;
let child;
async function start(){
 const env={...process.env,NODE_ENV:'test',GOD_MODE:'0',MITCH_BASE:root,PORT:port,CONDUIT_HOST:'127.0.0.1',CONDUIT_PORT:mockPort,RUST_LOG:'error'};
 delete env.MITCH_DB_PATH;delete env.DATA_DIR;
 child=spawn(resolve('rust/target/debug/mitch-server'),[],{env,stdio:['ignore',openSync('/tmp/mitch-chat-cleanup-child.log','a'),openSync('/tmp/mitch-chat-cleanup-child.log','a')]});
 await until(async()=>{try{return (await fetch(base+'/api/access/status')).ok;}catch{return false;}});
}
async function stop(){if(child&&!child.killed){const done=new Promise(resolve=>child.once('exit',resolve));child.kill('SIGTERM');await done;}}
async function until(check){for(let i=0;i<300;i++){if(await check())return;await new Promise(r=>setTimeout(r,100));}throw new Error('Timed out waiting for cleanup');}
const headers={'Content-Type':'application/json','X-Mitch-Requested-With':'1','X-Admin-Passphrase':'FixtureBansPassphrase37','Cookie':'mitch_session='+token,'Origin':base};
try{
 await start();
 const ban=await fetch(base+'/api/admin/ban-account',{method:'POST',headers,body:JSON.stringify({email,reason:'Fixture cleanup',duration:'1d'})});
 assert.equal(ban.status,200,await ban.text());
 const readDoc=path=>JSON.parse(sql("print(c.execute('SELECT content FROM json_documents WHERE path=?',(sys.argv[2],)).fetchone()[0]); c.close()",path));
 assert.equal(readDoc('data/dms.json').length,1);
 assert.equal(readDoc('data/pickle_dms.json').length,0);
 assert.equal(readDoc('data/gif_favorites.json')[email],undefined);
 assert.deepEqual(readDoc('data/gif_favorites.json')['neighbor@example.com'],['keep.gif']);
 await assert.rejects(access(root+'/data/e2e_attachments/private.gif'));
 await access(root+'/data/e2e_attachments/keep.gif');
 await stop();await start();
 await until(()=>sql("print(c.execute('SELECT last_error FROM account_chat_removals WHERE email=?',(sys.argv[2],)).fetchone()[0]); c.close()",email).includes('403'));
 assert.equal(sql("print(c.execute('SELECT matrix_done FROM account_chat_removals WHERE email=?',(sys.argv[2],)).fetchone()[0]); c.close()",email),'0','A failed server redaction must stay pending.');
 sql("c.execute('UPDATE json_documents SET content=? WHERE path=?',(sys.argv[2],'data/matrix_users.json')); c.commit(); c.close()",JSON.stringify({[email]:'changed_handle'}));
 const filtered=await fetch(base+'/_matrix/client/v3/rooms/!joined:mitch.pro/messages',{headers:{Cookie:'mitch_session='+token}});
 const events=await filtered.json();
 assert.deepEqual(events.chunk[0].content,{});
 assert.equal(events.chunk[1].content.body,'$future');
 assert.equal(events.chunk[2].content.body,'$neighbor');
 assert.equal((await fetch(base+'/_matrix/media/v3/download/mitch.pro/private-gif')).status,404);
 sql("c.execute('UPDATE json_documents SET content=? WHERE path=?',(sys.argv[2],'data/matrix_users.json')); c.commit(); c.close()",JSON.stringify({[email]:'fixture_student'}));
 await stop();failLeftRoom=false;await start();
 await until(()=>sql("print(c.execute('SELECT matrix_done FROM account_chat_removals WHERE email=?',(sys.argv[2],)).fetchone()[0]); c.close()",email)==='1');
 assert(redactions.some(path=>path.includes('!left')),'Retry must finish the left-room redaction.');
 assert(!redactions.some(path=>path.includes('future')||path.includes('neighbor')),'Other users and post-unban history must survive.');
 console.log('PASS: ban atomically purges messages, GIF favorites and owned files; Matrix tombstones cover history and media; failed redactions survive restart and retry; neighbors and newer messages remain.');
}catch(error){
 console.error('Fixture jobs:',sql("print(c.execute('SELECT email,matrix_done,last_error FROM account_chat_removals').fetchall()); c.close()"));console.error('Mock paths:',requests);throw error;
}finally{
 await stop();await new Promise(resolve=>mock.close(resolve));await rm(root,{recursive:true,force:true});
}
