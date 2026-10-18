import assert from 'node:assert/strict';
import {chromium} from '@playwright/test';
import {BlooketInstanceManager} from '../integrations/woodcreek/backend/blooket-manager.mjs';
import {advanceFeedback} from '../integrations/woodcreek/backend/blooket-answers.mjs';
import {JoinScheduler} from '../integrations/woodcreek/backend/join-scheduler.mjs';
const pause=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const browser=await chromium.launch({headless:true});
const manager=new BlooketInstanceManager();manager.answerSpeedMode='instant';manager.accuracyRate=100;manager.streakBreaker=0;
const question={questionText:'What is 2 + 2?',answers:['3','4'],correctAnswers:['4'],questionType:'mc'};
async function fixture(page,text=question.questionText,answers=question.answers){
  await page.setContent('<div class="questionText"></div><div class="answersHolder"><button></button><button></button></div>');
  await page.evaluate(({text,answers})=>{window.clicks=0;document.querySelector('.questionText').textContent=text;[...document.querySelectorAll('button')].forEach((button,index)=>{button.textContent=answers[index];button.onclick=()=>{window.clicks++;window.chosen=button.textContent;document.querySelector('.questionText').textContent='Next question';}})},{text,answers});
}
try{
  const page=await browser.newPage();await page.route('**/*',route=>route.abort());await fixture(page);
  const instance=manager.createBotInstance('one','Alex','cat','123456');instance.page=page;instance.status='game';instance.targetAccuracy=100;
  await manager.performBotAnswer(instance,question);assert.equal(await page.evaluate(()=>clicks),0,'Answering starts off');
  manager.updateSettings({autoAnswer:true});
  await Promise.all([manager.performBotAnswer(instance,question),manager.performBotAnswer(instance,question)]);
  assert.equal(await page.evaluate(()=>clicks),1,'One answer must cause one click');assert.equal(await page.evaluate(()=>chosen),'4');assert.equal(instance.questionsAnswered,1);
  await pause(180);assert.equal(await page.evaluate(()=>clicks),1,'No blind clicks on the next screen');
  await fixture(page,'Different question',['yes','no']);instance.lastAnswerAttemptKey=null;
  await manager.performBotAnswer(instance,question);assert.equal(await page.evaluate(()=>clicks),0,'A stale question cannot answer a new screen');
  await page.setContent('<input placeholder="Game ID"><div class="questionText">What is 2 + 2?</div><div class="answersHolder"><button>3</button><button>4</button></div>');
  await manager.performBotAnswer(instance,question);assert.equal(instance.questionsAnswered,1,'Join fields cannot be treated as answer inputs');
  await fixture(page);instance.lastAnswerAttemptKey=null;manager.answerSpeedMode='human';manager.answerDelayMinMs=100;manager.answerDelayMaxMs=100;manager.adaptiveReadingTime=false;
  const pending=manager.performBotAnswer(instance,question);await pause(20);manager.updateSettings({autoAnswer:false});await pending;
  assert.equal(await page.evaluate(()=>clicks),0,'Stop answering cancels a pending answer');
  await page.setContent('<button class="nextButton">Continue</button>');
  await page.evaluate(()=>{window.advances=0;document.querySelector('button').onclick=()=>{window.advances++;document.querySelector('button').outerHTML='<button class="answerContainer">Next answer</button>'}});
  await advanceFeedback(page);assert.equal(await page.evaluate(()=>advances),1,'Feedback advances once without clicking the next question');
  // All launch/retry callers share the scheduler and its resource admission check.
  let running=0,peak=0,admitted=true,started=0;
  const scheduler=new JoinScheduler({spacingMs:0,concurrency:2,capacity:async()=>admitted});
  await Promise.all(Array.from({length:8},()=>scheduler.run(async()=>{running++;started++;peak=Math.max(peak,running);await pause(15);running--})));
  assert.equal(peak,2);assert.equal(started,8);
  admitted=false;const guarded=new JoinScheduler({spacingMs:0,capacity:async()=>admitted,capacityTimeoutMs:5});let entered=false;
  await assert.rejects(guarded.run(async()=>{entered=true}),/at capacity/);assert.equal(entered,false);
  let active=true;const cancelled=guarded.run(async()=>{entered=true},()=>active);active=false;guarded.cancelInactive();await cancelled;assert.equal(entered,false);
  assert.equal(manager.fleetCount({count:2.8}),2);
  assert.throws(()=>manager.validateStart({pin:'123456',count:50}),/slots are available/);
  const retryManager=new BlooketInstanceManager();retryManager.joinScheduler=new JoinScheduler({spacingMs:0,capacity:async()=>true});
  const connected=retryManager.createBotInstance('connected','Alex','cat','123456');connected.status='lobby';
  const retry=retryManager.createBotInstance('retry','Alex','cat','123456');retry.status='error';
  retryManager.executeBotJoin=async instance=>{instance.status='lobby'};
  await retryManager.retryBot('retry');assert.notEqual(retry.name,connected.name,'A retry must not take over another connection with the same nickname');
  for(let i=0;i<48;i++)retryManager.instances.set('active-'+i,{id:'active-'+i,name:'Name'+i,pin:'123456',status:'lobby'});
  retry.status='error';retryManager.instances.set('extra',{id:'extra',name:'Extra',pin:'123456',status:'lobby'});
  await assert.rejects(retryManager.retryBot('retry'),/50 bots/);
  console.log('PASS: explicit answer activation, one answer per question, stale/join-screen protection, immediate pause, shared join limits, capacity rejection, cancellation, and total bot cap. No external requests.');
}finally{await browser.close()}
