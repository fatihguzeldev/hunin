import { join } from 'node:path';

export interface CodeOptions {
  compiler_export?: boolean;
  function_filter?: string;
  wasm_function_index?: number;
  sampling_interval_us?: number;
  max_artifact_bytes?: number;
}
export function codeFlags(directory: string, options: CodeOptions): string[] {
  const interval = options.sampling_interval_us ?? 1000;
  const maximum = options.max_artifact_bytes ?? 67_108_864;
  if (!Number.isSafeInteger(interval) || interval < 100 || interval > 1_000_000)
    throw Error(
      'code-origin sampling interval must be 100–1,000,000 microseconds',
    );
  if (!Number.isSafeInteger(maximum) || maximum < 1 || maximum > 536_870_912)
    throw Error('code-origin artifact byte budget must be 1–536,870,912');
  if (
    options.compiler_export !== undefined &&
    typeof options.compiler_export !== 'boolean'
  )
    throw Error('compiler_export must be a boolean');
  if (
    options.function_filter !== undefined &&
    (typeof options.function_filter !== 'string' ||
      Buffer.byteLength(options.function_filter) < 1 ||
      Buffer.byteLength(options.function_filter) > 256 ||
      /[\0\r\n]/.test(options.function_filter))
  )
    throw Error(
      'code-origin function filter must be 1–256 UTF-8 bytes without NUL or newlines',
    );
  if (options.wasm_function_index !== undefined) {
    if (
      !Number.isSafeInteger(options.wasm_function_index) ||
      options.wasm_function_index < 0 ||
      options.wasm_function_index > 2_147_483_647
    )
      throw Error(
        'Wasm function index must be an integer from 0 to 2,147,483,647',
      );
    if (options.compiler_export !== true)
      throw Error('Wasm function index requires compiler_export');
  }
  const flags = [
    '--prof',
    '--ll-prof',
    `--prof-sampling-interval=${interval}`,
    `--logfile=${join(directory, 'code-v8.log')}`,
  ];
  if (options.compiler_export)
    flags.push(
      '--trace-turbo',
      '--trace-turbo-graph',
      options.wasm_function_index === undefined
        ? '--print-wasm-code'
        : `--print-wasm-code-function-index=${options.wasm_function_index}`,
      `--trace-turbo-path=${directory}`,
      `--trace-turbo-cfg-file=${join(directory, 'code-turbo.cfg')}`,
      `--trace-turbo-filter=${options.function_filter ?? '*'}`,
    );
  return flags;
}
