'use strict';

const controlWrite = process.stdout.write.bind(process.stdout);
const controlToken = process.env.HUNIN_CONTROL_TOKEN;
const finalizeGate = process.env.HUNIN_FINALIZE_GATE === '1';
const emitControl = marker => controlWrite(`${marker}${controlToken ? ` ${controlToken}` : ''}\n`);

const fs = require('node:fs');
const path = require('node:path');
const readline = require('node:readline');
const crypto = require('node:crypto');
const { spawn } = require('node:child_process');
const { execFileSync } = require('node:child_process');
const net = require('node:net');
const { pathToFileURL } = require('node:url');

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
function writeNew(filename, value) {
  const fd = fs.openSync(filename, 'wx', 0o600);
  try { fs.writeFileSync(fd, JSON.stringify(value)); fs.fsyncSync(fd); }
  finally { fs.closeSync(fd); }
}

class Rpc {
  constructor(socket) {
    this.socket = socket; this.sequence = 0; this.pending = new Map(); this.scripts = [];
    this.attached = null; this.contexts = [];
    socket.addEventListener('message', event => {
      if (typeof event.data !== 'string' || Buffer.byteLength(event.data) > 16 * 1024 * 1024) { socket.close(); return; }
      let message;
      try { message = JSON.parse(event.data); } catch { socket.close(); return; }
      if (message.method === 'Runtime.executionContextCreated') {
        if (this.contexts.length >= 4096) { socket.close(); return; }
        this.contexts.push({ ...message.params.context, sessionId: message.sessionId });
      }
      if (message.method === 'Debugger.scriptParsed') {
        if (this.scripts.length >= 4096) { socket.close(); return; }
        this.scripts.push({ ...message.params, sessionId: message.sessionId });
      }
      if (message.method === 'Target.attachedToTarget') this.attached?.(message.params, message.sessionId);
      const pending = this.pending.get(message.id);
      if (!pending) return;
      this.pending.delete(message.id); clearTimeout(pending.timer);
      if (message.error || message.type === 'error') pending.reject(new Error(JSON.stringify(message)));
      else pending.resolve(message.result);
    });
    socket.addEventListener('close', () => {
      for (const entry of this.pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('browser diagnostic connection closed')); }
      this.pending.clear();
    });
  }
  static async connect(url) {
    const parsed = new URL(url);
    if (parsed.protocol !== 'ws:' || !['127.0.0.1', 'localhost', '[::1]'].includes(parsed.hostname)) throw new Error('browser diagnostic socket must be loopback');
    const socket = new WebSocket(url);
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => { socket.close(); reject(new Error('diagnostic connection deadline')); }, 10000);
      socket.addEventListener('open', () => { clearTimeout(timer); resolve(); }, { once: true });
      socket.addEventListener('error', () => { clearTimeout(timer); reject(new Error('diagnostic connection failed')); }, { once: true });
    });
    return new Rpc(socket);
  }
  call(method, params = {}, sessionId) {
    if (this.pending.size >= 64) return Promise.reject(new Error('too many diagnostic commands'));
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`${method} deadline exceeded`)); }, 10000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  close() {
    for (const entry of this.pending.values()) { clearTimeout(entry.timer); entry.reject(new Error('owned diagnostic connection closed')); }
    this.pending.clear();
    this.socket.close();
  }
}

function launch(executable, args, area) {
  const child = spawn(executable, args, { cwd: area, env: { ...process.env, TMPDIR: area, TMP: area, TEMP: area }, stdio: ['ignore', 'pipe', 'pipe'] });
  let error = ''; let failed;
  child.on('error', value => { failed = value; });
  for (const output of [child.stdout, child.stderr]) output.on('data', chunk => { if (error.length < 65536) error += String(chunk).slice(0, 65536 - error.length); });
  return { child, check() { if (failed) throw failed; if (child.exitCode !== null) throw new Error(`browser tool exited ${child.exitCode}: ${error}`); }, diagnostics: () => error };
}

function abortOwnedTool(tool) {
  tool.child.kill();
  tool.child.stdout.destroy(); tool.child.stderr.destroy(); tool.child.unref();
}

async function until(action, process, label) {
  const end = Date.now() + 10000;
  while (Date.now() < end) {
    process.check();
    const result = await action(); if (result) return result;
    await pause(20);
  }
  throw new Error(`${label} deadline: ${process.diagnostics()}`);
}

async function jsonRequest(base, method, route, body) {
  const response = await fetch(base + route, { method, headers: { 'Content-Type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(10000) });
  const reader = response.body.getReader();
  const chunks = []; let bytes = 0;
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    bytes += value.byteLength;
    if (bytes > 16 * 1024 * 1024) { await reader.cancel(); throw new Error('webdriver response exceeds limit'); }
    chunks.push(value);
  }
  const text = Buffer.concat(chunks, bytes).toString('utf8');
  const result = JSON.parse(text);
  if (!response.ok || result.value?.error) throw new Error(JSON.stringify(result));
  return result.value;
}

function remoteValue(value, depth = 0) {
  if (depth > 32) throw new Error('browser result exceeds depth bound');
  switch (value.type) {
    case 'null': return null;
    case 'undefined': return null;
    case 'string': case 'number': case 'boolean': return value.value;
    case 'array': return (value.value || []).map(item => remoteValue(item, depth + 1));
    case 'object': return Object.fromEntries((value.value || []).map(([key, item]) => [typeof key === 'string' ? key : remoteValue(key, depth + 1), remoteValue(item, depth + 1)]));
    default: throw new Error(`unsupported prepared result ${value.type}`);
  }
}

function chromiumEndpoint(filename) {
  let text;
  try { text = fs.readFileSync(filename, 'utf8'); }
  catch (error) {
    if (['ENOENT', 'EBUSY'].includes(error.code)) return null;
    throw error;
  }
  const [port, route] = text.trim().split('\n');
  const prefix = '/devtools/browser/';
  if (!/^\d+$/.test(port) || !route?.startsWith(prefix) || route.length === prefix.length) return null;
  return `ws://127.0.0.1:${port}${route}`;
}

async function chromium(browser, workload, area) {
  const profileDirectory = path.join(area, 'chrome-profile'); fs.mkdirSync(profileDirectory);
  // this disposable, credential-free profile must never access the user's OS secret store.
  const secretStore = process.platform === 'darwin' ? ['--use-mock-keychain'] : process.platform === 'linux' ? ['--password-store=basic'] : [];
  // fresh Windows profiles otherwise probe a blank OS password and can lock the user's account (chromium issue 541310282).
  const deviceAuthentication = process.platform === 'win32' ? ['--disable-features=AutofillAiWalletPrivatePasses'] : [];
  const child = launch(browser, ['--headless=new', '--remote-debugging-port=0', `--user-data-dir=${profileDirectory}`, ...secretStore, ...deviceAuthentication, '--no-first-run', '--disable-popup-blocking', '--no-default-browser-check', '--disable-background-networking', '--disable-component-update', '--disable-sync', 'about:blank'], area);
  let rpc;
  try {
    const active = path.join(profileDirectory, 'DevToolsActivePort');
    const endpoint = await until(() => chromiumEndpoint(active), child, 'chromium diagnostic startup');
    rpc = await Rpc.connect(endpoint);
    const version = await rpc.call('Browser.getVersion');
    const { targetId } = await rpc.call('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await rpc.call('Target.attachToTarget', { targetId, flatten: true });
    const sessions = [{ targetId, sessionId, type: 'page', selected: true }];
    const arming = []; const sourceFailures = []; const identityOnlyTargets = new Set();
    rpc.attached = (target, parentSessionId) => {
      if (identityOnlyTargets.has(target.targetInfo.targetId)) return;
      const identity = { targetId: target.targetInfo.targetId, sessionId: target.sessionId, type: target.targetInfo.type, parentSessionId, targetInfo: target.targetInfo };
      if (sessions.length >= 128) { sourceFailures.push('worker source budget exceeded'); return; }
      sessions.push(identity);
      arming.push((async () => {
        try {
          await rpc.call('Runtime.enable', {}, identity.sessionId);
          identity.isolateId = (await rpc.call('Runtime.getIsolateId', {}, identity.sessionId)).id;
          await rpc.call('Debugger.enable', {}, identity.sessionId);
          await rpc.call('Profiler.enable', {}, identity.sessionId);
          await rpc.call('Profiler.setSamplingInterval', { interval: 1000 }, identity.sessionId);
          await rpc.call('Profiler.start', {}, identity.sessionId);
          identity.ready = true;
        } catch (error) { sourceFailures.push(`${identity.targetId}: ${String(error)}`); }
        finally { try { await rpc.call('Runtime.runIfWaitingForDebugger', {}, identity.sessionId); } catch {} }
      })());
    };
    await rpc.call('Target.setAutoAttach', { autoAttach: true, waitForDebuggerOnStart: true, flatten: true }, sessionId);
    await rpc.call('Page.enable', {}, sessionId);
    await rpc.call('Runtime.enable', {}, sessionId);
    sessions[0].isolateId = (await rpc.call('Runtime.getIsolateId', {}, sessionId)).id;
    await rpc.call('Debugger.enable', {}, sessionId);
    await rpc.call('Page.navigate', { url: pathToFileURL(workload).href }, sessionId);
    await until(async () => (await rpc.call('Runtime.evaluate', { expression: 'typeof globalThis.run === "function"', returnByValue: true }, sessionId)).result.value, child, 'prepared page initialization');
    await rpc.call('Profiler.enable', {}, sessionId);
    await rpc.call('Profiler.setSamplingInterval', { interval: 1000 }, sessionId);
    await rpc.call('Profiler.start', {}, sessionId);
    sessions[0].ready = true;
    await Promise.all(arming);
    sessions[0].targetInfo = (await rpc.call('Target.getTargetInfo', { targetId })).targetInfo;
    const processes = (await rpc.call('SystemInfo.getProcessInfo')).processInfo;
    return {
      pid: child.child.pid, processes, targets: sessions, version,
      async run(input) {
        const expression = `globalThis.run(JSON.parse(${JSON.stringify(JSON.stringify(input))}))`;
        const result = await rpc.call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId);
        if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
        return result.result.value;
      },
      async stop() {
        await Promise.all(arming);
        const profiles = [];
        for (const identity of sessions) {
          if (!identity.ready) continue;
          try {
            const { profile } = await rpc.call('Profiler.stop', {}, identity.sessionId);
            profiles.push({ ...identity, profile });
          } catch (error) { sourceFailures.push(`${identity.targetId}: ${String(error)}`); }
        }
        const reportedTargets = (await rpc.call('Target.getTargets')).targetInfos;
        const contextObservers = [...sessions];
        // another page can share the profiled isolate; inspect its context identity without starting a second profiler.
        for (const info of reportedTargets.filter(info => info.type === 'page' && !sessions.some(s => s.targetId === info.targetId))) {
          if (contextObservers.length >= 128) { sourceFailures.push('context identity observer budget exceeded'); break; }
          try {
            identityOnlyTargets.add(info.targetId);
            const { sessionId: contextSessionId } = await rpc.call('Target.attachToTarget', { targetId: info.targetId, flatten: true });
            await rpc.call('Runtime.enable', {}, contextSessionId);
            const { id: isolateId } = await rpc.call('Runtime.getIsolateId', {}, contextSessionId);
            await rpc.call('Debugger.enable', {}, contextSessionId);
            contextObservers.push({ targetId: info.targetId, sessionId: contextSessionId, isolateId, targetInfo: info, diagnosticOnly: true });
          } catch (error) { sourceFailures.push(`context identity ${info.targetId}: ${String(error)}`); }
        }
        const frameTrees = [];
        for (const observer of contextObservers.filter(observer => observer.targetInfo?.type === 'page')) {
          try { frameTrees.push({ targetId: observer.targetId, sessionId: observer.sessionId, frameTree: (await rpc.call('Page.getFrameTree', {}, observer.sessionId)).frameTree }); }
          catch (error) { sourceFailures.push(`frame identity ${observer.targetId}: ${String(error)}`); }
        }
        const scripts = []; let scriptBytes = 0;
        for (const identity of rpc.scripts.filter(x => contextObservers.some(s => s.sessionId === x.sessionId) && (x.url.startsWith('file:') || x.url.startsWith('blob:')))) {
          try {
            const { scriptSource } = await rpc.call('Debugger.getScriptSource', { scriptId: identity.scriptId }, identity.sessionId);
            if (Buffer.byteLength(scriptSource) > 512 * 1024) throw new Error('executed script exceeds identity capture budget');
            if (scriptBytes + Buffer.byteLength(scriptSource) > 4 * 1024 * 1024) throw new Error('executed scripts exceed collection identity budget');
            scriptBytes += Buffer.byteLength(scriptSource);
            const observer = contextObservers.find(s => s.sessionId === identity.sessionId);
            scripts.push({ ...identity, targetId: observer.targetId, isolateId: observer.isolateId, source: scriptSource, sha256: crypto.createHash('sha256').update(scriptSource).digest('hex') });
          } catch (error) { sourceFailures.push(`script identity ${identity.sessionId}/${identity.scriptId}: ${String(error)}`); }
        }
        return { kind: 'chromium', version, processes, profiles, scripts, sourceFailures, selectedTargetId: targetId, targets: sessions, reportedTargets, contextObservers, executionContexts: rpc.contexts, frameTrees, launch_configuration: { profile: 'fresh owned credential-free directory', secret_store: secretStore, device_authentication: deviceAuthentication }, scope: 'profiler target sessions may share an isolate with other pages; context projection requires captured isolate/script/execution-context identity; browser/renderer resources have broader process scope; stopped or unarmed workers are explicit source failures' };
      },
      async close() {
        const exited = child.child.exitCode !== null ? Promise.resolve() : new Promise(resolve => child.child.once('exit', resolve));
        // chromium may exit before returning the Browser.close command response.
        const closing = rpc.call('Browser.close').catch(() => {});
        let timer;
        try { await Promise.race([exited, new Promise(resolve => { timer = setTimeout(resolve, 2000); })]); }
        finally { clearTimeout(timer); rpc.close(); await closing; }
        if (child.child.exitCode === null) child.child.kill();
        // only the owned diagnostic pipes remain; group/job cleanup belongs to the host backend.
        child.child.stdout.destroy(); child.child.stderr.destroy(); child.child.unref();
      },
    };
  } catch (error) { rpc?.close(); abortOwnedTool(child); throw error; }
}

async function firefox(browser, driver, workload, area) {
  const child = launch(driver, ['--host', '127.0.0.1', '--port', '0', '--profile-root', area], area);
  const port = await until(() => /Listening on 127\.0\.0\.1:(\d+)/.exec(child.diagnostics())?.[1], child, 'owned firefox listener');
  const base = `http://127.0.0.1:${port}`;
  let session, rpc;
  try {
    await until(async () => { try { return (await jsonRequest(base, 'GET', '/status')).ready; } catch { return false; } }, child, 'firefox driver startup');
    session = await jsonRequest(base, 'POST', '/session', { capabilities: { alwaysMatch: { browserName: 'firefox', webSocketUrl: true, 'moz:firefoxOptions': { binary: browser, args: ['-headless'], prefs: { 'browser.download.dir': area, 'browser.download.folderList': 2, 'browser.download.useDownloadDir': true } } } } });
    const caps = session.capabilities;
    if (!fs.realpathSync(caps['moz:profile']).startsWith(fs.realpathSync(area) + path.sep)) throw new Error('firefox profile escaped project area');
    rpc = await Rpc.connect(caps.webSocketUrl);
    const { context } = await rpc.call('browsingContext.create', { type: 'tab' });
    await rpc.call('moz:debugging.setDebuggerEnabled', { contexts: [context], enabled: true });
    await rpc.call('browsingContext.navigate', { context, url: pathToFileURL(workload).href, wait: 'complete' });
    const ready = await rpc.call('script.evaluate', { expression: 'typeof globalThis.run === "function"', target: { context }, awaitPromise: true });
    if (remoteValue(ready.result) !== true) throw new Error('prepared page must expose globalThis.run');
    await rpc.call('moz:profiler.start', { preset: 'web-developer', activeContext: context });
    if (!(await rpc.call('moz:profiler.isActive')).active) throw new Error('firefox profiler did not acknowledge running');
    return {
      pid: caps['moz:processID'], processes: [{ id: caps['moz:processID'], type: 'browser' }], targets: [{ context }], version: caps,
      async run(input) {
        const expression = `globalThis.run(JSON.parse(${JSON.stringify(JSON.stringify(input))}))`;
        const result = await rpc.call('script.evaluate', { expression, target: { context }, awaitPromise: true, serializationOptions: { maxObjectDepth: 16 } });
        if (result.type === 'exception') throw new Error(JSON.stringify(result.exceptionDetails));
        return remoteValue(result.result);
      },
      async stop() {
        const { path: saved } = await rpc.call('moz:profiler.stop');
        if (!saved || !fs.realpathSync(saved).startsWith(fs.realpathSync(area) + path.sep) || !fs.lstatSync(saved).isFile()) throw new Error('firefox export is absent or outside project area');
        if (fs.statSync(saved).size > 16 * 1024 * 1024) throw new Error('firefox recording exceeds input budget');
        const profile = JSON.parse(fs.readFileSync(saved, 'utf8'));
        const scripts = []; let scriptBytes = 0;
        const listed = await rpc.call('moz:debugging.listScripts', { context });
        if (!Array.isArray(listed.scripts) || listed.scripts.length > 4096) throw new Error('firefox script identity budget exceeded');
        for (const url of listed.scripts.filter(x => typeof x === 'string' && (x.startsWith('file:') || x.startsWith('blob:')))) {
          const { source } = await rpc.call('moz:debugging.getScriptSource', { context, scriptUrl: url });
          if (typeof source !== 'string' || Buffer.byteLength(source) > 512 * 1024) throw new Error('firefox script source exceeds budget');
          scriptBytes += Buffer.byteLength(source);
          if (scriptBytes > 4 * 1024 * 1024) throw new Error('firefox script collection exceeds budget');
          scripts.push({ scriptId: url, url, context, page_url: pathToFileURL(workload).href, source, sha256: crypto.createHash('sha256').update(source).digest('hex'), identity_origin: 'browser debugger returned bytes' });
        }
        const scopeBindings = { selectedContext: context, contexts: await rpc.call('browsingContext.getTree', {}), realms: await rpc.call('script.getRealms', {}) };
        return { kind: 'firefox', version: caps, profile, scripts, inputPath: saved, targets: [{ context }], scopeBindings, scope: 'global profiler in fresh owned browser; samples are not attributed to one tab by activeContext' };
      },
      async close() {
        rpc.close();
        try { await jsonRequest(base, 'DELETE', `/session/${session.sessionId}`); }
        finally { abortOwnedTool(child); }
      },
    };
  } catch (error) {
    rpc?.close();
    if (session) { try { await jsonRequest(base, 'DELETE', `/session/${session.sessionId}`); } catch {} }
    abortOwnedTool(child);
    throw new Error(`${error.stack || error}\nowned firefox diagnostics: ${child.diagnostics()}`);
  }
}

async function safari(browser, driver, workload, area) {
  if (process.platform !== 'darwin') throw new Error('Safari collection requires macOS');
  const config = JSON.parse(fs.readFileSync(process.env.HUNIN_SAFARI_SCRIPTS, 'utf8'));
  if (!Array.isArray(config) || config.length < 1 || config.length > 16) throw new Error('Safari needs1..16 explicitly prepared classic scripts');
  for (const script of config) {
    for (const key of ['original', 'instrumented', 'binding']) if (!path.isAbsolute(script[key])) throw new Error('Safari source paths must be absolute');
    script.metadata = JSON.parse(fs.readFileSync(script.binding, 'utf8'));
    const hash = filename => crypto.createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
    if (hash(script.original) !== script.metadata.original_sha256 || hash(script.instrumented) !== script.metadata.instrumented_sha256) throw new Error('Safari prepared source binding changed');
    script.original_url = pathToFileURL(script.original).href;
    script.instrumented_url = pathToFileURL(script.instrumented).href;
  }
  const lease = net.createServer();
  await new Promise((resolve, reject) => { lease.once('error', reject); lease.listen(0, '127.0.0.1', resolve); });
  const port = lease.address().port;
  await new Promise((resolve, reject) => lease.close(error => error ? reject(error) : resolve()));
  const child = launch(driver, ['--port', String(port)], area);
  const base = `http://127.0.0.1:${port}`;
  let session;
  try {
    await until(async () => {
      try {
        const status = await jsonRequest(base, 'GET', '/status');
        child.check();
        // The temporary port lease alone cannot prove which process won the bind.
        const listening = execFileSync('/usr/sbin/lsof', ['-nP', '-iTCP:' + port, '-sTCP:LISTEN', '-F', 'p'], { timeout: 2000, maxBuffer: 65536 }).toString();
        const listeners = [...new Set(listening.split('\n').filter(x => /^p\d+$/.test(x)).map(x => x.slice(1)))];
        if (listeners.length !== 1) return false;
        const executable = execFileSync('/bin/ps', ['-p', listeners[0], '-o', 'comm='], { timeout: 2000, maxBuffer: 65536 }).toString().trim();
        // Apple's listener is a launchd XPC service, not the safaridriver client PID.
        const expected = '/System/Library/PrivateFrameworks/WebDriver.framework/Versions/A/XPCServices/com.apple.WebDriver.HTTPService.xpc/Contents/MacOS/com.apple.WebDriver.HTTPService';
        return status.ready && fs.realpathSync(executable) === fs.realpathSync(expected);
      } catch { return false; }
    }, child, 'owned Safari driver listener');
    session = await jsonRequest(base, 'POST', '/session', { capabilities: { alwaysMatch: { browserName: 'safari' } } });
    const route = `/session/${session.sessionId}`;
    const execute = (script, args = []) => jsonRequest(base, 'POST', route + '/execute/sync', { script, args });
    await jsonRequest(base, 'POST', route + '/timeouts', { script: 10000, pageLoad: 10000 });
    const html = fs.readFileSync(workload, 'utf8');
    if (Buffer.byteLength(html) > 1024 * 1024) throw new Error('Safari prepared HTML exceeds budget');
    // DOMParser is inert. Only the resulting owned HTML is ever navigated, so startup runs once.
    const rewritten = await execute(`
      const [html, originalUrl, scripts] = arguments;
      const doc = new DOMParser().parseFromString(html, 'text/html');
      const base = doc.createElement('base'); base.href = originalUrl; doc.head.prepend(base);
      const found = new Set();
      for (const element of doc.querySelectorAll('script')) {
        if (element.type && !['text/javascript','application/javascript'].includes(element.type)) throw new Error('Safari fallback supports classic scripts only');
        if (element.async || element.defer || !element.hasAttribute('src')) throw new Error('Safari fallback requires ordered external classic scripts');
        const url = new URL(element.getAttribute('src'), originalUrl).href;
        const script = scripts.find(s => s.original_url === url);
        if (!script || found.has(url)) throw new Error('every prepared script needs one distinct binding');
        found.add(url); element.src = script.instrumented_url;
      }
      if (found.size !== scripts.length) throw new Error('unused prepared script binding');
      return '<!doctype html>\\n' + doc.documentElement.outerHTML;
    `, [html, pathToFileURL(workload).href, config.map(s => ({original_url:s.original_url,instrumented_url:s.instrumented_url}))]);
    const preparedHtml = path.join(area, 'safari-workload.html');
    fs.writeFileSync(preparedHtml, rewritten, { flag: 'wx', mode: 0o600 });
    await jsonRequest(base, 'POST', route + '/url', { url: pathToFileURL(preparedHtml).href });
    const bindings = config.map(s => ({ transform_id: s.metadata.transform_id, instrumented_sha256: s.metadata.instrumented_sha256 }));
    const acknowledged = await execute('if(typeof globalThis.run!=="function" || !globalThis.__HuninObservation) throw new Error("prepared Safari source missing"); return globalThis.__HuninObservation.start(arguments[0]);', [{bindings,max_events:100000}]);
    const candidates = execFileSync('/bin/ps', ['-axo', 'pid=,comm='], { timeout: 2000, maxBuffer: 1024 * 1024 }).toString().split('\n').flatMap(line => {
      const match = /^\s*(\d+)\s+(.+?)\s*$/.exec(line);
      if (!match) return [];
      try { return fs.realpathSync(match[2]) === fs.realpathSync(browser) ? [Number(match[1])] : []; } catch { return []; }
    });
    if (candidates.length !== 1) throw new Error('Safari primary process identity is ambiguous or absent');
    return {
      pid: candidates[0], processes: [{id:candidates[0],type:'shared Safari application',owned:false}], targets: [{sessionId:session.sessionId,preparedUrl:pathToFileURL(preparedHtml).href}], version: session.capabilities,
      async run(payload) {
        const result = await jsonRequest(base, 'POST', route + '/execute/async', { script:'const done=arguments[arguments.length-1]; Promise.resolve().then(()=>globalThis.run(arguments[0])).then(value=>done({ok:true,value}),error=>done({ok:false,error:String(error.stack||error)}));', args:[payload] });
        if (!result.ok) throw new Error(result.error);
        return result.value;
      },
      async stop() {
        const profile = await execute('return globalThis.__HuninObservation.stop();');
        const scripts = config.map((script, index) => {
          const source = fs.readFileSync(script.instrumented, 'utf8');
          const hash = crypto.createHash('sha256').update(source).digest('hex');
          if (hash !== script.metadata.instrumented_sha256) throw new Error('Safari prepared code changed during collection');
          return {scriptId:`prepared-${index}`,url:script.original_url,source,sha256:hash,identity_origin:'owned transformed file hash before and after page load; script transform id acknowledged by installed hooks',binding:script.metadata};
        });
        return {kind:'safari',version:session.capabilities,profile,scripts,acknowledged,scope:'compiler-inserted function body events in explicitly prepared classic scripts; shared browser resources are not page or call CPU'};
      },
      async close() {
        try { await jsonRequest(base, 'DELETE', route); }
        finally { child.child.kill(); child.child.stdout.destroy(); child.child.stderr.destroy(); child.child.unref(); }
      },
    };
  } catch (error) {
    if (session) { try { await jsonRequest(base, 'DELETE', `/session/${session.sessionId}`); } catch {} }
    abortOwnedTool(child); throw error;
  }
}

function browserPath(filename) {
  if (process.platform !== 'win32') return filename;
  if (filename.startsWith('\\\\?\\UNC\\')) return '\\\\' + filename.slice(8);
  return /^\\\\\?\\[A-Za-z]:\\/.test(filename) ? filename.slice(4) : filename;
}

async function main() {
  const [kind, browser, driver, workload, area] = process.argv.slice(2).map(browserPath);
  for (const filename of [browser, workload, area]) if (!path.isAbsolute(filename)) throw new Error('browser paths must be absolute');
  let preparedDriver;
  if (process.env.HUNIN_BROWSER_DRIVER) {
    const filename = fs.realpathSync(process.env.HUNIN_BROWSER_DRIVER);
    const bytes = fs.readFileSync(filename);
    if (bytes.length > 1024 * 1024 || crypto.createHash('sha256').update(bytes).digest('hex') !== process.env.HUNIN_BROWSER_DRIVER_SHA256) throw new Error('prepared browser driver identity changed');
    preparedDriver = require(filename);
    if (typeof preparedDriver.input !== 'function' || typeof preparedDriver.output !== 'function') throw new Error('prepared browser driver must expose input and output');
  }
  const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  const commands = input[Symbol.asyncIterator]();
  let observation;
  try {
    observation = kind === 'chromium' ? await chromium(browser, workload, area) : kind === 'firefox' ? await firefox(browser, driver, workload, area) : kind === 'safari' ? await safari(browser, driver, workload, area) : (() => { throw new Error('unsupported browser collector'); })();
    writeNew(path.join(area, 'browser-targets.json'), { pid: observation.pid, processes: observation.processes, targets: observation.targets, version: observation.version });
    emitControl('HUNIN_READY');
    const command = await commands.next();
    let outcome = 'stopped_before_work', output = null, applicationError = null;
    const workStartNs = process.hrtime.bigint().toString();
    if (!command.done && command.value === 'HUNIN_GO') {
      try {
        const payload = preparedDriver ? await preparedDriver.input() : null;
        const encoded = JSON.stringify(payload);
        if (Buffer.byteLength(encoded) > 4 * 1024 * 1024) throw new Error('prepared browser input exceeds budget');
        output = await observation.run(payload);
        if (Buffer.byteLength(JSON.stringify(output)) > 4 * 1024 * 1024) throw new Error('prepared browser output exceeds budget');
        if (preparedDriver) output = await preparedDriver.output(output);
        outcome = 'completed';
      }
      catch (error) { outcome = 'application_failed'; applicationError = String(error.stack || error); }
    }
    const workEndNs = process.hrtime.bigint().toString();
    const profile = await observation.stop();
    writeNew(path.join(area, 'browser-profile.json'), profile);
    writeNew(path.join(area, 'browser-report.json'), { outcome, output, applicationError, runtime: observation.version, workStartNs, workEndNs, clock: 'driver hrtime; not implicitly any browser profile clock' });
    if (finalizeGate) {
      emitControl('HUNIN_WORK_COMPLETE');
      const finish = await commands.next();
      if (!finish.done && !['HUNIN_EXIT', 'HUNIN_STOP'].includes(finish.value)) throw new Error('invalid browser finalization command');
    }
    if (applicationError) process.exitCode = 1;
  } finally { input.close(); process.stdin.pause(); if (observation) await observation.close(); }
}

main().catch(error => { process.stderr.write(String(error.stack || error) + '\n'); process.exitCode = 1; });
