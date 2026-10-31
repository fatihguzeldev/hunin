import test from 'node:test';
import assert from 'node:assert/strict';
import { codeFlags } from './plan.mjs';

test('code-origin uses explicit native logger flags without changing Wasm tier', () => {
  const flags = codeFlags('/capture with spaces', {
    sampling_interval_us: 250,
  });
  assert.ok(flags.includes('--prof'));
  assert.ok(flags.includes('--ll-prof'));
  assert.ok(flags.includes('--prof-sampling-interval=250'));
  assert.ok(flags.includes('--logfile=/capture with spaces/code-v8.log'));
  assert.ok(!flags.some((flag) => /liftoff|tier-up|cpu-prof/.test(flag)));
});

test('compiler export is explicit and options reject unsupported bounds', () => {
  assert.ok(!codeFlags('/capture', {}).includes('--trace-turbo'));
  const flags = codeFlags('/capture', {
    compiler_export: true,
    function_filter: 'generated_*',
  });
  assert.ok(flags.includes('--trace-turbo'));
  assert.ok(flags.includes('--trace-turbo-filter=generated_*'));
  assert.throws(() => codeFlags('/capture', { sampling_interval_us: 0 }));
  assert.throws(() =>
    codeFlags('/capture', { max_artifact_bytes: 536_870_913 }),
  );
  assert.throws(() => codeFlags('/capture', { function_filter: 'a\0b' }));
  assert.throws(() => codeFlags('/capture', { function_filter: '' }));
  assert.throws(() => codeFlags('/capture', { function_filter: 'a\nb' }));
  assert.throws(() =>
    codeFlags('/capture', { function_filter: 'é'.repeat(129) }),
  );
});

test('Wasm disassembly scope is opt-in, bounded and requires compiler export', () => {
  for (const index of [0, 2_147_483_647]) {
    const options = { compiler_export: true, wasm_function_index: index };
    assert.ok(
      codeFlags('/capture', options).includes(
        `--print-wasm-code-function-index=${index}`,
      ),
    );
    assert.ok(!codeFlags('/capture', options).includes('--print-wasm-code'));
  }
  assert.ok(
    !codeFlags('/capture', { compiler_export: true }).some((flag) =>
      flag.startsWith('--print-wasm-code-function-index='),
    ),
  );
  for (const index of [
    null,
    -1,
    0.5,
    2_147_483_648,
    NaN,
    Infinity,
    '0',
    true,
  ]) {
    const options = Object.assign(
      { compiler_export: true },
      { wasm_function_index: index },
    ) as Parameters<typeof codeFlags>[1];
    assert.throws(() => codeFlags('/capture', options));
  }
  const absentExport = { compiler_export: undefined, wasm_function_index: 0 };
  const disabledExport = { compiler_export: false, wasm_function_index: 0 };
  assert.throws(() => codeFlags('/capture', absentExport));
  assert.throws(() => codeFlags('/capture', disabledExport));
});
