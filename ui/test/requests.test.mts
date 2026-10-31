import test from 'node:test';
import assert from 'node:assert/strict';
import { getEventListeners } from 'node:events';
import { linkedLifetime, linkedRequest } from '../src/requests.js';

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

test('interactive foreground reads remain owned after the initial request finishes', async () => {
  for (const dispose of ['render', 'tab'] as const) {
    const render = new AbortController();
    const tab = new AbortController();
    const widget = linkedLifetime(render.signal, tab.signal);
    const initial = linkedRequest(widget, new AbortController().signal);
    await Promise.resolve();
    initial.finish();
    const foreground = linkedRequest(widget, new AbortController().signal);
    assert.equal(foreground.signal.aborted, false);
    (dispose === 'render' ? render : tab).abort();
    assert.equal(widget.aborted, true);
    assert.equal(foreground.signal.aborted, true);
    foreground.finish();
  }
});

test('anchor replacement aborts old foreground reads while a fresh anchor stays owned by the render', () => {
  const render = new AbortController();
  const firstAnchor = new AbortController();
  const first = linkedLifetime(render.signal, firstAnchor.signal);
  const proof = linkedRequest(first, new AbortController().signal);
  firstAnchor.abort();
  assert.equal(proof.signal.aborted, true);
  const second = linkedLifetime(render.signal, new AbortController().signal);
  const plan = linkedRequest(second, new AbortController().signal);
  assert.equal(plan.signal.aborted, false);
  assert.equal(second.aborted, false);
  render.abort();
  assert.equal(plan.signal.aborted, true);
  proof.finish();
  plan.finish();
});

test('replacing interactive scopes releases subscriptions to their still-live parent', () => {
  const parent = new AbortController();
  for (let index = 0; index < 2; index++) {
    const local = new AbortController();
    const scope = linkedLifetime(parent.signal, local.signal);
    assert.equal(getEventListeners(parent.signal, 'abort').length, 1);
    local.abort();
    assert.equal(scope.aborted, true);
    assert.equal(getEventListeners(parent.signal, 'abort').length, 0);
    assert.equal(getEventListeners(local.signal, 'abort').length, 0);
  }
  const disposed = new AbortController();
  disposed.abort();
  assert.equal(linkedLifetime(parent.signal, disposed.signal).aborted, true);
  assert.equal(getEventListeners(parent.signal, 'abort').length, 0);
});
