import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import {
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

function run(
  workload: 'disabled' | 'concurrent' | 'write-error',
  env: NodeJS.ProcessEnv = {},
) {
  const path = fileURLToPath(
    new URL(`./test/${workload}.mjs`, import.meta.url),
  );
  const flags = process.env.HUNIN_SDK_TEST_DENO
    ? ['run', '--allow-env', '--allow-write', '--allow-read']
    : [];
  return spawnSync(
    process.env.HUNIN_SDK_TEST_RUNTIME || process.execPath,
    [...flags, path],
    {
      encoding: 'utf8',
      timeout: 10000,
      env: { ...process.env, HUNIN_RUN_ID: '', HUNIN_RECORD_DIR: '', ...env },
    },
  );
}

function isRow(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

test('disabled instrumentation preserves values and exceptions', () => {
  const result = run('disabled');
  // Assertions also run inside the target, independently of the SDK internals.
  assert.equal(result.status, 0, result.stderr);
});

test('write failure disables new propagation tokens without changing target behavior', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-sdk-write-error-'));
  try {
    const blocked = join(directory, 'not-a-directory');
    writeFileSync(blocked, 'blocked-sdk-output');
    const result = run('write-error', {
      HUNIN_RUN_ID: 'test-run',
      HUNIN_RECORD_DIR: blocked,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'target-ok\n');
    assert.deepEqual(readdirSync(directory), ['not-a-directory']);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('falsy serialization failures remain sticky and disable propagation', () => {
  const directory = mkdtempSync(
    join(tmpdir(), 'hunin-sdk-serialization-error-'),
  );
  try {
    const result = run('write-error', {
      HUNIN_RUN_ID: 'test-run',
      HUNIN_RECORD_DIR: directory,
      HUNIN_SDK_FAILURE_TEST: 'serialize',
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'target-ok\n');
    assert.deepEqual(readdirSync(directory), []);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('automatic flush failure does not return an undeliverable observation ID', () => {
  const directory = mkdtempSync(join(tmpdir(), 'hunin-sdk-auto-flush-error-'));
  try {
    const blocked = join(directory, 'not-a-directory');
    writeFileSync(blocked, 'blocked-sdk-output');
    const result = run('write-error', {
      HUNIN_RUN_ID: 'test-run',
      HUNIN_RECORD_DIR: blocked,
      HUNIN_SDK_FAILURE_TEST: 'auto',
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, 'target-ok\n');
    assert.deepEqual(readdirSync(directory), ['not-a-directory']);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('concurrent asynchronous spans preserve independent parents and survive explicit flush', () => {
  const dir = mkdtempSync(join(tmpdir(), 'hunin-js-sdk-'));
  try {
    const result = run('concurrent', {
      HUNIN_RUN_ID: 'test-run',
      HUNIN_RECORD_DIR: dir,
    });
    assert.equal(result.status, 0, result.stderr);
    const rows = readdirSync(dir)
      .filter((name) => name.endsWith('.jsonl'))
      .flatMap((name) =>
        readFileSync(join(dir, name), 'utf8')
          .trim()
          .split('\n')
          .map((line) => {
            const row: unknown = JSON.parse(line);
            assert.ok(isRow(row));
            return row;
          }),
      );
    const spans = rows.filter((row) => row.kind === 'span');
    assert.equal(spans.length, 4);
    for (const prefix of ['a', 'b']) {
      const parent = spans.find((row) => row.name === prefix);
      const child = spans.find((row) => row.name === prefix + '-child');
      assert.ok(parent && child);
      assert.equal(child.parent_id, parent.id);
      assert.ok(
        typeof child.timestamp_us === 'number' &&
          typeof parent.timestamp_us === 'number',
      );
      assert.ok(
        typeof child.duration_us === 'number' &&
          typeof parent.duration_us === 'number',
      );
      assert.ok(child.timestamp_us >= parent.timestamp_us);
      assert.ok(child.duration_us <= parent.duration_us);
    }
    assert.equal(rows.filter((row) => row.kind === 'metric').length, 2);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
