import test from 'node:test';
import assert from 'node:assert/strict';
import { linkedRequest } from '../src/requests.js';

test('view disposal and replacement abort read interest independently', () => {
  const view = new AbortController();
  const first = new AbortController();
  const old = linkedRequest(view.signal, first.signal);
  first.abort();
  assert.equal(old.signal.aborted, true);
  assert.equal(view.signal.aborted, false);
  old.finish();
  const second = linkedRequest(view.signal, new AbortController().signal);
  assert.equal(second.signal.aborted, false);
  view.abort();
  assert.equal(second.signal.aborted, true);
  second.finish();
});

test('already disposed views cannot start live reads and completed links release their lifetime', () => {
  const view = new AbortController();
  const finished = linkedRequest(view.signal, new AbortController().signal);
  finished.finish();
  view.abort();
  assert.equal(finished.signal.aborted, false);
  const stale = linkedRequest(view.signal, new AbortController().signal);
  assert.equal(stale.signal.aborted, true);
  stale.finish();
});
