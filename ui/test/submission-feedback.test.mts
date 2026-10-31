import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from '../src/api.js';
import { rejectedBeforeAdmission } from '../src/recordings/submission.js';

function failure(code: string, operation = 'runs.start') {
  return new ApiError('operation_failed', 'Request rejected', {
    operation_error: { code, operation, message: 'Request rejected' },
  });
}

test('only checked pre-admission rejections prove a recording was not submitted', () => {
  for (const code of [
    'invalid_parameters',
    'unknown_operation',
    'capacity_exceeded',
  ])
    assert.equal(
      rejectedBeforeAdmission(failure(code), 'runs.start'),
      true,
      code,
    );
  for (const error of [
    new TypeError('Failed to fetch'),
    failure('operation_failed'),
    failure('conflict'),
    new ApiError('forbidden', 'Blocked'),
  ])
    assert.equal(rejectedBeforeAdmission(error, 'runs.start'), false);
});

test('typed rejection must belong to the requested mutation', () => {
  const imported = failure('invalid_parameters', 'recordings.import');
  assert.equal(rejectedBeforeAdmission(imported, 'runs.start'), false);
  assert.equal(rejectedBeforeAdmission(imported, 'recordings.import'), true);
  assert.equal(
    rejectedBeforeAdmission(new Error('invalid_parameters'), 'runs.start'),
    false,
  );
});
