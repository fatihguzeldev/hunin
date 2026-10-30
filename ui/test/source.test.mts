import test from 'node:test';
import assert from 'node:assert/strict';
import { operation } from '../src/api.js';
import { hasLocalSource, hasInspectableSource } from '../src/source/paths.js';

test('source responses preserve nullable provenance while checking the measured-source envelope', async () => {
  const original = globalThis.fetch;
  const result = {
    path: 'runtime://script',
    content: 'retained',
    start_line: 1,
    identity_verified: true,
    source_kind: 'recorded_source_map',
    build_mapping_verified: false,
    diagnostic: 'Mapping remains unverified',
    provenance: null,
  };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(
      await operation('source.read', { run_id: 'r', path: result.path }),
      result,
    );
    const continued = {
      ...result,
      start_line: 5001,
      next_line: 5121,
      total_lines: 6003,
    };
    globalThis.fetch = async () => Response.json({ result: continued });
    assert.deepEqual(
      await operation('source.read', {
        run_id: 'r',
        path: result.path,
        line: 5001,
      }),
      continued,
    );
    globalThis.fetch = async () =>
      Response.json({ result: { ...continued, next_line: null } });
    assert.equal(
      (await operation('source.read', { run_id: 'r', path: result.path }))
        .next_line,
      null,
    );
    globalThis.fetch = async () =>
      Response.json({ result: { ...continued, next_line: 0 } });
    await assert.rejects(
      operation('source.read', { run_id: 'r', path: result.path }),
      /invalid source.read result/,
    );
    globalThis.fetch = async () =>
      Response.json({ result: { ...result, start_line: 'one' } });
    await assert.rejects(
      operation('source.read', { run_id: 'r', path: result.path }),
      /invalid source.read result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('virtual runtime source locations are not offered as local filesystem paths', async () => {
  for (const local of [
    '/src/main.rs',
    'src/main.py',
    'file:///src/a%20b.js',
    'C:\\src\\main.cpp',
  ])
    assert.equal(hasLocalSource(local), true);
  for (const virtual of [
    null,
    undefined,
    '',
    'node:internal/loader',
    'wasm:wasm-function[1]',
    'https://example.test/app.js',
  ])
    assert.equal(hasLocalSource(virtual), false);
  for (const retained of [
    'webpack://app/src/main.ts',
    'https://example.test/src.ts',
  ]) {
    assert.equal(hasInspectableSource(retained), false);
    assert.equal(
      hasInspectableSource(retained, 'measured-script-sha256'),
      true,
    );
  }
  for (const internal of ['node:internal/loader', 'wasm:wasm-function[1]'])
    assert.equal(hasInspectableSource(internal, 'code-id'), false);
});
