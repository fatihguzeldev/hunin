import test from 'node:test';
import assert from 'node:assert/strict';
import {
  projectFromUrl,
  projectUrl,
  projectStorage,
} from '../src/projects/context.js';
import { Investigations } from '../src/investigation/state.js';
import { AnalysisWatch } from '../src/analysis/watch.js';

test('project navigation retains the server origin and clears recording-specific URLs', () => {
  const url = projectUrl(
    'http://localhost:7878/?project=old&run=same&view=profile&retained=1#source',
    'new / project',
  );
  assert.equal(url.origin, 'http://localhost:7878');
  assert.equal(projectFromUrl(url.href), 'new / project');
  assert.equal(url.searchParams.get('retained'), '1');
  assert.equal(url.searchParams.has('run'), false);
  assert.equal(url.searchParams.has('view'), false);
  assert.equal(url.hash, '');
  const picker = projectUrl(url.href);
  assert.equal(picker.searchParams.has('project'), true);
  assert.equal(projectFromUrl(picker.href), undefined);
});

test('identical recording and attempt IDs remain isolated between project and direct-store contexts', () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
    removeItem: (key: string) => {
      values.delete(key);
    },
  };
  const one = projectStorage(storage, 'project:one');
  const two = projectStorage(storage, 'project:two');
  const direct = projectStorage(storage, 'direct:/another/store');
  const investigation = new Investigations(one);
  investigation.forRun('same').profile.query = 'only one';
  investigation.persist('same');
  new AnalysisWatch(one).track('same', 'attempt');
  for (const scope of [two, direct]) {
    assert.equal(new Investigations(scope).forRun('same').profile.query, '');
    assert.deepEqual(new AnalysisWatch(scope).forRun('same'), []);
  }
  assert.equal(
    new Investigations(one).forRun('same').profile.query,
    'only one',
  );
  assert.equal(new AnalysisWatch(one).forRun('same')[0]?.id, 'attempt');
  new Investigations(two).forget('same');
  assert.equal(
    new Investigations(one).forRun('same').profile.query,
    'only one',
  );
});
