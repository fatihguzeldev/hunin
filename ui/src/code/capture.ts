import type { OperationInputs } from '../operations.generated.js';

export type CodeCaptureValues = {
  compilerExport: boolean;
  filter: string;
  interval: string;
  budget: string;
  wasmFunctionIndex?: string;
};

export function codeCapture(
  mode: string,
  values: CodeCaptureValues,
): Pick<OperationInputs['runs.start'], 'capture_mode' | 'code_options'> {
  if (mode !== 'code-origin') return {};
  const options: NonNullable<OperationInputs['runs.start']['code_options']> =
    {};
  if (values.compilerExport) options.compiler_export = true;
  const wasmIndex = values.wasmFunctionIndex?.trim();
  if (wasmIndex) {
    if (!values.compilerExport)
      throw new Error(
        'Wasm disassembly function index requires compiler export.',
      );
    const index = Number(wasmIndex);
    if (!Number.isSafeInteger(index) || index < 0 || index > 2_147_483_647)
      throw new Error(
        'Wasm disassembly function index must be an integer between 0 and 2147483647.',
      );
    options.wasm_function_index = index;
  }
  if (values.filter.trim()) {
    const filter = values.filter.trim();
    if (
      new TextEncoder().encode(filter).length > 256 ||
      /[\0\n\r]/.test(filter)
    )
      throw new Error(
        'Function filter must contain at most 256 bytes without newlines.',
      );
    options.function_filter = filter;
  }
  for (const [text, key, min, max, label] of [
    [
      values.interval,
      'sampling_interval_us',
      100,
      1_000_000,
      'Sampling interval',
    ],
    [values.budget, 'max_artifact_bytes', 1, 536_870_912, 'Artifact budget'],
  ] as const) {
    if (!text) continue;
    const value = Number(text);
    if (!Number.isSafeInteger(value) || value < min || value > max)
      throw new Error(`${label} must be an integer between ${min} and ${max}.`);
    options[key] = value;
  }
  return {
    capture_mode: 'code-origin',
    ...(Object.keys(options).length ? { code_options: options } : {}),
  };
}
