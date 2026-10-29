import assert from 'node:assert/strict';
import * as h from '../index.mjs';

await h.withContext('request', async () => {
  for (const original of [
    Object.assign(Error('target-error'), {
      toString() {
        throw Error('formatter-error');
      },
    }),
    {
      toString() {
        throw 0;
      },
    },
  ]) {
    assert.throws(
      () =>
        h.withSpan('sync-error', () => {
          throw original;
        }),
      (actual) => actual === original,
    );
    await assert.rejects(
      h.withSpan('async-error', async () => {
        throw original;
      }),
      (actual) => actual === original,
    );
  }
});
h.flush();
assert.equal(h.lastError(), undefined);
console.log('target-ok');
