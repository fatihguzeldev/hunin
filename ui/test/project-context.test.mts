import test from 'node:test';
import assert from 'node:assert/strict';
import {
  projectFromUrl,
  projectUrl,
  recordingUrl,
  projectStorage,
  projectStorageScope,
  resolveProjectSelection,
} from '../src/projects/context.js';
import type { ProjectCatalog } from '../src/operations.generated.js';
import { Investigations } from '../src/investigation/state.js';
import { AnalysisWatch } from '../src/analysis/watch.js';

test('original/count recording links retain each tab project across deep links and reloads', () => {
  for (const project of ['one', 'two / encoded']) {
    const document = new URL('http://localhost:7878/?retained=1#source');
    document.searchParams.set('project', project);
    document.searchParams.set('run', 'count-child');
    const parent = recordingUrl(document.href, 'sample-parent', 'calls');
    const child = recordingUrl(parent.href, 'count-child', 'calls');
    assert.equal(parent.origin, document.origin);
    assert.equal(parent.searchParams.get('project'), project);
    assert.equal(parent.searchParams.get('run'), 'sample-parent');
    assert.equal(parent.searchParams.get('view'), 'calls');
    assert.equal(parent.searchParams.get('retained'), '1');
    assert.equal(parent.hash, '');
    assert.equal(child.searchParams.get('project'), project);
    assert.equal(child.searchParams.get('run'), 'count-child');
    assert.equal(
      resolveProjectSelection(
        {
          projects: [
            {
              id: project,
              name: project,
              store_path: '/store',
              workspace_roots: [],
              legacy: false,
            },
          ],
          selected_project_id: null,
          direct_store: false,
          storage_scope: null,
        },
        parent.href,
      ).kind,
      'project',
    );
  }
  const direct = recordingUrl('http://localhost/?run=parent', 'child', 'calls');
  assert.equal(direct.searchParams.has('project'), false);
});

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

test('legacy store preferences bind the actual store while project preferences follow stable identity', () => {
  const project = {
    id: 'one',
    name: 'One',
    store_path: '/one',
    workspace_roots: [],
    legacy: false,
  };
  assert.equal(
    projectStorageScope(project),
    projectStorageScope({ ...project, store_path: '/moved' }),
  );
  const legacy = { ...project, id: 'legacy', legacy: true };
  assert.notEqual(
    projectStorageScope(legacy),
    projectStorageScope({ ...legacy, store_path: '/other-registry' }),
  );
  assert.notEqual(projectStorageScope(legacy), `direct:${legacy.store_path}`);
  assert.notEqual(
    projectStorageScope(legacy),
    projectStorageScope({ ...legacy, legacy: false }),
  );
});

test('startup project selection is explicit, canonical and never falls back from an unknown project', () => {
  const catalog: ProjectCatalog = {
    projects: [
      {
        id: 'one',
        name: 'One',
        store_path: '/one',
        workspace_roots: [],
        legacy: false,
      },
    ],
    selected_project_id: 'one',
    direct_store: false,
    storage_scope: null,
  };
  const startup = resolveProjectSelection(
    catalog,
    'http://localhost/?run=existing',
  );
  assert.equal(startup.kind, 'redirect');
  if (startup.kind === 'redirect') {
    assert.equal(startup.url.searchParams.get('project'), 'one');
    assert.equal(startup.url.searchParams.get('run'), 'existing');
  }
  assert.equal(
    resolveProjectSelection(catalog, 'http://localhost/?project=').kind,
    'picker',
  );
  assert.equal(
    resolveProjectSelection(catalog, 'http://localhost/?project=one').kind,
    'project',
  );
  const unknown = resolveProjectSelection(
    catalog,
    'http://localhost/?project=missing&run=existing',
  );
  assert.equal(unknown.kind, 'picker');
  assert.ok(unknown.kind === 'picker' && unknown.problem);
  const direct = {
    ...catalog,
    projects: [],
    selected_project_id: null,
    direct_store: true,
    storage_scope: '/explicit/store',
  };
  assert.deepEqual(resolveProjectSelection(direct, 'http://localhost/'), {
    kind: 'direct',
    storageScope: '/explicit/store',
  });
  assert.equal(
    resolveProjectSelection(direct, 'http://localhost/?project=one').kind,
    'picker',
  );
  assert.throws(
    () =>
      resolveProjectSelection(
        { ...direct, storage_scope: null },
        'http://localhost/',
      ),
    /identity/,
  );
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
