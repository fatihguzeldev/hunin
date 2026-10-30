import test from 'node:test';
import assert from 'node:assert/strict';
import { Investigations } from '../src/investigation/state.js';
import { InvestigationHistory } from '../src/investigation/history.js';

function browser() {
  const location = { href: 'http://localhost/?retained=1#position' };
  let state: unknown;
  const writes: ('push' | 'replace')[] = [];
  const traversals: number[] = [];
  const write = (
    mode: 'push' | 'replace',
    value: unknown,
    url?: string | URL | null,
  ) => {
    state = structuredClone(value);
    if (url) location.href = String(url);
    writes.push(mode);
  };
  return {
    location,
    writes,
    traversals,
    history: {
      length: 4,
      go: (delta?: number) => {
        traversals.push(delta ?? 0);
      },
      get state() {
        return state;
      },
      pushState: (value: unknown, _title: string, url?: string | URL | null) =>
        write('push', value, url),
      replaceState: (
        value: unknown,
        _title: string,
        url?: string | URL | null,
      ) => write('replace', value, url),
    },
  };
}

test('browser entries retain per-recording investigation snapshots and source return depth across window paging', () => {
  const binding = browser();
  const investigations = new Investigations();
  const navigation = new InvestigationHistory(investigations, binding);
  const state = investigations.forRun('recording');
  state.profile.query = 'hotspot';
  navigation.write({ runId: 'recording', view: 'profile' }, 'push');
  state.source = {
    view: 'profile',
    path: 'measured.ts',
    line: 20,
    codeId: 'code',
  };
  navigation.write({ runId: 'recording', view: 'profile' }, 'push', 1);
  state.source.startLine = 5001;
  navigation.write({ runId: 'recording', view: 'profile' }, 'push', 2);
  navigation.write({ runId: 'recording', view: 'profile' }, 'replace');
  assert.equal(navigation.state()?.sourceReturn, 2);
  assert.equal(navigation.returnToView('other-recording'), false);
  assert.equal(navigation.returnToView('recording'), true);
  assert.deepEqual(binding.traversals, [-2]);
  state.profile.query = 'later unpersisted edit';

  const restored = new Investigations();
  const history = new InvestigationHistory(restored, binding);
  history.restore('other-recording');
  assert.equal(restored.forRun('other-recording').source, undefined);
  history.restore('recording');
  assert.equal(restored.forRun('recording').profile.query, 'hotspot');
  assert.deepEqual(restored.forRun('recording').source, {
    view: 'profile',
    path: 'measured.ts',
    line: 20,
    codeId: 'code',
    startLine: 5001,
  });
  assert.deepEqual(binding.writes, ['push', 'push', 'push', 'replace']);
});

test('leaving source clears its return distance while keeping unrelated URL state and malformed browser entries untrusted', () => {
  const binding = browser();
  const investigations = new Investigations();
  const navigation = new InvestigationHistory(investigations, binding);
  investigations.forRun('recording').source = {
    view: 'timeline',
    path: 'source.rs',
  };
  navigation.write({ runId: 'recording', view: 'timeline' }, 'push', 1);
  investigations.forRun('recording').source = undefined;
  navigation.write({ runId: 'recording', view: 'timeline' });
  assert.equal(navigation.state()?.sourceReturn, 0);
  assert.equal(navigation.returnToView('recording'), false);
  navigation.write({ runId: '', view: 'capabilities' });
  const url = new URL(binding.location.href);
  assert.equal(url.searchParams.has('run'), false);
  assert.equal(url.searchParams.get('view'), 'capabilities');
  assert.equal(url.searchParams.get('retained'), '1');
  assert.equal(url.hash, '#position');
  for (const state of [
    null,
    { hunin: false },
    { hunin: { runId: 1, saved: {} } },
    { hunin: { runId: 'recording' } },
  ]) {
    binding.history.replaceState(state, '');
    assert.equal(navigation.state(), undefined);
  }
});

test('history from another project cannot restore or traverse a same-ID recording', () => {
  const binding = browser();
  const one = new Investigations();
  one.forRun('same').profile.query = 'project one';
  one.forRun('same').source = { view: 'profile', path: 'one.ts' };
  new InvestigationHistory(one, binding, 'project:one').write(
    { runId: 'same', view: 'profile' },
    'push',
    1,
  );
  const two = new Investigations();
  const otherHistory = new InvestigationHistory(two, binding, 'project:two');
  otherHistory.restore('same');
  assert.equal(two.forRun('same').profile.query, '');
  assert.equal(otherHistory.returnToView('same'), false);
  const restored = new Investigations();
  new InvestigationHistory(restored, binding, 'project:one').restore('same');
  assert.equal(restored.forRun('same').profile.query, 'project one');
});
