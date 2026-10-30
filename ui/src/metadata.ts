function document(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function strings(value: unknown): string[] | undefined {
  return Array.isArray(value) &&
    value.every((item): item is string => typeof item === 'string')
    ? value
    : undefined;
}

function number(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value)
    ? value
    : undefined;
}

export function scenarioMetadata(value: unknown) {
  const row = document(value);
  return {
    id: text(row.id),
    name: text(row.name) ?? 'Unknown workload',
    command: strings(row.command),
    cwd: text(row.cwd),
    runtime: text(row.runtime),
    timeout_seconds: number(row.timeout_seconds),
    expected_exit_code: number(row.expected_exit_code),
    env: row.env,
    input_files: row.input_files,
    expected_stdout: row.expected_stdout,
    expected_stdout_sha256: row.expected_stdout_sha256,
    raw: value,
  };
}

/** Display retained identities without assuming that source and executable match. */
export function sourceIdentityMetadata(value: unknown) {
  const source = document(value);
  return {
    sourceFingerprint: text(source.fingerprint),
    executableSha256: text(source.executable_sha256),
    runtimeVersion: text(source.runtime_version),
    buildSourceVerified:
      typeof source.build_source_verified === 'boolean'
        ? source.build_source_verified
        : undefined,
  };
}
