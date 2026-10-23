import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import ts from 'typescript';
import { generate, generateOperations } from '../scripts/generate-record.mjs';

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
  assert.equal(await readFile(new URL('../src/record.generated.ts', import.meta.url), 'utf8'), text);
});

test('operation input types are generated from the authoritative transport contract', async () => {
  const operations = JSON.parse(await readFile(new URL('../../protocol/operations.json', import.meta.url), 'utf8'));
  const generated = generateOperations(operations);
  assert.equal(await readFile(new URL('../src/operations.generated.ts', import.meta.url), 'utf8'), generated);
  assert.ok(generated.includes('"timeline.query"'));
  assert.ok(generated.includes('"command": Array<string>'));
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

test('virtual runtime source locations are not offered as local filesystem paths', async () => {
  const { hasLocalSource } = await moduleFrom('../src/source-path.ts');
  for (const local of ['/src/main.rs', 'src/main.py', 'file:///src/a%20b.js', 'C:\\src\\main.cpp']) assert.equal(hasLocalSource(local), true);
  for (const virtual of [null, undefined, '', 'node:internal/loader', 'wasm:wasm-function[1]', 'https://example.test/app.js']) assert.equal(hasLocalSource(virtual), false);
});
