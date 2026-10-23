export type WorkloadOptions = {
  env: Record<string, string>;
  input_files: string[];
  expected_exit_code: number;
  expected_stdout?: string;
  expected_stdout_sha256?: string;
};

/** Preserve literal values: output expectations compare exact UTF-8 bytes. */
export function parseWorkloadOptions(environment: string, files: string, exitCode: string, outputMode: string, output: string): WorkloadOptions {
  const env: unknown = JSON.parse(environment);
  if (!env || typeof env !== 'object' || Array.isArray(env) || Object.entries(env).some(([key, value]) => !key || /[=\0]/.test(key) || typeof value !== 'string' || value.includes('\0'))) {
    throw new Error('Environment overrides must be a JSON object with valid names and string values.');
  }
  const inputs: unknown = JSON.parse(files);
  if (!Array.isArray(inputs) || inputs.some(value => typeof value !== 'string' || !value || value.includes('\0'))) {
    throw new Error('Input files must be a JSON array of non-empty paths.');
  }
  const expected_exit_code = Number(exitCode);
  if (!Number.isInteger(expected_exit_code) || expected_exit_code < 0 || expected_exit_code > 255) throw new Error('Expected exit code must be an integer between 0 and 255.');
  const result: WorkloadOptions = { env: env as Record<string, string>, input_files: inputs as string[], expected_exit_code };
  if (outputMode === 'text') result.expected_stdout = output;
  else if (outputMode === 'sha256') {
    if (!/^[a-fA-F0-9]{64}$/.test(output)) throw new Error('Expected stdout SHA-256 must contain exactly 64 hexadecimal characters.');
    result.expected_stdout_sha256 = output.toLowerCase();
  } else if (outputMode !== 'exit') throw new Error('Choose a valid output check.');
  return result;
}
