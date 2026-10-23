import assert from 'node:assert/strict';
import { mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const entry = fileURLToPath(new URL('./index.mjs', import.meta.url));
function run(code, env = {}) {
  return spawnSync(process.env.HUNIN_SDK_TEST_RUNTIME || process.execPath, [...(process.env.HUNIN_SDK_TEST_DENO ? ['eval', '--allow-env', '--allow-write', '--allow-read'] : ['--input-type=module', '-e']), `import assert from "node:assert/strict"; import * as h from ${JSON.stringify(entry)}; ${code}`], {
    encoding: 'utf8', env: { ...process.env, HUNIN_RUN_ID: '', HUNIN_RECORD_DIR: '', ...env },
  });
}
test('disabled instrumentation preserves values and exceptions', () => {
  const result = run(`assert.equal(await h.withSpan('work', async () => 42), 42); try { h.withSpan('failure', () => { throw Error('expected'); }); } catch(e) { assert.equal(e.message, 'expected'); } h.counter('jobs', 1); h.flush();`, {});
  // Keep assertions inside target script independent of SDK dependencies.
  assert.equal(result.status, 0, result.stderr);
});
test('concurrent asynchronous spans preserve independent parents and survive explicit flush', () => {
  const dir = mkdtempSync(join(tmpdir(), 'hunin-js-sdk-'));
  try {
    const result = run(`await Promise.all(['a', 'b'].map(name => h.withSpan(name, async () => { await new Promise(resolve => setTimeout(resolve, name === 'a' ? 2 : 1)); await h.withSpan(name + '-child', async () => h.counter('items', 1, 'count')); }))); h.flush();`, { HUNIN_RUN_ID: 'test-run', HUNIN_RECORD_DIR: dir });
    assert.equal(result.status, 0, result.stderr);
    const rows = readdirSync(dir).filter(name => name.endsWith('.jsonl')).flatMap(name => readFileSync(join(dir, name), 'utf8').trim().split('\n').map(JSON.parse));
    const spans = rows.filter(row => row.kind === 'span');
    assert.equal(spans.length, 4);
    for (const prefix of ['a', 'b']) {
      const parent = spans.find(row => row.name === prefix);
      const child = spans.find(row => row.name === prefix + '-child');
      assert.equal(child.parent_id, parent.id);
      assert.ok(child.timestamp_us >= parent.timestamp_us);
      assert.ok(child.duration_us <= parent.duration_us);
    }
    assert.equal(rows.filter(row => row.kind === 'metric').length, 2);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
