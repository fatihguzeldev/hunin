import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import {
  generate,
  generateOperations,
  generatedMatches,
  type ObservationSchema,
  type OperationDefinition,
  type ResponseSchema,
} from '../scripts/generate-record.mjs';
import { operation, ApiError } from '../src/api.js';
import { duration, bytes, percentage } from '../src/format.js';
import { parseCommand } from '../src/command.js';
import { parseWorkloadOptions } from '../src/workload.js';
import { hasLocalSource, hasInspectableSource } from '../src/source-path.js';
import { canReanalyze } from '../src/artifact.js';
import {
  observationLabel,
  referenceIds,
  nativeEvidenceRows,
} from '../src/observation.js';
import { PagedQuery, type Page } from '../src/pagination.js';
import type { Observation } from '../src/record.generated.js';
import { runSource, runCorrectness, runStatusTone } from '../src/run.js';

test('canonical schema produces required fields per observation kind', async () => {
  const schema: ObservationSchema = JSON.parse(
    await readFile(
      new URL('../../../protocol/record.schema.json', import.meta.url),
      'utf8',
    ),
  );
  const text = generate(schema);
  for (const kind of schema.properties.kind.enum)
    assert.ok(text.includes(`{ kind: "${kind}" }`));
  assert.ok(
    text.includes(
      'Required<Pick<RecordBase, "stack" | "weight" | "weight_unit">>',
    ),
  );
  assert.ok(text.includes('"timestamp_us": number;'));
  assert.ok(
    generatedMatches(
      await readFile(
        new URL('../../src/record.generated.ts', import.meta.url),
        'utf8',
      ),
      text,
    ),
  );
});

test('operation input types are generated from the authoritative transport contract', async () => {
  const operations: OperationDefinition[] = JSON.parse(
    await readFile(
      new URL('../../../protocol/operations.json', import.meta.url),
      'utf8',
    ),
  );
  const responses: ResponseSchema = JSON.parse(
    await readFile(
      new URL('../../../protocol/response.schema.json', import.meta.url),
      'utf8',
    ),
  );
  const generated = generateOperations(operations, responses.$defs);
  assert.ok(
    generatedMatches(
      await readFile(
        new URL('../../src/operations.generated.ts', import.meta.url),
        'utf8',
      ),
      generated,
    ),
  );
  assert.ok(generated.includes('"timeline.query"'));
  assert.ok(generated.includes('"command": Array<string>'));
  assert.ok(generated.includes('export type OperationOutputs'));
  assert.ok(generated.includes('"next_cursor": string | null;'));
  assert.ok(generated.includes('"encoding": "utf8_lossy";'));
});

test('operation output generation refuses unsupported validation rules', () => {
  assert.throws(
    () =>
      generateOperations([
        {
          name: 'future',
          inputSchema: {},
          outputSchema: { type: 'string', pattern: '^accepted$' },
        },
      ]),
    /Unsupported operation output schema keyword: pattern/,
  );
});

test('generated freshness tolerates Windows checkout line endings but rejects stale types', () => {
  const generated = 'export type Example = {\n  id: string;\n};\n';
  assert.equal(generatedMatches(generated, generated), true);
  assert.equal(
    generatedMatches(generated.replaceAll('\n', '\r\n'), generated),
    true,
  );
  assert.equal(
    generatedMatches(generated.replace('string', 'number'), generated),
    false,
  );
  assert.equal(
    generatedMatches(generated.replaceAll('\n', '\r'), generated),
    false,
  );
});

test('transport preserves operation arguments and rejects backend errors', async () => {
  let captured: ({ url: RequestInfo | URL } & RequestInit) | undefined;
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async (url, request) => {
      captured = { url, ...request };
      return Response.json({
        result: { records: [], bounds: { start_us: 1, end_us: 2 } },
      });
    };
    await operation('timeline.query', {
      run_id: 'r1',
      start_us: 123,
      kind: 'sample',
      pid: 1,
    });
    assert.ok(captured);
    assert.ok(typeof captured.body === 'string');
    assert.equal(captured.url, '/api/operations');
    assert.deepEqual(JSON.parse(captured.body), {
      operation: 'timeline.query',
      params: { run_id: 'r1', start_us: 123, kind: 'sample', pid: 1 },
    });
    assert.equal(captured.credentials, 'same-origin');
    globalThis.fetch = async () =>
      Response.json(
        {
          error: {
            code: 'unsupported',
            message: 'Missing collector',
            details: { runtime: 'native' },
          },
        },
        { status: 400 },
      );
    await assert.rejects(
      operation('runs.start', { command: ['workload'] }),
      (error) =>
        error instanceof ApiError &&
        error.code === 'unsupported' &&
        error.message === 'Missing collector',
    );
    globalThis.fetch = async () => Response.json({ wrong: true });
    await assert.rejects(operation('runs.list', {}), /HTTP 200/);
    globalThis.fetch = async () => Response.json({ error: null });
    await assert.rejects(operation('runs.list', {}), /invalid error envelope/);
  } finally {
    globalThis.fetch = original;
  }
});

test('artifact responses preserve null cursors and byte offsets while rejecting malformed results', async () => {
  const original = globalThis.fetch;
  let result: unknown;
  try {
    globalThis.fetch = async () => Response.json({ result });
    result = {
      artifacts: [
        { id: 'a1', name: 'profile', size: 4, sha256: 'digest', format: 'raw' },
      ],
      next_cursor: null,
    };
    assert.deepEqual(
      await operation('artifacts.list', { run_id: 'r1' }),
      result,
    );
    result = {
      content: '\ufffd',
      encoding: 'utf8_lossy',
      next_offset: 3,
      size: 4,
    };
    assert.deepEqual(
      await operation('artifacts.read', {
        run_id: 'r1',
        artifact_id: 'a1',
        offset: 2,
        limit: 1,
      }),
      result,
    );
    result = {
      content: '',
      encoding: 'utf8_lossy',
      next_offset: null,
      size: 4,
      future_metadata: true,
    };
    assert.deepEqual(
      await operation('artifacts.read', {
        run_id: 'r1',
        artifact_id: 'a1',
        offset: 4,
      }),
      result,
    );
    for (const malformed of [
      { artifacts: [], next_cursor: 4 },
      { artifacts: [] },
      {
        artifacts: [
          {
            id: 'a1',
            name: 'profile',
            size: '4',
            sha256: 'digest',
            format: 'raw',
          },
        ],
        next_cursor: null,
      },
    ]) {
      result = malformed;
      await assert.rejects(
        operation('artifacts.list', { run_id: 'r1' }),
        /invalid artifacts.list result/,
      );
    }
    result = { content: '', encoding: 'base64', next_offset: null, size: 4 };
    await assert.rejects(
      operation('artifacts.read', { run_id: 'r1', artifact_id: 'a1' }),
      /invalid artifacts.read result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('run responses retain legacy metadata and extension statuses but reject malformed fixed fields', async () => {
  const original = globalThis.fetch;
  const run = {
    id: 'legacy',
    name: 'legacy',
    status: 'archived-by-extension',
    command: [],
    cwd: '/project',
    runtime: 'imported',
    started_at_us: 1,
    finished_at_us: null,
    exit_code: null,
    diagnostics: [],
    source: null,
    correctness: 'unverified',
  };
  let result: unknown = { runs: [run], next_cursor: null };
  try {
    globalThis.fetch = async () => Response.json({ result });
    assert.deepEqual(await operation('runs.list', {}), result);
    result = {
      runs: [{ ...run, finished_at_us: 'yesterday' }],
      next_cursor: null,
    };
    await assert.rejects(
      operation('runs.list', {}),
      /invalid runs.list result/,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test('run presentation reads known metadata safely without changing retained values', () => {
  for (const value of [
    null,
    'legacy',
    42,
    [],
    { fingerprint: 7, revision: false, passed: 'yes' },
  ]) {
    const run = { source: value, correctness: value };
    assert.equal(runSource(run).fingerprint, 'unknown');
    assert.equal(runCorrectness(run).passed, null);
    assert.equal(run.source, value);
    assert.equal(run.correctness, value);
  }
  assert.deepEqual(
    runSource({
      source: {
        fingerprint: 'measured',
        revision: null,
        dirty: true,
        build_source_verified: false,
      },
    }),
    {
      fingerprint: 'measured',
      revision: undefined,
      dirty: true,
      executable_sha256: undefined,
      runtime_version: undefined,
      build_source_verified: false,
    },
  );
  assert.equal(
    runCorrectness({
      correctness: { passed: false, reason: 'mismatch', actual_exit_code: 1 },
    }).passed,
    false,
  );
  assert.equal(runStatusTone('completed'), 'good');
  assert.equal(runStatusTone('archived-by-extension'), '');
  for (const status of ['constructor', '__proto__', 'toString']) {
    assert.equal(runStatusTone(status), '');
  }
});

test('transport rejects malformed error fields instead of manufacturing an ApiError', async () => {
  const original = globalThis.fetch;
  try {
    for (const error of [
      { code: 7, message: 'failure' },
      { code: 'failure', message: [] },
      {},
    ]) {
      globalThis.fetch = async () => Response.json({ error }, { status: 400 });
      await assert.rejects(
        operation('runs.list', {}),
        /invalid error envelope/,
      );
    }
  } finally {
    globalThis.fetch = original;
  }
});

test('weight formatting keeps sample counts separate from elapsed durations', async () => {
  assert.equal(duration(1_000_000), '1 ms');
  assert.equal(duration(1_500_000_000), '1.5 s');
  assert.equal(bytes(1024), '1 KiB');
  assert.equal(percentage(null), '—');
  assert.equal(percentage(undefined), '—');
  assert.equal(percentage(0), '0%');
  assert.equal(percentage(50), '+50%');
});

test('recording commands retain literal arguments and reject ambiguous input', async () => {
  assert.deepEqual(
    parseCommand('["node", "file with spaces.js", "$(literal)", ""]'),
    ['node', 'file with spaces.js', '$(literal)', ''],
  );
  for (const invalid of [
    'node app.js',
    '[]',
    '[""]',
    '["node", 1]',
    '{}',
    'null',
  ])
    assert.throws(() => parseCommand(invalid));
});

test('workload recipes preserve exact output bytes and reject invalid environment declarations', async () => {
  const exact = parseWorkloadOptions(
    '{"MODE":"literal $(value)"}',
    '["data/input.json"]',
    '0',
    'text',
    'result\n',
  );
  assert.deepEqual(exact, {
    env: { MODE: 'literal $(value)' },
    input_files: ['data/input.json'],
    expected_exit_code: 0,
    expected_stdout: 'result\n',
  });
  assert.equal(
    parseWorkloadOptions('{}', '[]', '1', 'text', '').expected_stdout,
    '',
  );
  const hash = parseWorkloadOptions('{}', '[]', '0', 'sha256', 'A'.repeat(64));
  assert.equal(hash.expected_stdout_sha256, 'a'.repeat(64));
  assert.equal('expected_stdout' in hash, false);
  assert.equal(
    'expected_stdout' in
      parseWorkloadOptions('{}', '[]', '0', 'exit', 'ignored'),
    false,
  );
  for (const environment of ['[]', 'null', '{"EMPTY":1}', '{"A=B":"value"}'])
    assert.throws(() =>
      parseWorkloadOptions(environment, '[]', '0', 'exit', ''),
    );
  for (const files of ['{}', '[1]', '[""]'])
    assert.throws(() => parseWorkloadOptions('{}', files, '0', 'exit', ''));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '0', 'sha256', 'abc'));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '1.5', 'exit', ''));
});

test('virtual runtime source locations are not offered as local filesystem paths', async () => {
  for (const local of [
    '/src/main.rs',
    'src/main.py',
    'file:///src/a%20b.js',
    'C:\\src\\main.cpp',
  ])
    assert.equal(hasLocalSource(local), true);
  for (const virtual of [
    null,
    undefined,
    '',
    'node:internal/loader',
    'wasm:wasm-function[1]',
    'https://example.test/app.js',
  ])
    assert.equal(hasLocalSource(virtual), false);
  for (const retained of [
    'webpack://app/src/main.ts',
    'https://example.test/src.ts',
  ]) {
    assert.equal(hasInspectableSource(retained), false);
    assert.equal(
      hasInspectableSource(retained, 'measured-script-sha256'),
      true,
    );
  }
  for (const internal of ['node:internal/loader', 'wasm:wasm-function[1]'])
    assert.equal(hasInspectableSource(internal, 'code-id'), false);
});

test('profile reanalysis actions exclude SDK spools and analyzer output artifacts', async () => {
  for (const format of [
    'samply',
    'v8-cpuprofile',
    'vmprof',
    'go-pprof',
    'go-trace',
    'graal-cpusampler',
  ])
    assert.equal(canReanalyze(format), true);
  for (const format of [
    'hunin-jsonl',
    'jsonl',
    'stdout',
    'analysis-output',
    'source',
    'native-image',
    'profile-sidecar',
  ])
    assert.equal(canReanalyze(format), false);
});

test('native lifetime labels retain instance identity and unknown mapping evidence', async () => {
  const first: Observation = {
    schema_version: 1,
    kind: 'event',
    id: 'module',
    run_id: 'r',
    producer_id: 'p',
    tid: '7',
    timestamp_us: 0,
    basis: 'observed',
    name: 'module.observed_lifetime',
    pid: 7,
    attributes: {
      scope: 'module',
      module: '/tmp/libA.dylib',
      process_instance_id: 'p1',
      mapping_instance_id: 'load1',
      start_censored: true,
      end_censored: false,
      start_window_ms: [null, 3],
      exact_event_timestamps: false,
    },
  };
  const again = {
    ...first,
    attributes: { ...first.attributes, mapping_instance_id: 'load3' },
  };
  assert.equal(observationLabel(first), 'libA.dylib · p1/load1');
  assert.notEqual(observationLabel(first), observationLabel(again));
  assert.ok(
    nativeEvidenceRows(first).some(
      ([name, value]) =>
        name.startsWith('Start window') && value === '[null,3]',
    ),
  );
  assert.ok(
    nativeEvidenceRows(first).some(
      ([name, value]) => name === 'Exact event timestamps' && value === 'false',
    ),
  );
  const sample: Observation = {
    ...first,
    name: 'mapping-control',
    evidence: ['raw'],
    attributes: {
      native_load_instances: [
        { record_id: 'module-1', load_id: 'load1' },
        { record_id: null, reason: 'mapping_unresolved' },
        { record_id: 'module-1' },
      ],
    },
  };
  assert.deepEqual(referenceIds(sample), ['raw', 'module-1']);
  assert.ok(
    nativeEvidenceRows(sample).some(([, value]) =>
      value.includes('mapping_unresolved'),
    ),
  );
  assert.deepEqual(
    nativeEvidenceRows({
      ...first,
      name: 'mapping-control',
      attributes: { native_mapping_status: 'sample_link_unavailable' },
    }),
    [['Native mapping status', 'sample_link_unavailable']],
  );
});

test('metadata page navigation retains history, handles failures, and rejects stale refreshes', async () => {
  const requested: (string | undefined)[] = [];
  let fail = false;
  const pages = new PagedQuery(async (cursor) => {
    requested.push(cursor);
    if (fail) throw new Error('unavailable');
    return {
      items: [cursor ?? 'first'],
      next_cursor: cursor ? null : 'opaque-next',
    };
  });
  await pages.refresh();
  await pages.next();
  assert.deepEqual(pages.page.items, ['opaque-next']);
  assert.equal(pages.hasPrevious, true);
  await pages.refresh(); // Polling keeps the historical page selected.
  assert.equal(requested.at(-1), 'opaque-next');
  fail = true;
  await assert.rejects(pages.previous(), /unavailable/);
  assert.deepEqual(pages.page.items, ['opaque-next']);
  assert.equal(pages.hasPrevious, true);
  fail = false;
  await pages.previous();
  assert.deepEqual(pages.page.items, ['first']);
  assert.equal(pages.hasPrevious, false);
  await pages.next();
  await pages.refresh(true);
  assert.deepEqual(pages.page.items, ['first']);
  assert.equal(pages.hasPrevious, false);

  const pending: {
    resolve: (page: Page<string>) => void;
    reject: (error: Error) => void;
  }[] = [];
  const concurrent = new PagedQuery<string>(
    () =>
      new Promise<Page<string>>((resolve, reject) =>
        pending.push({ resolve, reject }),
      ),
  );
  const old = concurrent.refresh();
  const current = concurrent.refresh(true);
  pending[1].resolve({ items: ['current'] });
  assert.equal(await current, true);
  pending[0].resolve({ items: ['stale'] });
  assert.equal(await old, false);
  assert.deepEqual(concurrent.page.items, ['current']);
  const staleFailure = concurrent.refresh();
  const latest = concurrent.refresh();
  pending[3].resolve({ items: ['latest'] });
  await latest;
  pending[2].reject(new Error('stale failure'));
  assert.equal(await staleFailure, false);
  assert.deepEqual(concurrent.page.items, ['latest']);
});
