const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(process.argv[2], 'utf8').split('main().catch(')[0];
const area = process.argv[3];
fs.mkdirSync(area + '/profile', { recursive: true });
const test = `
(async () => {
 const assert = require('node:assert/strict');
 const originalRead = fs.readFileSync;
 try {
  for (const code of ['ENOENT', 'EBUSY']) {
   fs.readFileSync = () => { throw Object.assign(new Error(code), {code}); };
   assert.equal(chromiumEndpoint('owned-port-file'), null);
  }
  fs.readFileSync = () => { throw Object.assign(new Error('permanent access denial'), {code:'EACCES'}); };
  assert.throws(() => chromiumEndpoint('owned-port-file'), /permanent access denial/);
  fs.readFileSync = () => 'waiting'; assert.equal(chromiumEndpoint('owned-port-file'), null);
  fs.readFileSync = () => '9223\\n/devtools/browser/\\n'; assert.equal(chromiumEndpoint('owned-port-file'), null);
  fs.readFileSync = () => '9223\\n/devtools/browser/owned\\n';
  assert.equal(chromiumEndpoint('owned-port-file'), 'ws://127.0.0.1:9223/devtools/browser/owned');
 } finally { fs.readFileSync = originalRead; }
 assert.equal(browserPath(String.raw\`\\\\?\\C:\\owned\\profile\`), String.raw\`C:\\owned\\profile\`);
 assert.equal(browserPath(String.raw\`\\\\?\\UNC\\server\\share\\profile\`), String.raw\`\\\\server\\share\\profile\`);
 assert.equal(browserPath(String.raw\`C:\\owned\\profile\`), String.raw\`C:\\owned\\profile\`);
 assert.equal(browserPath(String.raw\`\\\\?\\GLOBALROOT\\device\`), String.raw\`\\\\?\\GLOBALROOT\\device\`);
 for (const platform of ['win32', 'linux', 'darwin']) {
  Object.defineProperty(process, 'platform', { value: platform, configurable: true });
  const platformArea = ${JSON.stringify(area)} + '/' + platform;
  fs.mkdirSync(platformArea);
  let captured;
  launch = (executable, args) => { captured = args; throw new Error('launch captured'); };
  await assert.rejects(chromium('browser', 'workload', platformArea), /launch captured/);
  assert.equal(captured.includes('--disable-features=AutofillAiWalletPrivatePasses'), platform === 'win32');
  assert.equal(captured.some(arg => arg === '--no-sandbox' || arg === '--disable-setuid-sandbox'), false);
 }
 Object.defineProperty(process, 'platform', { value: 'win32', configurable: true });
 for (const fail of [false, true]) {
  const events = [];
  launch = () => ({ child: { kill() { events.push('kill'); }, stdout: {destroy() { events.push('stdout'); }}, stderr: {destroy() { events.push('stderr'); }}, unref() { events.push('unref'); } }});
  until = async () => '1234';
  jsonRequest = async (base, method) => {
   if (method === 'DELETE') { events.push('delete'); if (fail) throw new Error('shutdown control'); return; }
   return { sessionId: 'owned', capabilities: { 'moz:profile': ${JSON.stringify(area + '/profile')}, webSocketUrl: 'ws://127.0.0.1' } };
  };
  Rpc.connect = async () => ({
   close() { events.push('rpc.close'); },
   async call(method) {
    if (method === 'browsingContext.create') return { context: 'owned' };
    if (method === 'script.evaluate') return { result: {type: 'boolean', value: true} };
    if (method === 'moz:profiler.isActive') return { active: true };
    return {};
   }
  });
  const observation = await firefox('browser', 'driver', ${JSON.stringify(area + '/workload.html')}, ${JSON.stringify(area)});
  if (fail) await assert.rejects(observation.close(), /shutdown control/);
  else await observation.close();
  assert.deepEqual(events, ['rpc.close', 'delete', 'kill', 'stdout', 'stderr', 'unref']);
 }
})().catch(error => { console.error(error); process.exitCode = 1; });`;
const processView = Object.create(process);
Object.defineProperty(processView, 'platform', { value: 'win32', configurable: true });
vm.runInNewContext(source + test, { require, process: processView, console, setTimeout, clearTimeout, Buffer, URL });
