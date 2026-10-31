import { ApiError } from '../api.js';

/** Source scope is a server-owned decision; unknown failures remain errors. */
export function sourceUnavailable(
  error: unknown,
): { title: string; message: string } | undefined {
  if (
    !(error instanceof ApiError) ||
    error.operationFailure?.operation !== 'source.read' ||
    error.operationFailure.code !== 'source_outside_project'
  )
    return undefined;
  return {
    title: 'Source outside measured project',
    message:
      'This file is outside the recording’s project. Hunin only reads current source within that project.',
  };
}
