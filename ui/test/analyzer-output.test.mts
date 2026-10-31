import test from 'node:test';
import assert from 'node:assert/strict';
import { outputArtifact, outputSelections } from '../src/analysis/output.js';

test('only the retained stdout artifact identifies an attempt output', () => {
  assert.equal(
    outputArtifact({ raw_artifacts: [{ name: 'stderr.txt', id: 'log' }] }),
    undefined,
  );
  assert.deepEqual(
    outputArtifact({
      raw_artifacts: [
        { name: 'stdout.jsonl', format: 'analysis-output', id: 'output' },
      ],
    }),
    { id: 'output', name: 'stdout.jsonl' },
  );
  assert.equal(
    outputArtifact({
      raw_artifacts: [
        { name: 'stdout.jsonl', format: 'analysis-input', id: 'input' },
      ],
    }),
    undefined,
  );
});

test('output selection uses complete derived records for this recording, without trusting partial JSONL', () => {
  const event = JSON.stringify({
    id: 'derived-event',
    run_id: 'run',
    kind: 'event',
    basis: 'derived',
    name: 'Derived event',
  });
  const sample = JSON.stringify({
    id: 'derived-sample',
    run_id: 'run',
    kind: 'sample',
    basis: 'derived',
  });
  assert.deepEqual(
    outputSelections(event + '\n' + sample.slice(0, 20), 'run', false, false),
    [{ id: 'derived-event', label: 'Derived event' }],
  );
  assert.deepEqual(
    outputSelections('partial remainder\n' + sample, 'run', true, true),
    [{ id: 'derived-sample', label: 'sample' }],
  );
  assert.deepEqual(outputSelections(event, 'run', false, true), [
    { id: 'derived-event', label: 'Derived event' },
  ]);
});

test('invalid, foreign and non-derived rows cannot become evidence selections', () => {
  const rows = [
    'not JSON',
    JSON.stringify({
      id: 'foreign',
      run_id: 'other',
      kind: 'event',
      basis: 'derived',
    }),
    JSON.stringify({
      id: 'sampled',
      run_id: 'run',
      kind: 'sample',
      basis: 'sampled',
    }),
    JSON.stringify({
      id: 'derived',
      run_id: 'run',
      kind: 'event',
      basis: 'derived',
    }),
    JSON.stringify({
      id: 'derived',
      run_id: 'run',
      kind: 'event',
      basis: 'derived',
    }),
  ];
  assert.deepEqual(outputSelections(rows.join('\n'), 'run', false, true), [
    { id: 'derived', label: 'event' },
  ]);
});
