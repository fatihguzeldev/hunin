import test from 'node:test';
import assert from 'node:assert/strict';
import {
  analysisMetadata,
  analysisAttemptMetadata,
} from '../src/analysis/metadata.js';
import { scenarioMetadata } from '../src/metadata.js';
import { operation } from '../src/api.js';

test('legacy document results retain arbitrary rows while checking their page envelope', async () => {
  const original = globalThis.fetch;
  let result: unknown = {
    analyses: [null, 'legacy', { id: 'minimal', name: 'retained' }],
    next_cursor: null,
  };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(await operation('analyses.list', {}), result);
    result = { analyses: {}, next_cursor: null };
    await assert.rejects(
      operation('analyses.list', {}),
      /invalid analyses.list result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('legacy analyzer metadata stays inspectable without inventing package or attempt fields', () => {
  const raw = {
    id: 'legacy',
    name: 'retained',
    created_at_us: 1,
    extension: { private: true },
  };
  const packageView = analysisMetadata(raw);
  assert.equal(packageView.id, 'legacy');
  assert.equal(packageView.command, undefined);
  assert.equal(packageView.version, undefined);
  assert.equal(packageView.raw, raw);
  const attempt = analysisAttemptMetadata({ ...raw, selected: false });
  assert.equal(attempt.status, 'unknown');
  assert.equal(attempt.selected, false);
  assert.equal(attempt.created_at, '1970-01-01T00:00:00.000Z');
  assert.equal(attempt.input_fingerprint, undefined);
  assert.deepEqual(attempt.diagnostics, []);
});

test('scalar, malformed and out-of-range documents remain unknown rather than actionable recipes', () => {
  for (const raw of [
    null,
    'legacy',
    7,
    [],
    { id: 7, command: ['node', 7], created_at_us: Number.MAX_VALUE },
  ]) {
    assert.equal(analysisMetadata(raw).id, undefined);
    assert.equal(analysisMetadata(raw).command, undefined);
    assert.equal(analysisAttemptMetadata(raw).created_at, undefined);
    assert.equal(scenarioMetadata(raw).command, undefined);
    assert.equal(scenarioMetadata(raw).raw, raw);
  }
  const raw = { id: 'scenario', name: 'minimal', command: ['workload'] };
  const scenario = scenarioMetadata(raw);
  assert.equal(scenario.id, raw.id);
  assert.deepEqual(scenario.command, raw.command);
  assert.equal(scenario.cwd, undefined);
  assert.equal(scenario.timeout_seconds, undefined);
  assert.equal(scenario.expected_exit_code, undefined);
  assert.equal(scenario.raw, raw);
});
