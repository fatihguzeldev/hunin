import { queryFailureMessage } from './recording-details.js';

type RemovalNotice = {
  kind: 'pending' | 'completed' | 'unknown' | 'absent' | 'unavailable';
  message: string;
  lastError?: string;
};

export function removalFailureMessage(error: unknown): string {
  return `Removal did not return a confirmation. ${queryFailureMessage(error)} Check the retained cleanup receipt and recording state before retrying.`;
}

/** Project retained facts without authorizing cleanup or confirming a mutation. */
export function removalReceiptNotice(
  runId: string,
  value: unknown,
): RemovalNotice {
  if (value === null)
    return {
      kind: 'absent',
      message:
        'No retained removal receipt was returned. The current removal request remains unconfirmed; refresh Recordings to inspect its stored state.',
    };
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    !('version' in value) ||
    value.version !== 1 ||
    !('run_id' in value) ||
    value.run_id !== runId ||
    !('completed' in value) ||
    typeof value.completed !== 'boolean' ||
    !('last_error' in value) ||
    (value.last_error !== null && typeof value.last_error !== 'string')
  )
    return {
      kind: 'unknown',
      message:
        'The retained receipt is incomplete or uses a format this UI cannot interpret. The current removal request remains unconfirmed; inspect the receipt through runs.removal before retrying.',
    };
  if (value.completed)
    return {
      kind: 'completed',
      message:
        'The retained v1 receipt reports cleanup completed for the original removal. The current removal request remains unconfirmed: a historical receipt does not confirm the identity or state of a recording now using this ID.',
    };
  return {
    kind: 'pending',
    message:
      'The retained v1 receipt reports cleanup pending for the original removal. Retry the same removal request; the service rechecks ownership, path identities and shared references.',
    lastError: value.last_error ?? undefined,
  };
}

/** The caller supplies only a read; failure never replaces the mutation error. */
export async function inspectRemovalReceipt(
  runId: string,
  read: () => Promise<unknown>,
): Promise<RemovalNotice> {
  try {
    return removalReceiptNotice(runId, await read());
  } catch (error) {
    return {
      kind: 'unavailable',
      message: `Retained cleanup receipt unavailable. ${queryFailureMessage(error)} The original removal failure still needs confirmation from the stored recording state.`,
    };
  }
}
