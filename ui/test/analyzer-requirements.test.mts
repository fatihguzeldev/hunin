import test from 'node:test';
import assert from 'node:assert/strict';
import { analyzerRequirements } from '../src/analysis/requirements.js';

test('source-specific analyzers require a known fingerprint without treating it as a build mapping', () => {
  const source = {
    fingerprint: 'source-v1',
    executable_sha256: 'exe',
    runtime_version: 'v24',
  };
  assert.deepEqual(analyzerRequirements('recording', 'node', source), {
    runtime: 'node',
    source_fingerprint: 'source-v1',
    executable_sha256: 'exe',
    runtime_version: 'v24',
  });
  for (const fingerprint of ['', 'unknown'])
    assert.throws(
      () => analyzerRequirements('recording', 'node', { fingerprint }),
      /source fingerprint/,
    );
});

test('source independence is explicit and does not inherit source-specific requirements', () => {
  assert.deepEqual(
    analyzerRequirements('independent', '*', {
      fingerprint: 'unknown',
      executable_sha256: 'exe',
      runtime_version: 'v24',
    }),
    { runtime: '*', source_independent: true },
  );
  assert.throws(
    () => analyzerRequirements('independent', '', { fingerprint: 'unknown' }),
    /runtime/,
  );
});

test('unknown optional identities are omitted rather than creating impossible applicability', () => {
  assert.deepEqual(
    analyzerRequirements('recording', 'node', {
      fingerprint: 'known',
      executable_sha256: 'unknown',
      runtime_version: '',
    }),
    { runtime: 'node', source_fingerprint: 'known' },
  );
});
