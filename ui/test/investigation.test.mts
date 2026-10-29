import test from 'node:test';
import assert from 'node:assert/strict';
import { Investigations, sourceReturnDepth } from '../src/investigation.js';

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
    startLine: 5001,
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

test('timeline ranges and evidence identities restore only within the selected observation snapshot', () => {
  const storage = session();
  const first = new Investigations(storage);
  first.observeResult('run', 'a');
  first.forRun('run').timeline = {
    kind: 'span',
    pid: 7,
    alignment: 'aligned',
    window: { start_us: 100, end_us: 200 },
    cursor: 'opaque',
    selectedId: 'measured-id',
  };
  first.persist('run');
  const reload = new Investigations(storage);
  assert.equal(reload.observeResult('run', 'a'), undefined);
  assert.deepEqual(reload.forRun('run').timeline, first.forRun('run').timeline);
  reload.observeResult('run', 'b');
  assert.deepEqual(reload.forRun('run').timeline, {
    kind: 'span',
    pid: 7,
    alignment: 'aligned',
    window: undefined,
    cursor: undefined,
    selectedId: undefined,
  });
});

test('evidence restores observation IDs against result identity and immutable artifact chunks by artifact identity', () => {
  const storage = session();
  const first = new Investigations(storage);
  first.observeResult('run', 'a');
  first.forRun('run').evidence = {
    cursor: 'page',
    selection: { kind: 'observation', id: 'evidence-a' },
  };
  first.persist('run');
  const reload = new Investigations(storage);
  reload.observeResult('run', 'a');
  assert.deepEqual(reload.forRun('run').evidence, first.forRun('run').evidence);
  reload.observeResult('run', 'b');
  assert.equal(reload.forRun('run').evidence.cursor, undefined);
  assert.equal(reload.forRun('run').evidence.selection, undefined);
  reload.forRun('run').evidence.selection = {
    kind: 'artifact',
    id: 'immutable',
    name: 'input',
    offset: 5000,
  };
  reload.persist('run');
  const after = new Investigations(storage);
  after.observeResult('run', 'c');
  assert.deepEqual(after.forRun('run').evidence.selection, {
    kind: 'artifact',
    id: 'immutable',
    name: 'input',
    offset: 5000,
  });
});

test('private source-history depth preserves legacy entries without accepting invalid navigation distances', () => {
  assert.equal(sourceReturnDepth(true), 1);
  assert.equal(sourceReturnDepth(3), 3);
  for (const invalid of [
    false,
    undefined,
    -1,
    1.5,
    Number.MAX_SAFE_INTEGER + 1,
    '2',
    {},
  ])
    assert.equal(sourceReturnDepth(invalid), 0);
});

test('comparison pair preferences survive reload and observation replacement without caching comparison results', () => {
  const storage = session();
  const first = new Investigations(storage);
  first.observeResult('candidate', 'a');
  first.forRun('candidate').comparison = {
    before: 'older-than-first-page',
    after: 'candidate',
  };
  first.persist('candidate');
  const reload = new Investigations(storage);
  reload.observeResult('candidate', 'b');
  assert.deepEqual(reload.forRun('candidate').comparison, {
    before: 'older-than-first-page',
    after: 'candidate',
  });
  assert.deepEqual(reload.forRun('other').comparison, {});
});

test('resource and artifact page positions restore without cached rows and invalidate with selected observations', () => {
  const storage = session();
  const first = new Investigations(storage);
  first.observeResult('run', 'a');
  first.forRun('run').resources = { cursor: '2000', previous: [null] };
  first.forRun('run').evidence.artifacts = { cursor: '50', previous: [null] };
  first.persist('run');
  const reload = new Investigations(storage);
  reload.observeResult('run', 'a');
  assert.deepEqual(reload.forRun('run').resources, {
    cursor: '2000',
    previous: [null],
  });
  assert.deepEqual(reload.forRun('run').evidence.artifacts, {
    cursor: '50',
    previous: [null],
  });
  reload.observeResult('run', 'b');
  assert.deepEqual(reload.forRun('run').resources, { previous: [] });
  assert.equal(reload.forRun('run').evidence.artifacts, undefined);
});

test('confirmed removal forgets only that recording private location and preferences', () => {
  const storage = session();
  const state = new Investigations(storage);
  state.forRun('removed').source = {
    view: 'profile',
    path: '/retained/path',
    line: 7,
  };
  state.forRun('kept').profile.query = 'keep this';
  state.persist('removed');
  state.persist('kept');
  state.forget('removed');
  assert.equal(state.forRun('removed').source, undefined);
  const reload = new Investigations(storage);
  assert.equal(reload.forRun('removed').source, undefined);
  assert.equal(reload.forRun('kept').profile.query, 'keep this');
});
