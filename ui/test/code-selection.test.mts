import test from 'node:test';
import assert from 'node:assert/strict';
import {
  mappingAnchors,
  mappingOccurrenceIndex,
  mappingScope,
} from '../src/code/selection.js';
import { codeCapture } from '../src/code/capture.js';
import type { Observation } from '../src/record.generated.js';
import { Investigations } from '../src/investigation/state.js';

test('mapping anchors preserve actual sample/frame identity including recursive occurrences', () => {
  const sample: Observation = {
    schema_version: 1,
    kind: 'sample',
    id: 'sample',
    run_id: 'run',
    producer_id: 'p',
    pid: 1,
    tid: 't',
    timestamp_us: 1,
    basis: 'sampled',
    weight: 1,
    weight_unit: 'samples',
    stack: [
      {
        function: 'work',
        file: '/project/work.ts',
        code_id: 'code',
        code_instance_id: 'first',
      },
      {
        function: 'work',
        file: '/project/work.ts',
        code_id: 'code',
        code_instance_id: 'second',
      },
    ],
  };
  const selected = {
    function: 'work',
    file: '/project/work.ts',
    module: null,
    code_id: 'code',
  };
  assert.deepEqual(
    mappingAnchors([sample], selected).anchors.map((value) => [
      value.observation_id,
      value.frame_index,
    ]),
    [
      ['sample', 0],
      ['sample', 1],
    ],
  );
  assert.equal(
    mappingAnchors([sample], { ...selected, file: '/other/work.ts' }).anchors
      .length,
    0,
  );
  assert.equal(
    mappingAnchors([sample], { ...selected, code_id: 'other-code' }).anchors
      .length,
    0,
  );
  assert.equal(mappingAnchors([sample], selected, 1).truncated, true);
  assert.equal(
    mappingAnchors([sample], { ...selected, evidence_truncated: true })
      .truncated,
    true,
  );
});

test('automatic capture keeps existing request bytes and code-origin options are explicit and bounded', () => {
  assert.deepEqual(
    codeCapture('automatic-cpu', {
      compilerExport: true,
      filter: 'work',
      interval: '1000',
      budget: '10000',
    }),
    {},
  );
  assert.deepEqual(
    codeCapture('code-origin', {
      compilerExport: true,
      filter: 'work',
      interval: '1000',
      budget: '67108864',
    }),
    {
      capture_mode: 'code-origin',
      code_options: {
        compiler_export: true,
        function_filter: 'work',
        sampling_interval_us: 1000,
        max_artifact_bytes: 67108864,
      },
    },
  );
  for (const interval of ['0', '1.1', 'nope'])
    assert.throws(() =>
      codeCapture('code-origin', {
        compilerExport: false,
        filter: '',
        interval,
        budget: '',
      }),
    );
});

test('relative clock and mapping inspection preference survive private state restoration without becoming epoch alignment', () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
  const first = new Investigations(storage);
  first.observeResult('run', 'same');
  first.forRun('run').workbench.clockDomain = 'runtime:producer-1';
  first.forRun('run').workbench.inspectorTab = 'mapping';
  first.persist('run');
  const restored = new Investigations(storage);
  restored.observeResult('run', 'same');
  assert.equal(
    restored.forRun('run').workbench.clockDomain,
    'runtime:producer-1',
  );
  assert.equal(restored.forRun('run').workbench.inspectorTab, 'mapping');
});

test('Wasm disassembly index is opt-in, compiler-only and does not change ordinary capture', () => {
  const values = {
    compilerExport: true,
    filter: '',
    interval: '',
    budget: '',
    wasmFunctionIndex: '',
  };
  assert.deepEqual(codeCapture('code-origin', values), {
    capture_mode: 'code-origin',
    code_options: { compiler_export: true },
  });
  for (const wasmFunctionIndex of ['0', '2147483647'])
    assert.deepEqual(
      codeCapture('code-origin', { ...values, wasmFunctionIndex }),
      {
        capture_mode: 'code-origin',
        code_options: {
          compiler_export: true,
          wasm_function_index: Number(wasmFunctionIndex),
        },
      },
    );
  for (const wasmFunctionIndex of ['-1', '2147483648', '1.5', 'invalid'])
    assert.throws(() =>
      codeCapture('code-origin', { ...values, wasmFunctionIndex }),
    );
  assert.throws(() =>
    codeCapture('code-origin', {
      ...values,
      compilerExport: false,
      wasmFunctionIndex: '0',
    }),
  );
  assert.deepEqual(
    codeCapture('automatic-cpu', {
      ...values,
      compilerExport: false,
      wasmFunctionIndex: 'invalid',
    }),
    {},
  );
});

test('mapping occurrence survives inspector rebuilds only within its retained result context', () => {
  const values = new Map<string, string>();
  const storage = {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => {
      values.set(key, value);
    },
  };
  const first = new Investigations(storage);
  first.observeResult('run', 'same');
  const window = { start_us: 100, end_us: 200 };
  const scope = mappingScope('run', 'function', window, 'runtime:1', 'samples');
  const occurrence = { scope, observationId: 'sample:384', frameIndex: 6 };
  const state = first.forRun('run').workbench;
  state.mappingSelection = occurrence;
  first.persist('run');
  const restored = new Investigations(storage);
  restored.observeResult('run', 'same');
  const saved = restored.forRun('run').workbench;
  assert.deepEqual(saved.mappingSelection, occurrence);
  const anchors = [
    { observation_id: 'sample:383', frame_index: 6 },
    { observation_id: 'sample:384', frame_index: 6 },
  ];
  assert.equal(
    mappingOccurrenceIndex(anchors, scope, saved.mappingSelection),
    1,
  );
  for (const changed of [
    mappingScope('other', 'function', window, 'runtime:1', 'samples'),
    mappingScope('run', 'other-function', window, 'runtime:1', 'samples'),
    mappingScope(
      'run',
      'function',
      { start_us: 100, end_us: 201 },
      'runtime:1',
      'samples',
    ),
    mappingScope('run', 'function', window, 'runtime:2', 'samples'),
    mappingScope('run', 'function', window, 'runtime:1', 'microseconds'),
  ])
    assert.equal(
      mappingOccurrenceIndex(anchors, changed, saved.mappingSelection),
      0,
    );
  assert.equal(
    mappingOccurrenceIndex(anchors.slice(0, 1), scope, saved.mappingSelection),
    0,
  );
  assert.equal(restored.forRun('other').workbench.mappingSelection, undefined);
  restored.observeResult('run', 'changed');
  assert.equal(saved.mappingSelection, undefined);
});
