import type { Run } from './api';

function metadata(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : {};
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

/** Present known fields without narrowing or rewriting retained raw metadata. */
export function runSource(run: Pick<Run, 'source'>) {
  const source = metadata(run.source);
  return {
    fingerprint: text(source.fingerprint) ?? 'unknown',
    revision: text(source.revision),
    dirty: source.dirty === true,
    executable_sha256: text(source.executable_sha256),
    runtime_version: text(source.runtime_version),
    build_source_verified: source.build_source_verified === true,
  };
}

export function runCorrectness(run: Pick<Run, 'correctness'>) {
  const correctness = metadata(run.correctness);
  return {
    passed: typeof correctness.passed === 'boolean' ? correctness.passed : null,
    actual_exit_code:
      typeof correctness.actual_exit_code === 'number'
        ? correctness.actual_exit_code
        : null,
    reason: text(correctness.reason),
    validation: text(correctness.validation),
  };
}

export function runStatusTone(status: string): string {
  const tones: Record<string, string> = {
    completed: 'good',
    failed: 'bad',
    canceled: 'warn',
    running: 'warn',
  };
  return Object.hasOwn(tones, status) ? tones[status] : '';
}
