import test from 'node:test';
import assert from 'node:assert/strict';
import { ApiError } from '../src/api.js';
import {
  inspectRemovalReceipt,
  removalFailureMessage,
  removalReceiptNotice,
} from '../src/removal-feedback.js';

const pending = {
  version: 1,
  run_id: 'original',
  completed: false,
  last_error: 'managed path identity changed; preserved original workspace',
};

test('a matching v1 pending receipt reports retained cleanup limits and the same request retry', () => {
  const notice = removalReceiptNotice('original', pending);
  assert.equal(notice.kind, 'pending');
  assert.match(notice.message, /Retry the same removal request/);
  assert.match(notice.lastError ?? '', /identity changed/);
  assert.equal('removed' in notice, false);
});

test('a completed receipt is history rather than confirmation of a failed current request', () => {
  const notice = removalReceiptNotice('original', {
    ...pending,
    completed: true,
    last_error: null,
  });
  assert.equal(notice.kind, 'completed');
  assert.match(notice.message, /current removal request remains unconfirmed/);
  assert.equal(notice.lastError, undefined);
  assert.equal('removed' in notice, false);
});

test('future, incomplete, mismatched and malformed metadata never acquire known cleanup authority', () => {
  for (const receipt of [
    { ...pending, version: 2 },
    { ...pending, run_id: 'reused-other' },
    { ...pending, completed: 'false' },
    { version: 1, run_id: 'original', completed: false },
    { ...pending, last_error: 7 },
    [],
    'retained scalar',
  ]) {
    const notice = removalReceiptNotice('original', receipt);
    assert.equal(notice.kind, 'unknown');
    assert.equal('removed' in notice, false);
    assert.equal(notice.lastError, undefined);
  }
  assert.equal(removalReceiptNotice('original', null).kind, 'absent');
});

test('read-only lookup failure leaves the original failure message intact, including legacy unavailable operations', async () => {
  const originalError = new Error('original removal failed');
  const originalMessage = removalFailureMessage(originalError);
  let reads = 0;
  const notice = await inspectRemovalReceipt('original', async () => {
    reads++;
    throw new ApiError(
      'unknown_operation',
      'legacy service has no runs.removal',
    );
  });
  assert.equal(reads, 1);
  assert.equal(notice.kind, 'unavailable');
  assert.match(notice.message, /legacy service has no runs.removal/);
  assert.match(originalMessage, /original removal failed/);
  assert.equal(removalFailureMessage(originalError), originalMessage);
});

test('a successful receipt read remains a notice and never retries or confirms a mutation', async () => {
  let reads = 0;
  const notice = await inspectRemovalReceipt('original', async () => {
    reads++;
    return { ...pending, completed: true };
  });
  assert.equal(reads, 1);
  assert.equal(notice.kind, 'completed');
  assert.match(notice.message, /current removal request remains unconfirmed/);
  assert.equal('removed' in notice, false);
});
