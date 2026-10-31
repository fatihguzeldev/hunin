import { ApiError } from '../api.js';

/** A lost response can follow acceptance; only typed admission rejection is certain. */
export function rejectedBeforeAdmission(
  error: unknown,
  operation: 'runs.start' | 'recordings.import',
): boolean {
  if (!(error instanceof ApiError)) return false;
  const failure = error.operationFailure;
  return (
    failure?.operation === operation &&
    ['invalid_parameters', 'unknown_operation', 'capacity_exceeded'].includes(
      failure.code,
    )
  );
}
