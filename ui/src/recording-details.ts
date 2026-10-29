import { ApiError, operation, type Operations, type Summary } from './api.js';

export type RecordingDetails = Operations['runs.metadata']['output'] & {
  summary?: Summary;
  summaryError?: unknown;
};

/** A failed aggregate read must not hide independently available run metadata. */
export async function recordingDetails(
  runId: string,
  signal: AbortSignal,
): Promise<RecordingDetails> {
  try {
    const details = await operation('runs.get', { run_id: runId }, signal);
    signal.throwIfAborted();
    return details;
  } catch (summaryError) {
    signal.throwIfAborted();
    try {
      const metadata = await operation(
        'runs.metadata',
        { run_id: runId },
        signal,
      );
      signal.throwIfAborted();
      return { ...metadata, summaryError };
    } catch {
      signal.throwIfAborted();
      throw summaryError;
    }
  }
}

/** Status polling needs no aggregate; older services have only runs.get. */
export async function recordingMetadata(
  runId: string,
  signal: AbortSignal,
): Promise<Operations['runs.metadata']['output']> {
  try {
    const metadata = await operation(
      'runs.metadata',
      { run_id: runId },
      signal,
    );
    signal.throwIfAborted();
    return metadata;
  } catch (error) {
    signal.throwIfAborted();
    if (
      !(error instanceof ApiError) ||
      (error.operationFailure?.code ?? error.code) !== 'unknown_operation'
    )
      throw error;
    const details = await operation('runs.get', { run_id: runId }, signal);
    signal.throwIfAborted();
    return { run: details.run, result_identity: details.result_identity };
  }
}

export function queryFailureMessage(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return error instanceof ApiError && error.operationFailure
    ? `${error.operationFailure.code} · ${message}`
    : message;
}
