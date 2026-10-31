import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { transpileModule, ModuleKind, ScriptTarget } from 'typescript';
import {
  generate,
  generateOperations,
  responseDefinitions,
  generatedMatches,
  type ObservationSchema,
  type OperationDefinition,
  type ResponseSchema,
} from '../scripts/generate-record.mjs';

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
  const record: ObservationSchema = JSON.parse(
    await readFile(
      new URL('../../../protocol/record.schema.json', import.meta.url),
      'utf8',
    ),
  );
  const generated = generateOperations(
    operations,
    responseDefinitions(responses, record),
  );
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
  assert.ok(generated.includes('"encoding": "utf8_lossy" | "hex";'));
});

test('operation output generation refuses unsupported validation rules', () => {
  assert.throws(
    () =>
      generateOperations([
        {
          name: 'future',
          inputSchema: {},
          outputSchema: {
            type: 'string',
            format: 'uri',
          } as unknown as OperationDefinition['outputSchema'],
        },
      ]),
    /Unsupported operation output schema keyword: format/,
  );
  assert.throws(
    () => generateOperations([{ name: 'missing', inputSchema: {} }]),
    /Missing operation output schema/,
  );
  assert.throws(
    () =>
      generateOperations(
        [
          {
            name: 'reference',
            inputSchema: {},
            outputSchema: { $ref: '#/$defs/Example', type: 'string' },
          },
        ],
        { Example: { type: 'object' } },
      ),
    /Unsupported response reference siblings/,
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

test('generated transport guards enforce canonical hexadecimal ranges', () => {
  const generated = generateOperations([
    {
      name: 'hex.check',
      inputSchema: {},
      outputSchema: { type: 'string', pattern: '^0x(?:0|[1-9a-f][0-9a-f]*)$' },
    },
  ]);
  const compiled = transpileModule(generated, {
    compilerOptions: {
      module: ModuleKind.CommonJS,
      target: ScriptTarget.ES2022,
    },
  }).outputText;
  const exports: {
    validateOperationOutput?: (name: string, value: unknown) => void;
  } = {};
  new Function('exports', compiled)(exports);
  const validate = exports.validateOperationOutput!;
  for (const value of ['0x0', '0x1', '0xabcdef'])
    assert.doesNotThrow(() => validate('hex.check', value));
  for (const value of ['0x00', '0X1', '0xABC', 'before0x1', '', null, 1])
    assert.throws(
      () => validate('hex.check', value),
      /invalid hex.check result/,
    );
});

test('generated transport guards enforce Unicode string length limits', () => {
  const generated = generateOperations([
    {
      name: 'bounded.text',
      inputSchema: {},
      outputSchema: {
        type: 'string',
        maxLength: 2,
      } as unknown as OperationDefinition['outputSchema'],
    },
  ]);
  const compiled = transpileModule(generated, {
    compilerOptions: {
      module: ModuleKind.CommonJS,
      target: ScriptTarget.ES2022,
    },
  }).outputText;
  const exports: {
    validateOperationOutput?: (name: string, value: unknown) => void;
  } = {};
  new Function('exports', compiled)(exports);
  for (const value of ['', 'ab', '😀😀', 'a😀'])
    assert.doesNotThrow(() =>
      exports.validateOperationOutput!('bounded.text', value),
    );
  for (const value of ['abc', '😀😀😀', 'a😀b'])
    assert.throws(
      () => exports.validateOperationOutput!('bounded.text', value),
      /invalid bounded.text result/,
    );
});
