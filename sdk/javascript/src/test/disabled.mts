import assert from 'node:assert/strict';
import * as h from '../index.mjs';

assert.equal(await h.withSpan('work', async () => 42), 42);
try {
  h.withSpan('failure', () => {
    throw Error('expected');
  });
} catch (error) {
  assert.ok(error instanceof Error);
  assert.equal(error.message, 'expected');
}
h.counter('jobs', 1);
h.flush();
