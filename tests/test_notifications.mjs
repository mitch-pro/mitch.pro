import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';

const listeners=new Map(),shown=[],navigated=[];
let windows=[];
const origin='https://mitchdog.com';
const self={location:{origin,hostname:'mitchdog.com'},registration:{scope:origin+'/',getNotifications:async({tag})=>shown.filter(n=>n.options.tag===tag).map(n=>({data:n.options.data})),showNotification:async(title,options)=>{shown.push({title,options});}},addEventListener:(type,handler)=>listeners.set(type,handler)};
const clients={matchAll:async()=>windows,openWindow:async url=>navigated.push(url)};
runInNewContext(readFileSync(new URL('../webserver/sw.js',import.meta.url),'utf8'),{self,clients,URL,console,Date});
async function push(data){let work;listeners.get('push')({data:{text:()=>JSON.stringify(data)},waitUntil:promise=>{work=promise}});await work;}
async function click(url){let work,closed=false;listeners.get('notificationclick')({notification:{data:{url},close:()=>{closed=true}},waitUntil:promise=>{work=promise}});await work;assert(closed);}
await push({title:'Message',url:'/matrix/#/room/!a%3Aexample.test/$one',tag:'matrix-a',eventId:'one'});
assert.equal(shown.length,1);assert.equal(shown[0].options.renotify,false);
await push({title:'Message',url:'/matrix/#/room/!a%3Aexample.test/$one',tag:'matrix-a',eventId:'one'});
assert.equal(shown.length,1,'same event must not redisplay an existing notification');
windows=[{url:origin+'/matrix/#/room/!other%3Aexample.test/$x',visibilityState:'visible',focused:true}];
await push({title:'Message',url:'/matrix/#/room/!a%3Aexample.test/$two',tag:'matrix-a',eventId:'two'});
assert.equal(shown.length,2,'a different conversation still alerts');
windows=[{url:origin+'/matrix/#/room/!fixture%3Aexample.test/$old',visibilityState:'visible',focused:true}];
await push({title:'Message',url:'/matrix/#/room/!fixture%3Aexample.test/$new',tag:'matrix-room',eventId:'new'});
assert.equal(shown.length,2,'active Matrix room is quiet regardless of event');
windows=[{url:origin+'/matrix/home/!fixture%3Aexample.test/$old/',visibilityState:'visible',focused:true}];
await push({title:'Message',url:'/matrix/home/!fixture%3Aexample.test/$new/',tag:'cinny-room',eventId:'new-native'});
assert.equal(shown.length,2,'active Cinny room ignores event suffix');
windows=[{url:origin+'/',focused:true,navigate:async url=>{navigated.push(url);return {focus:async()=>{}}}}];
for(const target of ['/coins/','/friends/','/matrix/#/room/!fixture%3Aexample.test/$event']){
  await click(target);assert.equal(navigated.at(-1),origin+target);
}
await click('javascript:alert(1)');assert.equal(navigated.at(-1),origin+'/notifications/');
await click('https://untrusted.example.test/phishing');assert.equal(navigated.at(-1),origin+'/notifications/');
await click('https://mitch.pro/friends/');assert.equal(navigated.at(-1),origin+'/friends/');
console.log('Notification tests passed: duplicate events, active conversations, direct navigation and unsafe URL rejection. No messages sent.');

const peopleContext={window:{}};
runInNewContext(readFileSync(new URL('../webserver/people.js',import.meta.url),'utf8'),peopleContext);
const people=[{displayName:'Offline',online:false,lastActive:999},{displayName:'Away',online:true,status:'away',lastActive:500},{displayName:'Active older',online:true,status:'active',lastActive:100},{displayName:'Active recent',online:true,status:'active',lastActive:200}];
people.sort(peopleContext.window.MitchPeople.compare);
assert.deepEqual(people.map(p=>p.displayName),['Active recent','Active older','Away','Offline']);
assert.equal(peopleContext.window.MitchPeople.rank({role:'admin/developer'}),'Admin');
console.log('Shared presence ordering and account rank labels passed.');
