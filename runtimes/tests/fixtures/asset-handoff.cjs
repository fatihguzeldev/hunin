'use strict';
const { Worker, isMainThread, parentPort, workerData, threadId } = require('node:worker_threads');
const fs = require('node:fs');
const path = require('node:path');
const inspector = require('node:inspector');
const clock = () => process.hrtime.bigint().toString();
function unrelatedWork() {
  const values = new Uint32Array(1 << 20);
  let checksum = 0;
  for (let pass = 0; pass < 32; pass++) {
    for (let i = 0; i < values.length; i++) values[i] = (i * 17 + pass) >>> 0;
    checksum = (checksum + values[(pass * 997) & (values.length - 1)]) >>> 0;
  }
  return checksum;
}
async function worker() {
  const session = new inspector.Session(); session.connect();
  const post = (method, params = {}) => new Promise((resolve,reject) => session.post(method,params,(error,result) => error ? reject(error) : resolve(result)));
  await post('Profiler.enable'); await post('Profiler.setSamplingInterval',{interval:1000}); await post('Profiler.start');
  const readyNs=clock();
  parentPort.postMessage({type:'ready',thread_id:threadId,role:workerData.role,source_ready_ns:readyNs});
  parentPort.once('message',async message => {
    const events=[{kind:'dequeue',id:message.id,thread_id:threadId,role:workerData.role,monotonic_ns:clock()}];
    let output, failure;
    try {
      events.push({kind:'start',id:message.id,thread_id:threadId,role:workerData.role,monotonic_ns:clock()});
      output=workerData.role==='preview' ? require(workerData.preview).run() : unrelatedWork();
    } catch(error) {failure=String(error.stack||error);}
    events.push({kind:'finish',id:message.id,thread_id:threadId,role:workerData.role,monotonic_ns:clock()});
    const {profile}=await post('Profiler.stop');session.disconnect();
    const profilePath=path.join(process.cwd(),`worker-profile-${threadId}.json`);
    const descriptor=fs.openSync(profilePath,'wx',0o600);
    try {fs.writeFileSync(descriptor,JSON.stringify(profile));fs.fsyncSync(descriptor);} finally {fs.closeSync(descriptor);}
    parentPort.postMessage({type:'result',id:message.id,thread_id:threadId,role:workerData.role,profile_path:profilePath,source_ready_ns:readyNs,events,output,failure});parentPort.close();
  });
}
async function run() {
  const workers=[];const events=[];
  try {
    const jobs=[{role:'preview',id:'preview-job-1'},{role:'unrelated',id:'unrelated-job-1'}].map(job=>{
      const child=new Worker(__filename,{workerData:{role:job.role,preview:path.join(__dirname,'asset-preview.cjs')}});workers.push(child);
      return new Promise((resolve,reject)=>{
        const timer=setTimeout(()=>reject(new Error('worker job deadline exceeded')),15000);
        child.once('error',error=>{clearTimeout(timer);reject(error);});
        child.on('message',message=>{
          if(message.type==='ready'){
            events.push({kind:'enqueue',id:job.id,thread_id:threadId,destination_thread_id:message.thread_id,role:job.role,monotonic_ns:clock()});
            child.postMessage({id:job.id});
          } else if(message.type==='result') {clearTimeout(timer);if(message.failure)reject(new Error(message.failure));else resolve(message);}
        });
        child.once('exit',code=>{if(code!==0){clearTimeout(timer);reject(new Error(`worker exited ${code}`));}});
      });
    });
    const results=await Promise.all(jobs);
    for(const result of results)events.push(...result.events);
    return {manifest:results.find(result=>result.role==='preview').output,unrelated_checksum:results.find(result=>result.role==='unrelated').output,
      events,worker_sources:results.map(result=>({thread_id:result.thread_id,role:result.role,profile_path:result.profile_path,source_ready_ns:result.source_ready_ns,sampling_interval_us:1000,
        scope:'worker inspector Session; Node worker id is not an OS thread id',operation:result.id}))};
  } finally {await Promise.all(workers.map(child=>child.terminate()));}
}
if(!isMainThread)worker().catch(error=>{console.error(error);process.exitCode=1;parentPort.close();});
module.exports={run};
