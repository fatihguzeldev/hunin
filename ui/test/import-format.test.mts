import test from 'node:test';
import assert from 'node:assert/strict';
import {
  importChoices,
  canReanalyze,
  ImportCatalog,
} from '../src/recordings/formats.js';
import type { ImportFormat } from '../src/operations.generated.js';

function descriptor(overrides: Partial<ImportFormat> = {}): ImportFormat {
  return {
    format: 'future-profiler-format',
    label: 'Future profiler',
    description: 'Retained profile bytes from a future profiler.',
    aliases: ['legacy-future-profile'],
    requires: ['A retained profile and its matching sidecars'],
    requires_existing_recording: false,
    ...overrides,
  };
}

test('import choices preserve the backend canonical parser key rather than aliases or filenames', () => {
  const [choice] = importChoices([descriptor()]) ?? [];
  assert.equal(choice?.value, 'future-profiler-format');
  assert.equal(choice?.label, 'Future profiler');
  assert.equal(choice?.disabled, false);
  assert.match(choice?.description ?? '', /legacy-future-profile/);
  assert.match(choice?.description ?? '', /matching sidecars/);
});

test('new-recording availability follows ownership metadata rather than a format name table', () => {
  const choices = importChoices([
    descriptor({ requires_existing_recording: true }),
    descriptor({ format: 'hunin-jsonl', requires_existing_recording: false }),
  ]);
  assert.equal(choices?.[0]?.disabled, true);
  assert.match(choices?.[0]?.description ?? '', /matching run IDs/);
  assert.equal(choices?.[1]?.disabled, false);
});

test('an absent legacy catalog is distinct from an authoritative empty registry', () => {
  assert.equal(importChoices(undefined), undefined);
  assert.deepEqual(importChoices([]), []);
});

test('profile reanalysis follows explicit registry roles and aliases without a UI decoder list', () => {
  const formats = [
    descriptor({ input_kind: 'profile', requires_existing_recording: true }),
    descriptor({
      format: 'hunin-jsonl',
      aliases: ['jsonl'],
      input_kind: 'observations',
    }),
  ];
  assert.equal(canReanalyze('future-profiler-format', formats), true);
  assert.equal(canReanalyze('legacy-future-profile', formats), true);
  for (const format of [
    'hunin-jsonl',
    'jsonl',
    'stdout',
    'analysis-output',
    'source',
    'native-image',
    'profile-sidecar',
  ])
    assert.equal(canReanalyze(format, formats), false);
  assert.equal(
    canReanalyze('v8-cpuprofile', [
      descriptor({ format: 'v8-cpuprofile', input_kind: 'observations' }),
    ]),
    false,
  );
});

test('missing or legacy input roles do not invent profile reanalysis eligibility', () => {
  assert.equal(canReanalyze('v8-cpuprofile', undefined), false);
  assert.equal(canReanalyze('v8-cpuprofile', []), false);
  assert.equal(
    canReanalyze('v8-cpuprofile', [descriptor({ format: 'v8-cpuprofile' })]),
    false,
  );
});

function capabilities(formats: ImportFormat[]) {
  return {
    host: { os: 'fixture', arch: 'fixture' },
    schema_version: 1,
    runtimes: [],
    import_formats: formats,
    native_sampling_permission: { state: 'permission_unverified' },
    probe_limits: {
      timeout_seconds_per_command: 2,
      output_bytes_per_stream: 32768,
    },
    verification: 'Registry fixture, not a capture guarantee',
  };
}

test('a workspace caches a checked catalog while retaining per-view abort interest', async () => {
  const original = globalThis.fetch;
  const catalog = new ImportCatalog();
  const formats = [descriptor({ input_kind: 'profile' })];
  let reads = 0;
  globalThis.fetch = async () => {
    reads++;
    return Response.json({ result: capabilities(formats) });
  };
  try {
    assert.deepEqual(await catalog.read(new AbortController().signal), formats);
    assert.deepEqual(await catalog.read(new AbortController().signal), formats);
    const disposed = new AbortController();
    disposed.abort();
    await assert.rejects(catalog.read(disposed.signal), { name: 'AbortError' });
    assert.equal(reads, 1);
  } finally {
    globalThis.fetch = original;
  }
});

test('failed format discovery is retryable and a late disposed response cannot populate the workspace cache', async () => {
  const original = globalThis.fetch;
  const catalog = new ImportCatalog();
  const formats = [descriptor({ input_kind: 'profile' })];
  const disposed = new AbortController();
  let reads = 0;
  globalThis.fetch = async () => {
    reads++;
    if (reads === 1) {
      disposed.abort();
      return Response.json({ result: capabilities([]) });
    }
    if (reads === 2) throw new Error('discovery unavailable');
    return Response.json({ result: capabilities(formats) });
  };
  try {
    await assert.rejects(catalog.read(disposed.signal), { name: 'AbortError' });
    await assert.rejects(
      catalog.read(new AbortController().signal),
      /discovery unavailable/,
    );
    assert.deepEqual(await catalog.read(new AbortController().signal), formats);
    assert.equal(reads, 3);
  } finally {
    globalThis.fetch = original;
  }
});
