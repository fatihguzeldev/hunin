import test from 'node:test';
import assert from 'node:assert/strict';
import { Investigations } from '../src/investigation.js';

function session() {
  const values = new Map<string, string>();
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
}

test('source and profile scope survive session reload only against the same known result identity', () => {
  const storage = session();
  const first = new Investigations(storage);
  first.observeResult('run', 'selected-a');
  const state = first.forRun('run');
  state.profile = {
    unit: 'samples',
    path: ['frame'],
    breadcrumbs: ['work'],
    mode: 'flame',
    query: 'hot',
    expanded: ['frame'],
    focus: { path: ['frame', 'child'], names: ['work', 'hot'] },
  };
  state.source = {
    view: 'profile',
    path: '/project/work.rs',
    line: 4,
    codeId: 'measured',
  };
  first.persist('run');
  const reloaded = new Investigations(storage);
  assert.equal(reloaded.observeResult('run', 'selected-a'), undefined);
  assert.deepEqual(
    JSON.parse(JSON.stringify(reloaded.forRun('run'))),
    JSON.parse(JSON.stringify(state)),
  );
  assert.equal(reloaded.observeResult('run', 'selected-b'), 'changed');
  assert.deepEqual(reloaded.forRun('run').profile.path, []);
  assert.equal(reloaded.forRun('run').source, undefined);
  assert.equal(reloaded.forRun('run').profile.query, 'hot');
  assert.equal(reloaded.forRun('run').profile.mode, 'flame');
  assert.equal(reloaded.forRun('run').profile.unit, 'samples');
});

test('history restores remain run-scoped and old services do not silently confirm saved frame identities', () => {
  const store = new Investigations();
  store.observeResult('run', 'selected-a');
  store.forRun('run').profile.path = ['old-frame'];
  const saved = JSON.parse(JSON.stringify(store.capture('run')));
  store.observeResult('run', 'selected-b');
  store.restore('run', saved);
  assert.equal(store.observeResult('run', 'selected-b'), 'changed');
  assert.deepEqual(store.forRun('other').profile.path, []);
  store.forRun('run').profile.path = ['unconfirmed'];
  store.persist('run');
  store.restore('run', store.capture('run'));
  assert.equal(store.observeResult('run', undefined), 'changed');
  assert.deepEqual(store.forRun('run').profile.path, []);
});

test('malformed or unavailable private session storage falls back to in-memory investigation state', () => {
  const failed = new Investigations({
    getItem: () => {
      throw new Error('unavailable');
    },
    setItem: () => {
      throw new Error('quota');
    },
  });
  failed.forRun('run').profile.query = 'kept on this page';
  failed.persist('run');
  assert.equal(failed.persistent, false);
  assert.equal(failed.forRun('run').profile.query, 'kept on this page');
  const malformed = new Investigations({
    getItem: () => '{not json',
    setItem: () => {},
  });
  assert.deepEqual(malformed.forRun('run').profile.path, []);
  assert.equal(malformed.persistent, true);
});
