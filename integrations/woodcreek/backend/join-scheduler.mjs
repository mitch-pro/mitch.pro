import { readFile } from 'node:fs/promises';
import { setTimeout as pause } from 'node:timers/promises';

export async function hasJoinCapacity() {
  async function value(name) {
    try { const raw=(await readFile('/sys/fs/cgroup/'+name,'utf8')).trim(); return raw==='max'?Infinity:Number(raw); }
    catch { return Infinity; }
  }
  const [pids,maxPids,memory,maxMemory]=await Promise.all(['pids.current','pids.max','memory.current','memory.max'].map(value));
  return (!Number.isFinite(maxPids)||!Number.isFinite(pids)||pids+96<maxPids)
    && (!Number.isFinite(maxMemory)||!Number.isFinite(memory)||memory+134217728<maxMemory*.9);
}

// One queue covers every launch and retry, including requests from other tabs.
export class JoinScheduler {
  constructor({concurrency=2,spacingMs=400,capacity=hasJoinCapacity,capacityTimeoutMs=30000}={}) {
    Object.assign(this,{concurrency,spacingMs,capacity,capacityTimeoutMs});
    this.queue=[];this.running=0;this.nextStart=0;this.blockedSince=null;
  }
  run(task,active=()=>true) {
    return new Promise((resolve,reject)=>{this.queue.push({task,active,resolve,reject});this.drain()});
  }
  cancelInactive() {
    this.queue=this.queue.filter(entry=>{if(entry.active())return true;entry.resolve(false);return false});
  }
  drain() {
    this.cancelInactive();
    while(this.running<this.concurrency&&this.queue.length) {
      const entry=this.queue.shift();this.running++;
      this.execute(entry).then(entry.resolve,entry.reject).finally(()=>{this.running--;this.drain()});
    }
  }
  async execute(entry) {
    const start=Math.max(Date.now(),this.nextStart);this.nextStart=start+this.spacingMs;
    while(entry.active()&&Date.now()<start)await pause(Math.min(100,start-Date.now()));
    while(entry.active()) {
      const available=await this.capacity();
      if(!entry.active())return false;
      if(available) {this.blockedSince=null;return entry.task();}
      this.blockedSince??=Date.now();
      if(Date.now()-this.blockedSince>=this.capacityTimeoutMs)throw new Error('The server is at capacity. Stop a bot before adding more.');
      await pause(200);
    }
    return false;
  }
}
