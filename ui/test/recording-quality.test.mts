import test from 'node:test';
import assert from 'node:assert/strict';
import { captureQuality, targetCheck } from '../src/recordings/presentation.js';

const run = {
  runtime: 'native',
  status: 'completed',
  exit_code: 0,
  source: {},
  correctness: null as unknown,
};

test('capture readiness does not invent completion from target success or missing metadata', () => {
  assert.equal(captureQuality(run).label, 'Capture status unknown');
  assert.equal(
    captureQuality({ ...run, source: { collection_status: 'completed' } })
      .label,
    'Capture ready',
  );
  assert.equal(
    captureQuality({ ...run, runtime: 'none' }).label,
    'Collection off',
  );
  assert.equal(
    captureQuality({ ...run, runtime: 'imported' }).label,
    'Imported profile',
  );
});

test('incomplete retained data outranks a completed collection label without confusing source coverage', () => {
  assert.equal(
    captureQuality({
      ...run,
      source: {
        collection_status: 'completed',
        collection_completion: { state: 'incomplete' },
      },
    }).label,
    'Partial capture',
  );
  assert.equal(
    captureQuality({
      ...run,
      source: {
        collection_status: 'completed',
        raw_retention: { complete: false },
      },
    }).label,
    'Partial capture',
  );
  assert.equal(
    captureQuality({
      ...run,
      source: {
        collection_status: 'completed',
        source_snapshot_complete: false,
      },
    }).label,
    'Capture ready',
  );
});

test('target failure and successful collection remain independent', () => {
  const failed = {
    ...run,
    status: 'failed',
    exit_code: 2,
    source: {
      collection_status: 'completed',
      target_outcome: { state: 'exited', exit_code: 2 },
    },
    correctness: {
      passed: false,
      actual_exit_code: 2,
      validation: 'exit_code_only',
    },
  };
  assert.equal(captureQuality(failed).label, 'Capture ready');
  assert.equal(targetCheck(failed).label, 'Exited 2');
  assert.equal(targetCheck(failed).tone, 'bad');
  assert.match(targetCheck(failed).detail, /checks failed/);
});

test('a collector exit cannot be substituted for an unknown target outcome', () => {
  const unknown = {
    ...run,
    status: 'failed',
    exit_code: 73,
    source: {
      target_outcome: { state: 'unknown', exit_code: null },
      collector_outcome: { exit_code: 73 },
    },
  };
  assert.equal(targetCheck(unknown).label, 'Target outcome unknown');
  assert.equal(targetCheck(unknown).tone, 'warn');
  assert.equal(
    targetCheck({ ...run, runtime: 'imported' }).label,
    'Target not recorded',
  );
});

test('exit-only success does not claim output validation; expected nonzero exits can pass', () => {
  assert.match(
    targetCheck({
      ...run,
      correctness: {
        passed: true,
        actual_exit_code: 0,
        validation: 'exit_code_only',
      },
    }).detail,
    /output not checked/,
  );
  const checked = targetCheck({
    ...run,
    exit_code: 2,
    correctness: {
      passed: true,
      actual_exit_code: 2,
      validation: 'exit_code_and_stdout',
    },
  });
  assert.equal(checked.label, 'Exited 2');
  assert.equal(checked.tone, 'good');
  assert.match(checked.detail, /Exit code and output matched/);
});

test('running, finalizing, cancellation and timeout remain distinguishable', () => {
  assert.equal(
    captureQuality({
      ...run,
      status: 'running',
      source: { collection_status: 'finalizing' },
    }).label,
    'Finalizing capture',
  );
  assert.equal(
    targetCheck({ ...run, status: 'running', exit_code: null }).label,
    'Running',
  );
  assert.equal(
    targetCheck({ ...run, correctness: { timed_out: true } }).label,
    'Timed out',
  );
  assert.equal(
    targetCheck({ ...run, correctness: { canceled: true } }).label,
    'Canceled',
  );
});
