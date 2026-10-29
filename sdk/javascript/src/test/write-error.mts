import assert from 'node:assert/strict';
import { rmdirSync, writeFileSync } from 'node:fs';
import * as h from '../index.mjs';

if (process.argv.includes('--block-owned-directory')) {
  const directory = process.env.HUNIN_RECORD_DIR;
  assert.ok(directory);
  // Refuse nonempty/unexpected input; this mode only sabotages the empty,
  // disposable SDK directory created by the integration recording.
  rmdirSync(directory);
  writeFileSync(directory, 'blocked-sdk-output', { flag: 'wx' });
}

assert.equal(h.enabled(), true);
h.withContext('request', () => {
  const automatic = process.env.HUNIN_SDK_FAILURE_TEST === 'auto';
  const pending = h.event('buffered-before-write-error', {
    payload: automatic ? 'x'.repeat(65536) : '',
  });
  if (automatic) assert.equal(pending, undefined);
  else assert.ok(pending);
  if (process.env.HUNIN_SDK_FAILURE_TEST === 'serialize') {
    h.event('serialization-error', {
      invalid: {
        toJSON() {
          throw 0;
        },
      },
    });
  } else {
    h.flush();
  }
  const failure = h.lastError();
  assert.ok(failure);
  assert.equal(h.enabled(), false);
  assert.equal(h.captureContext(), undefined);
  assert.equal(h.context('undeliverable-context'), undefined);
  const span = h.startSpan('undeliverable-span');
  assert.equal(span.id, undefined);
  assert.equal(
    span.run(() => 37),
    37,
  );
  span.end();
  assert.equal(
    h.withContext('undeliverable-context', () => h.captureContext()),
    undefined,
  );
  assert.equal(
    h.withSpan('disabled-after-error', (value) => {
      assert.equal(value, undefined);
      return 42;
    }),
    42,
  );
  const targetError = Error('target-error');
  assert.throws(
    () =>
      h.withSpan('target-error', () => {
        throw targetError;
      }),
    (error) => error === targetError,
  );
  assert.equal(h.event('after-error'), undefined);
  assert.equal(h.counter('after-error', 1), undefined);
  assert.equal(h.relation(pending, pending), undefined);
  h.flush();
  assert.equal(h.lastError(), failure);
});
console.log('target-ok');
