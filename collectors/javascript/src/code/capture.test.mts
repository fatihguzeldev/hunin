import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { plan } from '../host/plan.mjs';
import type { CodeOptions } from './plan.mjs';

function fixtureOutput(
  stdout: string,
  stderr = '',
): { runs: number; checksums: number[] } {
  const diagnostic = () =>
    `stdout (last 64 KiB):\n${stdout.slice(-65_536)}\nstderr (last 8 KiB):\n${stderr.slice(-8192)}`;
  const marker = '{"fixture":"independent-wasm-emitter-v1",';
  const start = stdout.indexOf(marker);
  assert.ok(start >= 0, `Target result absent. ${diagnostic()}`);
  assert.equal(
    stdout.indexOf(marker, start + marker.length),
    -1,
    `Target result duplicated. ${diagnostic()}`,
  );
  // This fixture emits a flat object. Parse its exact bytes; tracing may
  // surround it, but damaged/interleaved JSON must still fail.
  const end = stdout.indexOf('}', start);
  assert.ok(end >= 0, `Target result truncated. ${diagnostic()}`);
  try {
    return JSON.parse(stdout.slice(start, end + 1));
  } catch {
    assert.fail(`Target result is invalid JSON. ${diagnostic()}`);
  }
}

test('fixture output can be surrounded by tracing on the same line', () => {
  const target =
    '{"fixture":"independent-wasm-emitter-v1","runs":3,"checksums":[-128642144]}';
  assert.deepEqual(
    fixtureOutput(`V8 tracing prefix ${target} trace suffix\n`),
    {
      fixture: 'independent-wasm-emitter-v1',
      runs: 3,
      checksums: [-128642144],
    },
  );
});

test('fixture output rejects truncated and duplicate target results', () => {
  const target =
    '{"fixture":"independent-wasm-emitter-v1","runs":3,"checksums":[-128642144]}';
  assert.throws(() => fixtureOutput(`trace ${target.slice(0, -1)}`));
  assert.throws(() => fixtureOutput(`${target}\ntrace ${target}`));
  assert.throws(() =>
    fixtureOutput(target.slice(0, 50) + 'V8 trace' + target.slice(50)),
  );
});

function capture(command: string[], codeOptions?: CodeOptions) {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-code-capture-'));
  const planned = plan({
    command: [process.execPath, ...command],
    runtime: 'node',
    cwd: directory,
    artifact_dir: directory,
    run_id: 'independent-code-capture',
    capture_mode: 'code-origin',
    code_options: codeOptions,
  });
  const result = spawnSync(planned.command[0], planned.command.slice(1), {
    cwd: directory,
    env: { ...process.env, ...planned.env },
    encoding: 'utf8',
    timeout: 10000,
  });
  assert.equal(result.status, 0, result.stderr);
  const filename = readdirSync(directory).find((name) =>
    name.endsWith('.v8code.json'),
  );
  assert.ok(filename);
  const manifest = JSON.parse(
    readFileSync(join(directory, filename), 'utf8'),
  ) as {
    logs: Array<{ path: string; ll_path: string }>;
    wasm: Array<{ path: string; sha256: string }>;
    compiler_exports: unknown[];
    diagnostics: string[];
    runtime: { exec_argv: string[] };
    compiler_scope?: {
      requested_wasm_function_index: number | null;
      requested_scope: string;
    };
  };
  return { directory, manifest, result };
}

test('real default code capture keeps stdout and retains native PCs plus exact live Wasm bytes', () => {
  const fixture = fileURLToPath(
    new URL('./fixtures/workload.mjs', import.meta.url),
  );
  const { directory, manifest, result } = capture([fixture, '30']);
  try {
    assert.equal(
      JSON.parse(result.stdout).fixture,
      'independent-wasm-emitter-v1',
    );
    assert.equal(manifest.compiler_exports.length, 0);
    assert.ok(manifest.logs.length >= 1);
    assert.match(
      readFileSync(join(directory, manifest.logs[0].path), 'utf8'),
      /^tick,0x[0-9a-f]+,/m,
    );
    assert.ok(
      readFileSync(join(directory, manifest.logs[0].ll_path)).length > 0,
    );
    assert.equal(manifest.wasm.length, 2);
    assert.notEqual(manifest.wasm[0].sha256, manifest.wasm[1].sha256);
    for (const module of manifest.wasm) {
      const bytes = readFileSync(join(directory, module.path));
      assert.equal(
        createHash('sha256').update(bytes).digest('hex'),
        module.sha256,
      );
      assert.equal(WebAssembly.validate(bytes), true);
    }
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('forced process exit reports missing post-workload custody without fabricating modules', () => {
  const { directory, manifest } = capture(['-e', 'process.exit(0)']);
  try {
    assert.equal(manifest.wasm.length, 0);
    assert.ok(
      manifest.diagnostics.some((message) =>
        message.includes('before the post-workload'),
      ),
    );
    assert.ok(manifest.logs.length >= 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('indexed compiler capture retains requested scope, startup flags and exact Wasm bytes', () => {
  const fixture = fileURLToPath(
    new URL('./fixtures/workload.mjs', import.meta.url),
  );
  const { directory, manifest, result } = capture([fixture, '300', 'single'], {
    compiler_export: true,
    function_filter: 'generated_*',
    wasm_function_index: 1,
  });
  try {
    const names = Array.from(result.stdout.matchAll(/^name: (.+)$/gm), (m) =>
      m[1].trim(),
    );
    assert.ok(names.includes('generated_helper'));
    assert.ok(!names.includes('generated_kernel'));
    const output = fixtureOutput(result.stdout, result.stderr);
    assert.deepEqual(output.checksums, [-128642144]);
    assert.ok(Number.isSafeInteger(output.runs) && output.runs > 0);
    assert.deepEqual(manifest.compiler_scope, {
      requested_wasm_function_index: 1,
      requested_scope: 'function_index_across_modules',
    });
    assert.ok(
      manifest.runtime.exec_argv.includes('--print-wasm-code-function-index=1'),
    );
    assert.equal(manifest.wasm.length, 1);
    const retained = readFileSync(join(directory, manifest.wasm[0].path));
    assert.equal(WebAssembly.validate(retained), true);
    assert.equal(
      createHash('sha256').update(retained).digest('hex'),
      manifest.wasm[0].sha256,
    );
    assert.ok(manifest.compiler_exports.length >= 1);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
