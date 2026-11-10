'use strict';

const fs = require('node:fs');
const inspector = require('node:inspector');
const readline = require('node:readline');
const path = require('node:path');
const { fileURLToPath } = require('node:url');
function writeDurable(filename, value) {
  const descriptor = fs.openSync(filename, 'wx', 0o600);
  try { fs.writeFileSync(descriptor, JSON.stringify(value)); fs.fsyncSync(descriptor); }
  finally { fs.closeSync(descriptor); }
}
const cleanup = { input: null, socket: null, inspector: false };
async function connectDebugger(url, onScript) {
  const socket = new WebSocket(url);
  cleanup.socket = socket;
  const pending = new Map();
  let nextId = 1;
  socket.addEventListener('message', event => {
    const message = JSON.parse(String(event.data));
    if (message.method === 'Debugger.scriptParsed') onScript(message.params);
    if (message.id && pending.has(message.id)) {
      const command = pending.get(message.id); pending.delete(message.id); clearTimeout(command.timer);
      if (message.error) command.reject(new Error(JSON.stringify(message.error)));
      else command.resolve(message.result);
    }
  });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('debugger connection deadline exceeded')), 5000);
    socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
    socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('debugger connection failed')); }, { once: true });
  });
  return (method, params = {}) => new Promise((resolve, reject) => {
    const id = nextId++;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`debugger command deadline exceeded: ${method}`)); }, 5000);
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }));
  });
}

async function main() {
  const [workload, profilePath, reportPath] = process.argv.slice(2);
  for (const value of [workload, profilePath, reportPath]) {
    if (!value || !path.isAbsolute(value)) throw new Error('helper paths must be absolute');
  }
  const captureScripts = process.env.HUNIN_SCRIPT_SOURCES === '1';
  let ownedInspector = false;
  if (captureScripts && process.versions.bun) {
    if (inspector.url()) throw new Error('prepared helper does not own an existing inspector');
    inspector.open(0, '127.0.0.1', false);
    if (!inspector.url()) throw new Error('Bun inspector did not publish an endpoint');
    ownedInspector = true;
    cleanup.inspector = true;
  }
  const session = new inspector.Session();
  session.connect();
  const post = (method, args = {}) => new Promise((resolve, reject) => {
    session.post(method, args, (error, result) => error ? reject(error) : resolve(result));
  });
  const workloadScripts = [];
  let debugPost = post;
  if (captureScripts) {
    const onScript = params => {
      let filename = params.url;
      try { if (filename.startsWith('file:')) filename = fileURLToPath(filename); } catch { return; }
      if (filename === workload) workloadScripts.push({ scriptId: params.scriptId, url: params.url });
    };
    if (process.versions.bun) debugPost = await connectDebugger(inspector.url(), onScript);
    else session.on('Debugger.scriptParsed', ({ params }) => onScript(params));
    await debugPost('Debugger.enable');
  }
  await post('Profiler.enable');
  await post('Profiler.setSamplingInterval', { interval: 1000 });
  const startBeforeNs = process.hrtime.bigint().toString();
  await post('Profiler.start');
  const startAfterNs = process.hrtime.bigint().toString();
  const input = readline.createInterface({ input: process.stdin });
  cleanup.input = input;
  const commands = input[Symbol.asyncIterator]();
  process.stdout.write('HUNIN_READY\n');
  const first = await commands.next();
  const command = first.done ? 'EOF' : first.value;
  let outcome = 'stopped_before_work';
  let output = null;
  let applicationError = null;
  let workStartNs = null;
  let workEndNs = null;
  try {
    if (command === 'HUNIN_GO') {
      workStartNs = process.hrtime.bigint().toString();
      const module = require(workload);
      if (typeof module.run !== 'function') throw new Error('prepared workload must export run()');
      output = await module.run();
      outcome = 'completed';
    } else if (!['HUNIN_STOP', 'EOF'].includes(command)) {
      throw new Error('invalid prepared-work gate command');
    }
  } catch (error) {
    outcome = 'application_failed';
    applicationError = String(error && error.stack || error);
  } finally {
    if (workStartNs !== null) workEndNs = process.hrtime.bigint().toString();
  }
  const stopBeforeNs = process.hrtime.bigint().toString();
  const { profile } = await post('Profiler.stop');
  const stopAfterNs = process.hrtime.bigint().toString();
  if (captureScripts) {
    for (const node of profile.nodes) {
      const identity = node.callFrame;
      let filename = identity.url;
      try { if (filename.startsWith('file:')) filename = fileURLToPath(filename); } catch { continue; }
      if (filename === workload && !workloadScripts.some(script => script.scriptId === identity.scriptId)) {
        workloadScripts.push({ scriptId: identity.scriptId, url: identity.url });
      }
    }
  }
  const scripts = [];
  const crypto = captureScripts ? require('node:crypto') : null;
  for (const identity of new Map(workloadScripts.map(script => [script.scriptId, script])).values()) {
    const { scriptSource } = await debugPost('Debugger.getScriptSource', { scriptId: identity.scriptId });
    if (Buffer.byteLength(scriptSource) > 512 * 1024) throw new Error('executed script identity exceeds source budget');
    scripts.push({ ...identity, sha256: crypto.createHash('sha256').update(scriptSource).digest('hex'), source: scriptSource });
  }
  if (captureScripts && command === 'HUNIN_GO' && !scripts.length) throw new Error('runtime exposed no matching executed workload script');
  session.disconnect();
  if (cleanup.socket) { cleanup.socket.close(); cleanup.socket = null; }
  if (ownedInspector) { inspector.close(); cleanup.inspector = false; }
  writeDurable(profilePath, profile);
  writeDurable(reportPath, {
    outcome, output, applicationError, pid: process.pid, runtime: process.versions, scripts,
    profilePath, samplingIntervalUs: 1000,
    sourceClock: 'v8 profile microseconds; not implicitly the helper hrtime clock',
    helperClock: 'process.hrtime nanoseconds',
    startBeforeNs, startAfterNs, stopBeforeNs, stopAfterNs, workStartNs, workEndNs,
  });
  if (process.env.HUNIN_FINALIZE_GATE === '1') {
    process.stdout.write('HUNIN_WORK_COMPLETE\n');
    const final = await commands.next();
    const finish = final.done ? 'EOF' : final.value;
    if (!['HUNIN_EXIT', 'HUNIN_STOP', 'EOF'].includes(finish)) throw new Error('invalid finalize gate command');
  }
  input.close();
  process.stdin.pause();
  if (applicationError) process.exitCode = 1;
}

main().catch(error => { process.stderr.write(String(error.stack || error) + '\n'); process.exitCode = 1; }).finally(() => {
  if (cleanup.socket) cleanup.socket.close();
  if (cleanup.inspector) inspector.close();
  if (cleanup.input) cleanup.input.close();
  process.stdin.pause();
});
