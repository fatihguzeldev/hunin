import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from '../src/api.js';
import { sourceUnavailable } from '../src/source/errors.js';

function failure(operation: string, code: string, message = 'source failure') {
  return new ApiError('operation_failed', message, {
    operation_error: { operation, code, message },
  });
}

test('typed source confinement is an unavailable state without implementation diagnostics', () => {
  const state = sourceUnavailable(
    failure(
      'source.read',
      'source_outside_project',
      'source path is outside the measured project: prefix not found',
    ),
  );
  assert.ok(state);
  assert.equal(state.title, 'Source outside measured project');
  assert.match(state.message, /outside the recording’s project/);
  assert.doesNotMatch(state.message, /prefix not found/);
});

test('untyped messages and other operations are never inferred as source confinement', () => {
  assert.equal(
    sourceUnavailable(
      failure(
        'source.read',
        'operation_failed',
        'source path is outside the measured project',
      ),
    ),
    undefined,
  );
  assert.equal(
    sourceUnavailable(failure('runs.get', 'source_outside_project')),
    undefined,
  );
  assert.equal(
    sourceUnavailable(new Error('source path is outside the measured project')),
    undefined,
  );
  assert.equal(
    sourceUnavailable(
      new ApiError('source_outside_project', 'outside project'),
    ),
    undefined,
  );
});
