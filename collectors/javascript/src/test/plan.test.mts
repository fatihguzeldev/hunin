import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
  symlinkSync,
  cpSync,
  mkdirSync,
  chmodSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname, toNamespacedPath, basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import type { Profiler } from 'node:inspector';
import { plan, toolPath } from '../host/plan.mjs';

const denoExecutable = process.env.HUNIN_TEST_DENO;
const bunExecutable = process.env.HUNIN_TEST_BUN;

interface LifecycleFinished {
  status: string;
  event: 'finished';
  pid: number;
  workers: Array<{ registration_id: string; worker_id: number | null }>;
  profiles: Array<{ worker_id: number; filename: string; sha256: string }>;
  diagnostics: string[];
}
interface LifecycleRecord {
  status: string;
  event?: string;
}
function isFinished(record: LifecycleRecord): record is LifecycleFinished {
  return record.event === 'finished';
}

function retainFailure(directory: string, error: Error) {
  const root =
    process.env.HUNIN_JS_TEST_EVIDENCE ??
    (process.env.GITHUB_ACTIONS === 'true'
      ? 'collector-evidence/javascript-workers'
      : undefined);
  if (!root) return;
  const destination = resolve(root, basename(directory));
  mkdirSync(dirname(destination), { recursive: true });
  cpSync(directory, destination, { recursive: true });
  writeFileSync(
    join(destination, 'failure.json'),
    JSON.stringify({ message: error.message, stack: error.stack }, null, 2),
  );
  process.stderr.write(
    `JavaScript collector failure evidence retained at ${destination}\n`,
  );
}

test('runtime tool paths normalize only Win32 verbatim disks and UNC shares', () => {
  assert.equal(
    toolPath('\\\\?\\C:\\work dir\\capture', 'win32'),
    'C:\\work dir\\capture',
  );
  assert.equal(
    toolPath('\\\\?\\UNC\\server\\share\\capture', 'win32'),
    '\\\\server\\share\\capture',
  );
  for (const value of [
    '\\\\.\\pipe\\hunin',
    '\\\\?\\Volume{guid}\\capture',
    'relative path',
    'C:\\ordinary\\path',
    'file:///C:/work/file.mjs',
  ]) {
    assert.equal(toolPath(value, 'win32'), value);
  }
  assert.equal(toolPath('\\\\?\\C:\\literal', 'darwin'), '\\\\?\\C:\\literal');
});

test('capture planning runs when the installed collector is reached through a directory link', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-linked-plan-'));
  try {
    const linked = join(directory, 'collector');
    symlinkSync(
      fileURLToPath(new URL('../', import.meta.url)),
      linked,
      process.platform === 'win32' ? 'junction' : 'dir',
    );
    const request = {
      command: [process.execPath, '-e', 'console.log(1)'],
      cwd: directory,
      artifact_dir: join(directory, 'capture'),
      run_id: 'linked',
      runtime: 'node',
    };
    const result = spawnSync(process.execPath, [join(linked, 'plan.mjs')], {
      input: JSON.stringify(request),
      encoding: 'utf8',
      timeout: 10000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.ok(
      result.stdout.trim(),
      'entry point silently produced no capture plan',
    );
    assert.deepEqual(JSON.parse(result.stdout).command, request.command);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('JavaScript runtime aliases and case preserve the selected Node executable', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-runtime-alias-'));
  try {
    const command = [process.execPath, '-e', 'console.log("selected-node")'];
    for (const runtime of [
      'node',
      'Node',
      'NODEJS',
      'javascript',
      'TypeScript',
    ]) {
      const capture = plan({
        command,
        cwd: directory,
        artifact_dir: directory,
        run_id: 'alias',
        runtime,
      });
      assert.deepEqual(capture.command, command, runtime);
      assert.equal(capture.runtime_version, process.version, runtime);
      assert.match(capture.env.NODE_OPTIONS, /--cpu-prof/, runtime);
      assert.deepEqual(capture.output, {
        kind: 'profiles',
        format: 'v8-cpuprofile',
      });
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('capture planning probes the selected renamed executable without a fallback on PATH', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-selected-runtime-'));
  try {
    const selected = join(
      directory,
      process.platform === 'win32'
        ? 'selected-runtime.exe'
        : 'selected-runtime',
    );
    if (process.platform === 'win32') cpSync(process.execPath, selected);
    else symlinkSync(process.execPath, selected);
    const command = [selected, '-e', 'console.log("selected-node")'];
    const capture = plan({
      command,
      cwd: directory,
      artifact_dir: directory,
      run_id: 'selected',
      runtime: 'node',
      env: { PATH: directory },
    });
    assert.deepEqual(capture.command, command);
    assert.equal(capture.runtime_version, process.version);
    const result = spawnSync(capture.command[0], capture.command.slice(1), {
      cwd: directory,
      env: { ...process.env, ...capture.env },
      encoding: 'utf8',
      timeout: 10000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'selected-node\n');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test(
  'selected executable version output must establish the supported runtime floor',
  { skip: process.platform === 'win32' },
  () => {
    const directory = mkdtempSync(join(tmpdir(), 'hunin-selected-version-'));
    try {
      const selected = join(directory, 'selected-runtime');
      writeFileSync(
        selected,
        '#!/bin/sh\nprintf \'%s\\n\' "$HUNIN_VERSION_TEST"\n',
      );
      chmodSync(selected, 0o755);
      const request = {
        command: [selected, '-e', 'console.log("target")'],
        cwd: directory,
        artifact_dir: directory,
        run_id: 'selected-version',
      };
      for (const runtime of ['node', 'bun', 'deno']) {
        for (const version of [
          '',
          'unavailable',
          'build 2026.10',
          'v22.16',
          '999999999999999999999999.0.0',
        ]) {
          assert.throws(
            () =>
              plan({
                ...request,
                runtime,
                env: { HUNIN_VERSION_TEST: version },
              }),
            /cannot parse .* version from selected executable/,
            `${runtime}: ${JSON.stringify(version)}`,
          );
        }
      }
      for (const [runtime, version, diagnostic] of [
        ['node', 'v19.9.0', /Node 20 or newer/],
        ['bun', '1.3.99', /Bun 1.4 or newer/],
        ['deno', 'deno 2.7.99 (stable, release, test)', /Deno 2.8 or newer/],
      ] as const) {
        assert.throws(
          () =>
            plan({
              ...request,
              runtime,
              env: { HUNIN_VERSION_TEST: version },
            }),
          diagnostic,
        );
      }
      for (const [runtime, version] of [
        ['node', 'v20.0.0'],
        ['node', 'v24.0.0-nightly20261029.1'],
        ['bun', '1.4.0'],
        ['bun', '1.4.4-canary.1+build'],
        ['deno', 'deno 2.8.0'],
        ['deno', 'deno 2.9.8 (stable, release, aarch64-apple-darwin)'],
        ['deno', 'deno 2.9.8 (stable, release, test)\r\nv8 14.0.0\r\n'],
      ] as const) {
        const capture = plan({
          ...request,
          runtime,
          env: { HUNIN_VERSION_TEST: version },
        });
        assert.equal(capture.runtime_version, version.split('\n')[0].trim());
        assert.equal(capture.command[0], selected);
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  },
);

test('target environment composes with profiling and selects the requested runtime through PATH', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin plan env '));
  try {
    const executableDirectory = join(directory, 'runtime');
    symlinkSync(
      dirname(process.execPath),
      executableDirectory,
      process.platform === 'win32' ? 'junction' : 'dir',
    );
    const capture = plan({
      command: [
        'node',
        '-e',
        'console.log(JSON.stringify([process.env.HUNIN_ENV_TEST,process.execArgv,process.env.NODE_OPTIONS]))',
      ],
      cwd: directory,
      artifact_dir: directory,
      run_id: 'target-env',
      runtime: 'node',
      env: {
        PATH: executableDirectory,
        HUNIN_ENV_TEST: 'requested-value',
        NODE_OPTIONS: '--stack-trace-limit=42',
      },
    });
    assert.ok(capture.env.NODE_OPTIONS.startsWith('--stack-trace-limit=42 '));
    assert.match(capture.env.NODE_OPTIONS, /--cpu-prof/);
    assert.match(
      capture.env.NODE_OPTIONS,
      /--import="file:\/\//,
      'ESM preloads require file URLs on Windows',
    );
    assert.equal(capture.env.PATH, executableDirectory);
    const result = spawnSync(capture.command[0], capture.command.slice(1), {
      cwd: directory,
      env: { ...process.env, ...capture.env },
      encoding: 'utf8',
      timeout: 10000,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(result.stdout)[0], 'requested-value');
    assert.ok(
      readdirSync(directory).some((name) => name.endsWith('.cpuprofile')),
    );
    assert.throws(
      () =>
        plan({
          command: [process.execPath],
          cwd: directory,
          artifact_dir: directory,
          runtime: 'node',
          // @ts-expect-error Exercise the external request boundary with an invalid override.
          env: { VALUE: 3 },
        }),
      /env must be/,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('host planner preserves inherited runtime options and explicit target precedence', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-inherited-options-'));
  try {
    const request = {
      command: [process.execPath, '-e', 'console.log(1)'],
      cwd: directory,
      artifact_dir: directory,
      run_id: 'inherited-options',
      runtime: 'node',
      inherited_runtime_options: { NODE_OPTIONS: '--stack-trace-limit=37' },
    };
    const inherited = plan(request);
    assert.equal(inherited.planner_protocol, 1);
    assert.ok(inherited.env.NODE_OPTIONS.startsWith('--stack-trace-limit=37 '));
    const overridden = plan({
      ...request,
      env: { NODE_OPTIONS: '--stack-trace-limit=42' },
    });
    assert.ok(
      overridden.env.NODE_OPTIONS.startsWith('--stack-trace-limit=42 '),
    );
    assert.equal(
      overridden.env.NODE_OPTIONS.includes('--stack-trace-limit=37'),
      false,
    );
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

for (const [runtime, executable] of [
  ['node', process.execPath],
  ['bun', bunExecutable],
  ['deno', denoExecutable],
] as Array<[string, string | undefined]>) {
  if (!executable) continue;
  test(`${runtime} automatic capture resolves CPU work in its worker`, () => {
    const directory = mkdtempSync(join(tmpdir(), `hunin-${runtime}-`));
    try {
      const worker = join(directory, 'worker.mjs');
      const main = join(directory, 'main.mjs');
      writeFileSync(
        worker,
        `function workerHot() { const end = Date.now()+250; let result=0; while(Date.now()<end) for(let n=1;n<10000;n++) result+=Math.sqrt(n); return result; } ${runtime === 'node' ? "import { parentPort } from 'node:worker_threads'; parentPort.postMessage(workerHot());" : 'postMessage(workerHot());'}`,
      );
      writeFileSync(
        main,
        `${runtime === 'node' ? "import { Worker } from 'node:worker_threads'; const w = new Worker(new URL('./worker.mjs', import.meta.url)); w.on('message', value=>console.log(value));" : "const w = new Worker(new URL('./worker.mjs',import.meta.url), {type:'module'}); w.onmessage=event=>{ console.log(event.data); w.terminate(); };"}`,
      );
      const command =
        runtime === 'deno'
          ? [executable, 'run', `--allow-read=${directory}`, main]
          : [executable, main];
      const capture = plan({
        command,
        cwd: directory,
        artifact_dir: directory,
        run_id: 'test',
        runtime,
      });
      const result = spawnSync(capture.command[0], capture.command.slice(1), {
        cwd: directory,
        env: { ...process.env, ...capture.env },
        encoding: 'utf8',
        timeout: 10000,
      });
      assert.equal(result.status, 0, result.stderr);
      const profiles = readdirSync(directory)
        .filter((name) => name.endsWith('.cpuprofile'))
        .map(
          (name): Profiler.Profile =>
            JSON.parse(readFileSync(join(directory, name), 'utf8')),
        );
      assert.ok(
        profiles.some((profile) =>
          profile.nodes.some(
            (node) => node.callFrame.functionName === 'workerHot',
          ),
        ),
        `no workerHot frame in ${profiles.length} profiles: ${result.stderr}`,
      );
    } catch (error) {
      retainFailure(directory, error as Error);
      throw error;
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

if (denoExecutable) {
  function denoLifecycleTest(name: string, run: (directory: string) => void) {
    test(name, () => {
      const directory = mkdtempSync(join(tmpdir(), 'hunin-deno-lifecycle-'));
      try {
        run(directory);
      } catch (error) {
        retainFailure(directory, error as Error);
        throw error;
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
  function captureDeno(
    directory: string,
    source: string,
    workerSource = 'postMessage(1);',
    permissions: string[] = [],
  ) {
    const artifacts = join(directory, 'capture');
    writeFileSync(join(directory, 'worker.mjs'), workerSource);
    writeFileSync(join(directory, 'main.mjs'), source);
    const command = [
      denoExecutable!,
      'run',
      `--allow-read=${directory}`,
      ...permissions,
      join(directory, 'main.mjs'),
    ];
    const capture = plan({
      command,
      cwd: directory,
      artifact_dir: artifacts,
      run_id: 'worker-lifecycle',
      runtime: 'deno',
    });
    assert.deepEqual(capture.output, {
      kind: 'deno-workers',
      producer_id: capture.env.HUNIN_DENO_WORKER_SESSION,
    });
    const result = spawnSync(capture.command[0], capture.command.slice(1), {
      cwd: directory,
      env: { ...process.env, ...capture.env },
      encoding: 'utf8',
      timeout: 10000,
    });
    writeFileSync(
      join(directory, 'target-result.json'),
      JSON.stringify(
        {
          status: result.status,
          stdout: result.stdout,
          stderr: result.stderr,
          error: result.error?.message,
        },
        null,
        2,
      ),
    );
    assert.equal(result.status, 0, result.stderr);
    const statusPath = join(
      artifacts,
      `deno-workers-${capture.env.HUNIN_DENO_WORKER_SESSION}.jsonl`,
    );
    const records = readFileSync(statusPath, 'utf8')
      .trim()
      .split('\n')
      .map((line): LifecycleRecord => JSON.parse(line));
    return {
      result,
      records,
      artifacts,
      terminal: records.findLast(isFinished),
    };
  }
  const workerMain = `const w=new Worker(new URL('./worker.mjs',import.meta.url),{type:'module'});w.onmessage=e=>{console.log(e.data);w.terminate();};`;

  denoLifecycleTest(
    'Deno exit waits for a large exact worker profile without changing terminate',
    (directory) => {
      // A large sampled stack makes native serialization outlast the main
      // isolate's cleanup, reproducing missing/empty output without timing sleeps.
      const name = 'deep_' + 'x'.repeat(32000);
      const worker = `function workerHot(){const end=Date.now()+150;let value=0;while(Date.now()<end)for(let n=1;n<10000;n++)value+=Math.sqrt(n);return value;}function ${name}(n){return n?${name}(n-1):workerHot();}postMessage(${name}(1200));`;
      const { result, records, terminal, artifacts } = captureDeno(
        directory,
        workerMain,
        worker,
      );
      assert.ok(Number(result.stdout.trim()) > 0);
      assert.equal(result.stderr, '');
      assert.equal(terminal?.status, 'complete');
      assert.ok(terminal);
      assert.equal(terminal.workers.length, 1);
      assert.equal(terminal.profiles.length, 1);
      assert.equal(terminal.workers[0].worker_id, 1);
      assert.equal(
        records.filter((row) => row.event === 'worker_registered').length,
        1,
      );
      const profile = terminal.profiles[0];
      assert.equal(profile.worker_id, 1);
      assert.match(
        profile.filename,
        new RegExp(`^CPU\\.\\d+\\.${terminal.pid}\\.worker-1\\.cpuprofile$`),
      );
      const bytes = readFileSync(join(artifacts, profile.filename));
      assert.ok(
        bytes.length > 1024 * 1024,
        'control failed to exercise a large native profile write',
      );
      assert.equal(
        createHash('sha256').update(bytes).digest('hex'),
        profile.sha256,
      );
      assert.ok(
        (JSON.parse(bytes.toString('utf8')) as Profiler.Profile).nodes.some(
          (node) => node.callFrame.functionName === 'workerHot',
        ),
      );
    },
  );

  denoLifecycleTest(
    'Deno natural worker close completes its profile barrier',
    (directory) => {
      const { terminal, result } = captureDeno(
        directory,
        `const w=new Worker(new URL('./worker.mjs',import.meta.url),{type:'module'});w.onmessage=e=>console.log(e.data);`,
        'postMessage(1);close();',
      );
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'complete');
      assert.ok(terminal);
      assert.equal(terminal.profiles.length, 1);
    },
  );

  denoLifecycleTest(
    'Deno barrier uses runtime worker IDs rather than constructor counts',
    (directory) => {
      const source = `const NativeWorker=Object.getPrototypeOf(Worker);const first=new NativeWorker(new URL('./worker.mjs',import.meta.url),{type:'module'});first.onmessage=()=>{first.terminate();${workerMain}};`;
      const { terminal, result } = captureDeno(
        directory,
        source,
        `const end=Date.now()+50;while(Date.now()<end){}postMessage(1);`,
      );
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'complete');
      assert.ok(terminal);
      assert.deepEqual(
        terminal.workers.map((row) => row.worker_id),
        [2],
      );
      assert.deepEqual(
        terminal.profiles.map((row) => row.worker_id),
        [2],
      );
    },
  );

  denoLifecycleTest(
    'Deno handled worker startup error validates the emitted profile',
    (directory) => {
      const { terminal, result, artifacts } = captureDeno(
        directory,
        `const w=new Worker(new URL('./worker.mjs',import.meta.url),{type:'module'});w.onerror=e=>{e.preventDefault();console.log('handled');};`,
        `throw Error('expected worker startup failure');`,
      );
      assert.equal(result.stdout.trim(), 'handled');
      assert.equal(terminal?.status, 'complete');
      assert.ok(terminal);
      assert.equal(terminal.profiles.length, 1);
      assert.ok(
        JSON.parse(
          readFileSync(join(artifacts, terminal.profiles[0].filename), 'utf8'),
        ).nodes.length > 0,
      );
    },
  );

  denoLifecycleTest(
    'Deno lifecycle deadline rejects a missing exact worker profile',
    (directory) => {
      // Simulate a changed native ID contract: an unrelated completed profile
      // must never satisfy the registry just because its file count matches.
      const source = `import inspector from 'node:inspector';const post=inspector.Session.prototype.post;inspector.Session.prototype.post=function(method,...args){if(method==='Runtime.getProperties'){args.at(-1)(null,{privateProperties:[{name:'#id',value:{value:2}}]});return;}return post.call(this,method,...args);};${workerMain}`;
      const { terminal, result, artifacts } = captureDeno(directory, source);
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'incomplete');
      assert.ok(terminal);
      assert.deepEqual(
        terminal.workers.map((row) => row.worker_id),
        [2],
      );
      assert.ok(
        readdirSync(artifacts).some((name) =>
          name.endsWith('.worker-1.cpuprofile'),
        ),
      );
      assert.match(
        terminal.diagnostics.join(' '),
        /profile flush did not complete within 2000ms/,
      );
    },
  );

  denoLifecycleTest(
    'Deno denied Inspector access cannot claim tracked worker completion',
    (directory) => {
      const { terminal, result } = captureDeno(
        directory,
        workerMain,
        'postMessage(1);',
        ['--deny-sys=inspector'],
      );
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'incomplete');
      assert.ok(terminal);
      assert.match(
        terminal.diagnostics.join(' '),
        /worker identity unavailable/,
      );
      assert.equal(terminal.workers[0].worker_id, null);
    },
  );

  denoLifecycleTest(
    'Deno missing private worker identity never falls back to a guessed ID',
    (directory) => {
      const source = `import inspector from 'node:inspector';const post=inspector.Session.prototype.post;inspector.Session.prototype.post=function(method,...args){if(method==='Runtime.getProperties'){args.at(-1)(null,{privateProperties:[]});return;}return post.call(this,method,...args);};${workerMain}`;
      const { terminal, result } = captureDeno(directory, source);
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'incomplete');
      assert.ok(terminal);
      assert.equal(terminal.workers[0].worker_id, null);
      assert.equal(terminal.profiles.length, 0);
      assert.match(
        terminal.diagnostics.join(' '),
        /Worker.#id is unavailable or invalid/,
      );
    },
  );

  denoLifecycleTest(
    'Deno denied profile reads preserve target output and mark collection incomplete',
    (directory) => {
      const source = `const w=new Worker(new URL('data:application/javascript,postMessage(1)'),{type:'module'});w.onmessage=e=>{console.log(e.data);w.terminate();};`;
      const { terminal, result } = captureDeno(directory, source, '', [
        `--deny-read=${join(directory, 'capture')}`,
      ]);
      assert.equal(result.stdout.trim(), '1');
      assert.equal(terminal?.status, 'incomplete');
      assert.ok(terminal);
      assert.match(
        terminal.diagnostics.join(' '),
        /worker profile lifecycle unavailable.*[Rr]equires read access/,
      );
    },
  );

  denoLifecycleTest(
    'Deno forced main exit never claims a completed worker lifecycle',
    (directory) => {
      const { records, terminal, result } = captureDeno(
        directory,
        `console.log('forced');Deno.exit(0);`,
      );
      assert.equal(result.stdout.trim(), 'forced');
      assert.equal(records[0].status, 'planned');
      assert.notEqual(terminal?.status, 'complete');
    },
  );

  denoLifecycleTest(
    'Deno denied lifecycle environment preserves target execution',
    (directory) => {
      const { result, records, terminal } = captureDeno(
        directory,
        'console.log("target-ran");',
        '',
        ['--deny-env=HUNIN_DENO_WORKER_SESSION'],
      );
      assert.equal(result.stdout.trim(), 'target-ran');
      assert.match(result.stderr, /Deno worker lifecycle setup unavailable/);
      assert.equal(records[0].status, 'planned');
      assert.notEqual(terminal?.status, 'complete');
    },
  );

  denoLifecycleTest(
    'Deno lifecycle deadline bounds validation and resets only after a completed phase',
    (directory) => {
      const module = new URL('../agents/deno-workers.mjs', import.meta.url)
        .href;
      const producerId = 'zero-budget-control';
      writeFileSync(
        join(directory, `deno-workers-${producerId}.jsonl`),
        JSON.stringify({
          schema_version: 1,
          runtime: 'deno',
          run_id: 'zero-budget',
          producer_id: producerId,
          status: 'planned',
        }) + '\n',
      );
      writeFileSync(join(directory, 'worker.mjs'), 'postMessage(1);');
      writeFileSync(
        join(directory, 'zero.mjs'),
        `import {installDenoWorkerBarrier} from ${JSON.stringify(module)};installDenoWorkerBarrier({directory:${JSON.stringify(directory)},runId:'zero-budget',producerId:${JSON.stringify(producerId)},timeoutMs:0});${workerMain}`,
      );
      const zero = spawnSync(
        denoExecutable!,
        [
          'run',
          '--cpu-prof',
          `--cpu-prof-dir=${directory}`,
          `--allow-read=${directory}`,
          `--allow-write=${directory}`,
          '--allow-sys=inspector',
          join(directory, 'zero.mjs'),
        ],
        { encoding: 'utf8', timeout: 10000 },
      );
      assert.equal(zero.status, 0, zero.stderr);
      assert.equal(zero.stdout.trim(), '1');
      const terminal = readFileSync(
        join(directory, `deno-workers-${producerId}.jsonl`),
        'utf8',
      )
        .trim()
        .split('\n')
        .map((line) => JSON.parse(line))
        .at(-1);
      assert.equal(terminal.status, 'incomplete');
      assert.match(
        terminal.diagnostics.join(' '),
        /deadline expired after 0ms/,
      );

      // Advance a controlled main-isolate clock after a user beforeExit hook
      // restarts real worker work. Reusing the previous completed phase's
      // deadline would reject this new shutdown even though its profile is ready.
      const source = `import process from 'node:process';const now=performance.now.bind(performance);let offset=0;performance.now=()=>now()+offset;process.once('beforeExit',()=>{const w=new Worker(new URL('./worker.mjs',import.meta.url),{type:'module'});w.onmessage=e=>{offset=3000;console.log(e.data);w.terminate();};});`;
      const next = captureDeno(directory, source);
      assert.equal(next.result.stdout.trim(), '1');
      assert.equal(next.terminal?.status, 'complete');
      assert.ok(next.terminal);
      assert.equal(next.terminal.profiles.length, 1);
    },
  );

  test('Deno calibrates its CPU clock with only scoped Inspector access', () => {
    const directory = mkdtempSync(join(tmpdir(), 'hunin-deno-clock-'));
    try {
      const main = join(directory, 'main.mjs');
      writeFileSync(
        main,
        `console.log((await Deno.permissions.query({name:'sys',kind:'hostname'})).state); const end=Date.now()+100; while(Date.now()<end) Math.sqrt(42);`,
      );
      const capture = plan({
        command: [denoExecutable!, 'run', main],
        cwd: directory,
        artifact_dir: directory,
        run_id: 'calibrated-clock',
        runtime: 'deno',
      });
      const before = Date.now() * 1000;
      const result = spawnSync(capture.command[0], capture.command.slice(1), {
        cwd: directory,
        env: { ...process.env, ...capture.env },
        encoding: 'utf8',
        timeout: 10000,
      });
      const after = Date.now() * 1000;
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.trim(), 'prompt');
      assert.equal(result.stderr, '');
      const names = readdirSync(directory);
      const clockName = names.find(
        (name) => name.startsWith('clock-') && name.endsWith('-0.meta.json'),
      );
      assert.ok(clockName, 'missing per-process epoch calibration');
      const clock = JSON.parse(
        readFileSync(join(directory, clockName), 'utf8'),
      );
      const profileName = names.find((name) =>
        name.endsWith(`.${clock.pid}.cpuprofile`),
      );
      assert.ok(profileName, 'calibration has no matching process profile');
      const profile = JSON.parse(
        readFileSync(join(directory, profileName), 'utf8'),
      );
      assert.ok(
        profile.startTime + clock.profile_time_origin_us >=
          before - clock.uncertainty_us,
      );
      assert.ok(
        profile.endTime + clock.profile_time_origin_us <=
          after + clock.uncertainty_us,
      );
      assert.ok(
        clock.uncertainty_us >= 0 && Number.isSafeInteger(clock.uncertainty_us),
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
  for (const deny of ['--deny-sys', '--deny-sys=inspector']) {
    test(`Deno preserves explicit ${deny} and captures with an unmapped clock`, () => {
      const directory = mkdtempSync(join(tmpdir(), 'hunin-deno-denied-clock-'));
      try {
        const main = join(directory, 'main.mjs');
        writeFileSync(
          main,
          `console.log((await Deno.permissions.query({name:'sys',kind:'inspector'})).state);`,
        );
        const capture = plan({
          command: [denoExecutable!, 'run', deny, main],
          cwd: directory,
          artifact_dir: directory,
          run_id: 'denied-clock',
          runtime: 'deno',
        });
        const result = spawnSync(capture.command[0], capture.command.slice(1), {
          cwd: directory,
          env: { ...process.env, ...capture.env },
          encoding: 'utf8',
          timeout: 10000,
        });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout.trim(), 'denied');
        assert.match(result.stderr, /CPU clock calibration failed/);
        assert.ok(
          readdirSync(directory).some((name) => name.endsWith('.cpuprofile')),
        );
        assert.ok(
          !readdirSync(directory).some((name) => name.endsWith('.meta.json')),
        );
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
}

if (bunExecutable!) {
  for (const failSave of [false, true]) {
    test(`Bun final save ${failSave ? 'failure is never acknowledged as flushed' : 'is acknowledged after profile and clock persistence'}`, () => {
      const directory = mkdtempSync(join(tmpdir(), 'hunin-bun-flush-result-'));
      try {
        const main = join(directory, 'flush.ts');
        writeFileSync(
          main,
          `
          import inspector from 'node:inspector';
          import { readdirSync } from 'node:fs';
          import { threadId } from 'node:worker_threads';
          ${
            failSave
              ? `
            const original = inspector.Session.prototype.post;
            inspector.Session.prototype.post = function(method, ...args) {
              if (method === 'Profiler.stop') {
                args.at(-1)(Error('injected final save failure'));
                return;
              }
              return original.call(this, method, ...args);
            };
          `
              : ''
          }
          const channel = new BroadcastChannel('hunin-cpu-' + process.pid + '-' + threadId);
          const timeout = setTimeout(() => { console.error('flush response missing'); process.exit(2); }, 3000);
          channel.onmessage = event => {
            clearTimeout(timeout);
            const names = readdirSync(process.env.HUNIN_JS_PROFILE_DIR);
            console.log(JSON.stringify({ message: event.data, profiles: names.filter(name => name.endsWith('.cpuprofile')).map(name => ({ name, hasClock: names.includes(name + '.meta.json') })) }));
            channel.close();
          };
          channel.postMessage('flush-stop');
        `,
        );
        const capture = plan({
          command: [bunExecutable!, main],
          cwd: directory,
          artifact_dir: join(directory, 'capture'),
          run_id: 'flush-result',
          runtime: 'bun',
        });
        const result = spawnSync(capture.command[0], capture.command.slice(1), {
          cwd: directory,
          env: { ...process.env, ...capture.env },
          encoding: 'utf8',
          timeout: 10000,
        });
        assert.equal(result.status, 0, result.stderr);
        const response: {
          message: string;
          profiles: Array<{ name: string; hasClock: boolean }>;
        } = JSON.parse(result.stdout);
        assert.equal(response.message, failSave ? 'flush-failed' : 'flushed');
        if (failSave)
          assert.match(result.stderr, /injected final save failure/);
        else {
          assert.ok(response.profiles.length > 0);
          assert.ok(response.profiles.every((profile) => profile.hasClock));
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }

  test('Bun plans declare distinct generated preload identities without replacing user options', () => {
    const directory = mkdtempSync(join(tmpdir(), 'hunin-bun-session-'));
    try {
      const request = {
        command: [bunExecutable!, '-e', 'console.log(1)'],
        cwd: directory,
        run_id: 'bun-session',
        runtime: 'bun',
        env: { BUN_OPTIONS: '--smol' },
      };
      const first = plan({
        ...request,
        artifact_dir: join(directory, 'first'),
      });
      const second = plan({
        ...request,
        artifact_dir: join(directory, 'second'),
      });
      assert.notEqual(
        first.session_identity['bun-preload'],
        second.session_identity['bun-preload'],
      );
      for (const capture of [first, second]) {
        assert.equal(
          capture.env.BUN_OPTIONS,
          `--smol --preload=${capture.session_identity['bun-preload']}`,
        );
        assert.match(
          capture.session_identity['bun-preload'],
          /^hunin-bun-preload-[\w-]+\.mjs$/,
        );
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  test('Bun inherited preload executes through a capture directory with spaces', () => {
    const directory = mkdtempSync(
      join(tmpdir(), 'hunin bun inherited preload '),
    );
    try {
      const main = join(directory, 'main.mjs');
      writeFileSync(
        main,
        'function inheritedHot(){const end=Date.now()+150;let value=0;while(Date.now()<end)for(let i=1;i<10000;i++)value+=Math.sqrt(i);return value;}console.log(inheritedHot());',
      );
      const capture = plan({
        command: [toNamespacedPath(bunExecutable!), main],
        cwd: toNamespacedPath(directory),
        artifact_dir: toNamespacedPath(directory),
        run_id: 'inherited-preload',
        runtime: 'bun',
      });
      if (process.platform === 'win32')
        assert.equal(
          capture.env.NODE_PATH.startsWith('\\\\?\\'),
          false,
          'Bun cannot resolve a verbatim NODE_PATH',
        );
      assert.match(
        capture.env.BUN_OPTIONS,
        /--preload=hunin-bun-preload-[\w-]+\.mjs$/,
      );
      // A descendant inherits env, but not its parent's explicit launch argv.
      const result = spawnSync(bunExecutable!, [main], {
        cwd: directory,
        env: { ...process.env, ...capture.env },
        encoding: 'utf8',
        timeout: 10000,
      });
      assert.equal(result.status, 0, result.stderr);
      const profiles = readdirSync(directory)
        .filter((name) => name.endsWith('.cpuprofile'))
        .map(
          (name): Profiler.Profile =>
            JSON.parse(readFileSync(join(directory, name), 'utf8')),
        );
      assert.ok(
        profiles.some((profile) =>
          profile.nodes.some(
            (node) => node.callFrame.functionName === 'inheritedHot',
          ),
        ),
        'BUN_OPTIONS returned success without executing its profiler preload',
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}
