import test from 'node:test';
import assert from 'node:assert/strict';
import { sourceIdentityMetadata } from '../src/metadata.js';
import { Investigations } from '../src/investigation/state.js';

test('comparison identity presentation keeps source, executable and build claims separate', () => {
  const identity = sourceIdentityMetadata({
    fingerprint: 'changed-source',
    executable_sha256: 'same-runtime-executable',
    runtime_version: null,
    build_source_verified: false,
  });
  assert.equal(identity.sourceFingerprint, 'changed-source');
  assert.equal(identity.executableSha256, 'same-runtime-executable');
  assert.equal(identity.runtimeVersion, undefined);
  assert.equal(identity.buildSourceVerified, false);
  for (const value of [null, 'legacy', false, [], { fingerprint: 7 }])
    assert.equal(sourceIdentityMetadata(value).sourceFingerprint, undefined);
});

test('comparison unit and metric preferences survive reload without persisting calculated results', () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => values.set(key, value),
  };
  const first = new Investigations(storage);
  first.forRun('candidate').comparison = {
    before: 'baseline',
    after: 'candidate',
    unit: 'bytes',
    metric: '["supervised.elapsed","milliseconds","target_process"]',
  };
  first.persist('candidate');
  const reloaded = new Investigations(storage);
  assert.deepEqual(
    reloaded.forRun('candidate').comparison,
    first.forRun('candidate').comparison,
  );
  reloaded.observeResult('candidate', 'new-selected-results');
  assert.equal(reloaded.forRun('candidate').comparison.unit, 'bytes');
  assert.equal(reloaded.forRun('other').comparison.unit, undefined);
  const saved = JSON.parse(values.values().next().value ?? '{}');
  assert.equal(saved.state.comparison.hotspots, undefined);
  assert.equal(saved.state.comparison.metrics, undefined);
});
