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

test('target environment composes with profiling and selects the requested runtime through PATH', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin plan env '));
  try {
    const executableDirectory = join(directory, 'runtime');
    symlinkSync(dirname(process.execPath), executableDirectory, process.platform === 'win32' ? 'junction' : 'dir');
    const capture = plan({ command: ['node', '-e', 'console.log(JSON.stringify([process.env.HUNIN_ENV_TEST,process.execArgv,process.env.NODE_OPTIONS]))'], cwd: directory, artifact_dir: directory, run_id: 'target-env', runtime: 'node', env: { PATH: executableDirectory, HUNIN_ENV_TEST: 'requested-value', NODE_OPTIONS: '--stack-trace-limit=42' } });
    assert.ok(capture.env.NODE_OPTIONS.startsWith('--stack-trace-limit=42 '));
    assert.match(capture.env.NODE_OPTIONS, /--cpu-prof/);
    assert.match(capture.env.NODE_OPTIONS, /--import="file:\/\//, 'ESM preloads require file URLs on Windows');
    assert.equal(capture.env.PATH, executableDirectory);
    const result = spawnSync(capture.command[0], capture.command.slice(1), { cwd: directory, env: { ...process.env, ...capture.env }, encoding: 'utf8', timeout: 10000 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout)[0], 'requested-value');
    assert.ok(readdirSync(directory).some(name => name.endsWith('.cpuprofile')));
    assert.throws(() => plan({ command: [process.execPath], cwd: directory, artifact_dir: directory, runtime: 'node', env: { VALUE: 3 } }), /env must be/);
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

if (process.env.HUNIN_TEST_DENO) {
  test('Deno calibrates its CPU clock with only scoped Inspector access', () => {
    const directory = mkdtempSync(join(tmpdir(), 'hunin-deno-clock-'));
    try {
      const main = join(directory, 'main.mjs');
      writeFileSync(main, `console.log((await Deno.permissions.query({name:'sys',kind:'hostname'})).state); const end=Date.now()+100; while(Date.now()<end) Math.sqrt(42);`);
      const capture = plan({ command: [process.env.HUNIN_TEST_DENO, 'run', main], cwd: directory, artifact_dir: directory, run_id: 'calibrated-clock', runtime: 'deno' });
      const before = Date.now() * 1000;
      const result = spawnSync(capture.command[0], capture.command.slice(1), { cwd: directory, env: { ...process.env, ...capture.env }, encoding: 'utf8', timeout: 10000 });
      const after = Date.now() * 1000;
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.trim(), 'prompt');
      assert.equal(result.stderr, '');
      const names = readdirSync(directory);
      const clockName = names.find(name => name.startsWith('clock-') && name.endsWith('-0.meta.json'));
      assert.ok(clockName, 'missing per-process epoch calibration');
      const clock = JSON.parse(readFileSync(join(directory, clockName), 'utf8'));
      const profileName = names.find(name => name.endsWith(`.${clock.pid}.cpuprofile`));
      assert.ok(profileName, 'calibration has no matching process profile');
      const profile = JSON.parse(readFileSync(join(directory, profileName), 'utf8'));
      assert.ok(profile.startTime + clock.profile_time_origin_us >= before - clock.uncertainty_us);
      assert.ok(profile.endTime + clock.profile_time_origin_us <= after + clock.uncertainty_us);
      assert.ok(clock.uncertainty_us >= 0 && Number.isSafeInteger(clock.uncertainty_us));
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
  for (const deny of ['--deny-sys', '--deny-sys=inspector']) {
    test(`Deno preserves explicit ${deny} and captures with an unmapped clock`, () => {
      const directory = mkdtempSync(join(tmpdir(), 'hunin-deno-denied-clock-'));
      try {
        const main = join(directory, 'main.mjs');
        writeFileSync(main, `console.log((await Deno.permissions.query({name:'sys',kind:'inspector'})).state);`);
        const capture = plan({ command: [process.env.HUNIN_TEST_DENO, 'run', deny, main], cwd: directory, artifact_dir: directory, run_id: 'denied-clock', runtime: 'deno' });
        const result = spawnSync(capture.command[0], capture.command.slice(1), { cwd: directory, env: { ...process.env, ...capture.env }, encoding: 'utf8', timeout: 10000 });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout.trim(), 'denied');
        assert.match(result.stderr, /CPU clock calibration failed/);
        assert.ok(readdirSync(directory).some(name => name.endsWith('.cpuprofile')));
        assert.ok(!readdirSync(directory).some(name => name.endsWith('.meta.json')));
      } finally { rmSync(directory, { recursive: true, force: true }); }
    });
  }
}

if (process.env.HUNIN_TEST_BUN) {
  test('Bun inherited preload executes through a capture directory with spaces', () => {
    const directory = mkdtempSync(join(tmpdir(), 'hunin bun inherited preload '));
    try {
      const main = join(directory, 'main.mjs');
      writeFileSync(main, 'function inheritedHot(){const end=Date.now()+150;let value=0;while(Date.now()<end)for(let i=1;i<10000;i++)value+=Math.sqrt(i);return value;}console.log(inheritedHot());');
      const capture = plan({ command: [process.env.HUNIN_TEST_BUN, main], cwd: directory, artifact_dir: directory, run_id: 'inherited-preload', runtime: 'bun' });
      assert.match(capture.env.BUN_OPTIONS, /--preload=hunin-bun-preload-[\w-]+\.mjs$/);
      // A descendant inherits env, but not its parent's explicit launch argv.
      const result = spawnSync(process.env.HUNIN_TEST_BUN, [main], { cwd: directory, env: { ...process.env, ...capture.env }, encoding: 'utf8', timeout: 10000 });
      assert.equal(result.status, 0, result.stderr);
      const profiles = readdirSync(directory).filter(name => name.endsWith('.cpuprofile')).map(name => JSON.parse(readFileSync(join(directory, name), 'utf8')));
      assert.ok(profiles.some(profile => profile.nodes.some(node => node.callFrame.functionName === 'inheritedHot')), 'BUN_OPTIONS returned success without executing its profiler preload');
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
}
