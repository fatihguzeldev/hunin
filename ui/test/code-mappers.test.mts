import test from 'node:test';
import assert from 'node:assert/strict';
import { mapperAvailability } from '../src/code/mapper-state.js';
import type { CodePlan } from '../src/operations.generated.js';

function plan(steps: CodePlan['steps'], truncated = false): CodePlan {
  return {
    result_identity: 'selected-results',
    anchor: null,
    goal: 'semantic_origin',
    steps,
    truncated,
  };
}

test('only the server plan authorizes the selected mapper for the active recording', () => {
  const available = plan([
    {
      id: 'mapper:mapper-one',
      status: 'available',
      reason: 'Required input identities apply.',
      operation: 'analyses.run',
      params: { run_id: 'run-one', analysis_id: 'mapper-one' },
    },
  ]);
  assert.deepEqual(mapperAvailability(available, 'mapper-one', 'run-one'), {
    status: 'available',
    reason: 'Required input identities apply.',
    params: { run_id: 'run-one', analysis_id: 'mapper-one' },
  });
  assert.equal(
    mapperAvailability(available, 'mapper-one', 'other-run').status,
    'unassessed',
  );
  assert.equal(
    mapperAvailability(available, 'other-mapper', 'run-one').status,
    'unassessed',
  );
});

test('blocked and unassessed mapper states cannot become runnable through registration alone', () => {
  const blocked = plan([
    {
      id: 'mapper:old-build',
      status: 'blocked',
      reason: 'Required engine digest is absent.',
      operation: null,
      params: {},
    },
  ]);
  assert.deepEqual(mapperAvailability(blocked, 'old-build', 'run-one'), {
    status: 'blocked',
    reason: 'Required engine digest is absent.',
  });
  assert.equal(
    mapperAvailability(plan([], true), 'later-page', 'run-one').status,
    'unassessed',
  );
  assert.equal(
    mapperAvailability(undefined, 'registered', 'run-one').status,
    'unassessed',
  );
});
