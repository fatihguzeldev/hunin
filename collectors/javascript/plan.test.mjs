import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { plan } from './plan.mjs';

test('capture planning runs when the installed collector is reached through a directory link', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-linked-plan-'));
  try {
    const linked = join(directory, 'collector');
    symlinkSync(dirname(fileURLToPath(import.meta.url)), linked, process.platform === 'win32' ? 'junction' : 'dir');
    const request = { command: [process.execPath, '-e', 'console.log(1)'], cwd: directory, artifact_dir: join(directory, 'capture'), run_id: 'linked', runtime: 'node' };
    const result = spawnSync(process.execPath, [join(linked, 'plan.mjs')], { input: JSON.stringify(request), encoding: 'utf8', timeout: 10000 });
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.stdout.trim(), 'entry point silently produced no capture plan');
    assert.deepEqual(JSON.parse(result.stdout).command, request.command);
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

for (const [runtime, executable] of [['node', process.execPath], ['bun', process.env.HUNIN_TEST_BUN], ['deno', process.env.HUNIN_TEST_DENO]]) {
  if (!executable) continue;
  test(`${runtime} automatic capture resolves CPU work in its worker`, () => {
    const directory = mkdtempSync(join(tmpdir(), `hunin-${runtime}-`));
    try {
      const worker = join(directory, 'worker.mjs');
      const main = join(directory, 'main.mjs');
      writeFileSync(worker, `function workerHot() { const end = Date.now()+250; let result=0; while(Date.now()<end) for(let n=1;n<10000;n++) result+=Math.sqrt(n); return result; } ${runtime === 'node' ? "import { parentPort } from 'node:worker_threads'; parentPort.postMessage(workerHot());" : 'postMessage(workerHot());'}`);
      writeFileSync(main, `${runtime === 'node' ? "import { Worker } from 'node:worker_threads'; const w = new Worker(new URL('./worker.mjs', import.meta.url)); w.on('message', value=>console.log(value));" : "const w = new Worker(new URL('./worker.mjs',import.meta.url), {type:'module'}); w.onmessage=event=>{ console.log(event.data); w.terminate(); };"}`);
      const command = runtime === 'deno' ? [executable, 'run', `--allow-read=${directory}`, main] : [executable, main];
      const capture = plan({ command, cwd: directory, artifact_dir: directory, run_id: 'test', runtime });
      const result = spawnSync(capture.command[0], capture.command.slice(1), { cwd: directory, env: { ...process.env, ...capture.env }, encoding: 'utf8', timeout: 10000 });
      assert.equal(result.status, 0, result.stderr);
      const profiles = readdirSync(directory).filter(name => name.endsWith('.cpuprofile')).map(name => JSON.parse(readFileSync(join(directory, name), 'utf8')));
      assert.ok(profiles.some(profile => profile.nodes.some(node => node.callFrame.functionName === 'workerHot')), `no workerHot frame in ${profiles.length} profiles: ${result.stderr}`);
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
}
