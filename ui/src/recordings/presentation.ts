import type { Run } from '../api.js';

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

type RecordingState = Pick<
  Run,
  'runtime' | 'status' | 'exit_code' | 'source' | 'correctness'
>;

export type RecordingStatus = {
  label: string;
  detail: string;
  tone: 'good' | 'warn' | 'bad' | '';
};

/** Collection status is independent of target success and source coverage. */
export function captureQuality(run: RecordingState): RecordingStatus {
  const source = metadata(run.source);
  const completion = metadata(source.collection_completion);
  const raw = metadata(source.raw_retention);
  const state = text(source.collection_status);
  if (completion.state === 'incomplete' || raw.complete === false)
    return {
      label: 'Partial capture',
      detail:
        'Some capture data is missing. Inspect collection notes before using it.',
      tone: 'warn',
    };
  if (state === 'failed' || state === 'interrupted' || state === 'canceled')
    return {
      label: state === 'failed' ? 'Capture failed' : 'Capture interrupted',
      detail: 'Previously accepted data may still be available.',
      tone: state === 'failed' ? 'bad' : 'warn',
    };
  if (state === 'finalizing')
    return {
      label: 'Finalizing capture',
      detail: 'Saving and processing collected data.',
      tone: '',
    };
  if (run.status === 'running')
    return {
      label: 'Capturing',
      detail: 'The recording is still running.',
      tone: '',
    };
  if (state === 'disabled' || run.runtime === 'none')
    return {
      label: 'Collection off',
      detail: 'Automatic profiling was disabled.',
      tone: '',
    };
  if (state === 'completed')
    return {
      label: 'Capture ready',
      detail: text(completion.scope) ?? 'Collected data is ready to inspect.',
      tone: 'good',
    };
  if (run.runtime === 'imported')
    return {
      label: 'Imported profile',
      detail: 'Collection conditions come from the imported file.',
      tone: '',
    };
  return {
    label: 'Capture status unknown',
    detail: 'This recording has no collection status.',
    tone: '',
  };
}

/** Do not replace a missing native target result with the collector's exit. */
export function targetCheck(run: RecordingState): RecordingStatus {
  const source = metadata(run.source);
  const target = metadata(source.target_outcome);
  const correctness = metadata(run.correctness);
  if (run.runtime === 'imported')
    return {
      label: 'Target not recorded',
      detail: 'Imported profiles do not include a workload check.',
      tone: '',
    };
  if (correctness.timed_out === true || target.status === 'timed_out')
    return {
      label: 'Timed out',
      detail: 'The target exceeded its recording timeout.',
      tone: 'bad',
    };
  if (
    correctness.canceled === true ||
    target.status === 'canceled' ||
    run.status === 'canceled'
  )
    return {
      label: 'Canceled',
      detail: 'The recording was canceled.',
      tone: 'warn',
    };
  if (target.state === 'unknown')
    return {
      label: 'Target outcome unknown',
      detail: text(target.reason) ?? 'The target exit result was not captured.',
      tone: 'warn',
    };
  if (target.status === 'not_started')
    return {
      label: 'Target not started',
      detail: 'The workload did not start.',
      tone: '',
    };
  if (target.state === 'signaled')
    return {
      label:
        typeof target.signal === 'number'
          ? `Signal ${target.signal}`
          : 'Target terminated',
      detail: 'The target terminated without an exit code.',
      tone: 'bad',
    };
  const exit =
    typeof correctness.actual_exit_code === 'number'
      ? correctness.actual_exit_code
      : typeof target.exit_code === 'number'
        ? target.exit_code
        : run.exit_code;
  if (exit === null && run.status === 'running')
    return {
      label: 'Running',
      detail: 'The target is still running.',
      tone: '',
    };
  if (exit === null)
    return {
      label: 'Target outcome unknown',
      detail:
        text(correctness.reason) ?? 'The target exit result was not captured.',
      tone: 'warn',
    };
  const passed = correctness.passed;
  return {
    label: `Exited ${exit}`,
    detail:
      passed === false
        ? `Workload checks failed.${typeof correctness.reason === 'string' ? ` ${correctness.reason}` : ''}`
        : passed === true
          ? correctness.validation === 'exit_code_and_stdout'
            ? 'Exit code and output matched the expected values.'
            : correctness.validation === 'exit_code_only'
              ? 'Exit code matched; output not checked.'
              : 'Recorded workload checks passed.'
          : 'Workload checks are unavailable.',
    tone: passed === true ? 'good' : passed === false ? 'bad' : '',
  };
}

/** Display retained identities without assuming that source and executable match. */
export function sourceIdentityMetadata(value: unknown) {
  const source = metadata(value);
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
