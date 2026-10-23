import inspector from 'node:inspector';
import { writeFileSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { createRequire, syncBuiltinESMExports } from 'node:module';
import { fileURLToPath } from 'node:url';
import { randomUUID } from 'node:crypto';
import process from 'node:process';
const require = createRequire(import.meta.url);
const threads = require('node:worker_threads');
const preload = fileURLToPath(import.meta.url);
const directory = process.env.HUNIN_JS_PROFILE_DIR;
if (directory) {
  mkdirSync(directory, { recursive: true });
  const options = value => ({ ...value, preload: [preload, ...([value?.preload].flat().filter(Boolean).filter(item => item !== preload))] });
  const stopWorker = (worker, terminate) => {
    const channel = new BroadcastChannel(`hunin-cpu-${process.pid}-${worker.threadId}`);
    return new Promise(resolve => {
      let done = false;
      const finish = () => { if (done) return; done = true; clearTimeout(timeout); channel.close(); resolve(terminate()); };
      channel.onmessage = event => { if (event.data === 'flushed') finish(); };
      const timeout = setTimeout(finish, 100);
      channel.postMessage('flush-stop');
    });
  };
  const OriginalWebWorker = globalThis.Worker;
  globalThis.Worker = class extends OriginalWebWorker { constructor(script, value) { super(script, options(value)); } terminate() { void stopWorker(this, () => super.terminate()); } };
  const OriginalNodeWorker = threads.Worker;
  threads.Worker = class extends OriginalNodeWorker { constructor(script, value) { super(script, options(value)); } terminate() { return stopWorker(this, () => super.terminate()); } };
  syncBuiltinESMExports();
  const session = new inspector.Session();
  session.connect();
  const post = method => new Promise((resolve, reject) => session.post(method, (error, result) => error ? reject(error) : resolve(result)));
  await post('Profiler.enable');
  await post('Profiler.start');
  const producer = randomUUID();
  let sequence = 0, stopping = false, finished = false, pending;
  async function save(restart) {
    if (!restart) stopping = true;
    if (pending) { await pending; if (restart || finished) return; }
    if (finished) return;
    pending = (async () => {
    try {
      const { profile } = await post('Profiler.stop');
      const path = join(directory, `CPU.${process.pid}.${threads.threadId}.${producer}.${++sequence}.cpuprofile`);
      writeFileSync(path, JSON.stringify(profile), { mode: 0o600 });
      writeFileSync(path + '.meta.json', JSON.stringify({ profile: path, pid: process.pid, tid: String(threads.threadId),
        profile_time_origin_us: 0, uncertainty_us: 1, clock_source: 'JavaScriptCore epoch microseconds' }), { mode: 0o600 });
      if (restart && !stopping) await post('Profiler.start');
      else { finished = true; session.disconnect(); }
    } catch (error) { process.stderr.write(`hunin: Bun CPU collection failed: ${error.message}\n`); }
    })();
    try { await pending; } finally { pending = undefined; }
  }
  const control = new BroadcastChannel(`hunin-cpu-${process.pid}-${threads.threadId}`);
  control.unref();
  control.onmessage = event => { if (event.data === 'flush-stop') { clearInterval(timer); void save(false).then(() => control.postMessage('flushed')); } };
  const timer = setInterval(() => save(true), 50);
  timer.unref();
  process.once('beforeExit', () => { clearInterval(timer); void save(false); });
}
