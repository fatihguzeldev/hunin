import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';
import { generate, generateOperations, generatedMatches } from '../scripts/generate-record.mjs';

async function moduleFrom(path) {
  const source = await readFile(new URL(path, import.meta.url), 'utf8');
  const { outputText } = ts.transpileModule(source, { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 } });
  return import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`);
}

test('canonical schema produces required fields per observation kind', async () => {
  const schema = JSON.parse(await readFile(new URL('../../protocol/record.schema.json', import.meta.url), 'utf8'));
  const text = generate(schema);
  for (const kind of schema.properties.kind.enum) assert.ok(text.includes(`{ kind: "${kind}" }`));
  assert.ok(text.includes('Required<Pick<RecordBase, "stack" | "weight" | "weight_unit">>'));
  assert.ok(text.includes('"timestamp_us": number;'));
  assert.ok(generatedMatches(await readFile(new URL('../src/record.generated.ts', import.meta.url), 'utf8'), text));
});

test('operation input types are generated from the authoritative transport contract', async () => {
  const operations = JSON.parse(await readFile(new URL('../../protocol/operations.json', import.meta.url), 'utf8'));
  const generated = generateOperations(operations);
  assert.ok(generatedMatches(await readFile(new URL('../src/operations.generated.ts', import.meta.url), 'utf8'), generated));
  assert.ok(generated.includes('"timeline.query"'));
  assert.ok(generated.includes('"command": Array<string>'));
});

test('generated freshness tolerates Windows checkout line endings but rejects stale types', () => {
  const generated = 'export type Example = {\n  id: string;\n};\n';
  assert.equal(generatedMatches(generated, generated), true);
  assert.equal(generatedMatches(generated.replaceAll('\n', '\r\n'), generated), true);
  assert.equal(generatedMatches(generated.replace('string', 'number'), generated), false);
  assert.equal(generatedMatches(generated.replaceAll('\n', '\r'), generated), false);
});

test('transport preserves operation arguments and rejects backend errors', async () => {
  const { operation, ApiError } = await moduleFrom('../src/api.ts');
  let captured;
  const original = globalThis.fetch;
  try {
    globalThis.fetch = async (url, request) => {
      captured = { url, ...request };
      return Response.json({ result: { records: [], bounds: { start_us: 1, end_us: 2 } } });
    };
    await operation('timeline.query', { run_id: 'r1', start_us: 123, kind: 'sample', pid: 1 });
    assert.equal(captured.url, '/api/operations');
    assert.deepEqual(JSON.parse(captured.body), { operation: 'timeline.query', params: { run_id: 'r1', start_us: 123, kind: 'sample', pid: 1 } });
    assert.equal(captured.credentials, 'same-origin');
    globalThis.fetch = async () => Response.json({ error: { code: 'unsupported', message: 'Missing collector', details: { runtime: 'native' } } }, { status: 400 });
    await assert.rejects(operation('runs.start', {}), error => error instanceof ApiError && error.code === 'unsupported' && error.message === 'Missing collector');
    globalThis.fetch = async () => Response.json({ wrong: true });
    await assert.rejects(operation('runs.list', {}), /HTTP 200/);
    globalThis.fetch = async () => Response.json({ error: null });
    await assert.rejects(operation('runs.list', {}), /invalid error envelope/);
  } finally { globalThis.fetch = original; }
});

test('weight formatting keeps sample counts separate from elapsed durations', async () => {
  const { duration, bytes, percentage } = await moduleFrom('../src/format.ts');
  assert.equal(duration(1_000_000), '1 ms');
  assert.equal(duration(1_500_000_000), '1.5 s');
  assert.equal(bytes(1024), '1 KiB');
  assert.equal(percentage(null), '—');
  assert.equal(percentage(undefined), '—');
  assert.equal(percentage(0), '0%');
  assert.equal(percentage(50), '+50%');
});

test('recording commands retain literal arguments and reject ambiguous input', async () => {
  const { parseCommand } = await moduleFrom('../src/command.ts');
  assert.deepEqual(parseCommand('["node", "file with spaces.js", "$(literal)", ""]'), ['node', 'file with spaces.js', '$(literal)', '']);
  for (const invalid of ['node app.js', '[]', '[""]', '["node", 1]', '{}', 'null']) assert.throws(() => parseCommand(invalid));
});

test('workload recipes preserve exact output bytes and reject invalid environment declarations', async () => {
  const { parseWorkloadOptions } = await moduleFrom('../src/workload.ts');
  const exact = parseWorkloadOptions('{"MODE":"literal $(value)"}', '["data/input.json"]', '0', 'text', 'result\n');
  assert.deepEqual(exact, { env: { MODE: 'literal $(value)' }, input_files: ['data/input.json'], expected_exit_code: 0, expected_stdout: 'result\n' });
  assert.equal(parseWorkloadOptions('{}', '[]', '1', 'text', '').expected_stdout, '');
  const hash = parseWorkloadOptions('{}', '[]', '0', 'sha256', 'A'.repeat(64));
  assert.equal(hash.expected_stdout_sha256, 'a'.repeat(64));
  assert.equal('expected_stdout' in hash, false);
  assert.equal('expected_stdout' in parseWorkloadOptions('{}', '[]', '0', 'exit', 'ignored'), false);
  for (const environment of ['[]', 'null', '{"EMPTY":1}', '{"A=B":"value"}']) assert.throws(() => parseWorkloadOptions(environment, '[]', '0', 'exit', ''));
  for (const files of ['{}', '[1]', '[""]']) assert.throws(() => parseWorkloadOptions('{}', files, '0', 'exit', ''));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '0', 'sha256', 'abc'));
  assert.throws(() => parseWorkloadOptions('{}', '[]', '1.5', 'exit', ''));
});

test('virtual runtime source locations are not offered as local filesystem paths', async () => {
  const { hasLocalSource, hasInspectableSource } = await moduleFrom('../src/source-path.ts');
  for (const local of ['/src/main.rs', 'src/main.py', 'file:///src/a%20b.js', 'C:\\src\\main.cpp']) assert.equal(hasLocalSource(local), true);
  for (const virtual of [null, undefined, '', 'node:internal/loader', 'wasm:wasm-function[1]', 'https://example.test/app.js']) assert.equal(hasLocalSource(virtual), false);
  for (const retained of ['webpack://app/src/main.ts', 'https://example.test/src.ts']) {
    assert.equal(hasInspectableSource(retained), false);
    assert.equal(hasInspectableSource(retained, 'measured-script-sha256'), true);
  }
  for (const internal of ['node:internal/loader', 'wasm:wasm-function[1]']) assert.equal(hasInspectableSource(internal, 'code-id'), false);
});

test('profile reanalysis actions exclude SDK spools and analyzer output artifacts', async () => {
  const { canReanalyze } = await moduleFrom('../src/artifact.ts');
  for (const format of ['samply', 'v8-cpuprofile', 'vmprof', 'go-pprof', 'go-trace', 'graal-cpusampler']) assert.equal(canReanalyze(format), true);
  for (const format of ['hunin-jsonl', 'jsonl', 'stdout', 'analysis-output', 'source', 'native-image', 'profile-sidecar']) assert.equal(canReanalyze(format), false);
});

test('native lifetime labels retain instance identity and unknown mapping evidence', async () => {
  const { observationLabel, referenceIds, nativeEvidenceRows } = await moduleFrom('../src/observation.ts');
  const first = { name: 'module.observed_lifetime', pid: 7, attributes: { scope: 'module', module: '/tmp/libA.dylib', process_instance_id: 'p1', mapping_instance_id: 'load1', start_censored: true, end_censored: false, start_window_ms: [null, 3], exact_event_timestamps: false } };
  const again = { ...first, attributes: { ...first.attributes, mapping_instance_id: 'load3' } };
  assert.equal(observationLabel(first), 'libA.dylib · p1/load1');
  assert.notEqual(observationLabel(first), observationLabel(again));
  assert.ok(nativeEvidenceRows(first).some(([name, value]) => name.startsWith('Start window') && value === '[null,3]'));
  assert.ok(nativeEvidenceRows(first).some(([name, value]) => name === 'Exact event timestamps' && value === 'false'));
  const sample = { evidence: ['raw'], attributes: { native_load_instances: [
    { record_id: 'module-1', load_id: 'load1' }, { record_id: null, reason: 'mapping_unresolved' }, { record_id: 'module-1' },
  ] } };
  assert.deepEqual(referenceIds(sample), ['raw', 'module-1']);
  assert.ok(nativeEvidenceRows(sample).some(([, value]) => value.includes('mapping_unresolved')));
  assert.deepEqual(nativeEvidenceRows({ attributes: { native_mapping_status: 'sample_link_unavailable' } }), [['Native mapping status', 'sample_link_unavailable']]);
});

test('metadata page navigation retains history, handles failures, and rejects stale refreshes', async () => {
  const { PagedQuery } = await moduleFrom('../src/pagination.ts');
  const requested = [];
  let fail = false;
  const pages = new PagedQuery(async cursor => {
    requested.push(cursor);
    if (fail) throw new Error('unavailable');
    return { items: [cursor ?? 'first'], next_cursor: cursor ? null : 'opaque-next' };
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

  const pending = [];
  const concurrent = new PagedQuery(() => new Promise((resolve, reject) => pending.push({ resolve, reject })));
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
