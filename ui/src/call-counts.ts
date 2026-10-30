import type {
  OperationInputs,
  OperationOutputs,
} from './operations.generated.js';

export type FunctionSelection = NonNullable<
  OperationInputs['calls.plan']['functions']
>[number];
type CountParams = OperationInputs['calls.measure'];
type CountReport = OperationOutputs['calls.query'];

/** Preserve source identity supplied by the user; the backend resolves ambiguity. */
export function functionSelection(
  name: string,
  file = '',
  line = '',
): FunctionSelection {
  const selection: FunctionSelection = { name: name.trim() };
  if (!selection.name) throw new Error('Enter a function name.');
  if (file.trim()) selection.file = file.trim();
  if (line.trim()) {
    const number = Number(line);
    if (!Number.isSafeInteger(number) || number < 1)
      throw new Error('Declaration line must be a positive integer.');
    selection.line = number;
  }
  return selection;
}

export function countLabel(calls: number | null | undefined): string {
  return calls == null ? 'Not observed' : calls.toLocaleString('en-US');
}

export function countStatus(status: string): string {
  const labels: Record<string, string> = {
    not_collected: 'Not collected',
    blocked: 'Blocked',
    running: 'Running',
    complete: 'Complete',
    incomplete: 'Incomplete',
    failed: 'Failed',
    canceled: 'Canceled',
  };
  return labels[status] ?? status;
}

/** Retrying uncertain acceptance keeps the same job identity; a new accepted action does not. */
export class CountAttempt {
  private current: { fingerprint: string; params: CountParams } | undefined;
  constructor(private token: () => string = () => crypto.randomUUID()) {}
  request(params: CountParams): CountParams {
    const fingerprint = JSON.stringify(params);
    if (!this.current || this.current.fingerprint !== fingerprint)
      this.current = {
        fingerprint,
        params: { ...params, request_id: this.token() },
      };
    return this.current.params;
  }
  accepted(): void {
    this.current = undefined;
  }
}

/** A page can extend only the same retained counting execution and scope. */
export function appendCountPage(
  previous: CountReport,
  next: CountReport,
): CountReport {
  if (
    previous.run_id !== next.run_id ||
    previous.parent_run_id !== next.parent_run_id ||
    previous.method !== next.method ||
    previous.requested_scope !== next.requested_scope ||
    previous.collection_scope !== next.collection_scope
  )
    throw new Error(
      'The counting recording or scope changed while paging. Refresh to inspect it separately.',
    );
  return { ...next, counts: [...previous.counts, ...next.counts] };
}

/** Admission and its exact child stay busy independently of unrelated report refreshes. */
export class CountActivity {
  private admitting = false;
  private child: string | undefined;
  get busy(): boolean {
    return this.admitting || this.child !== undefined;
  }
  get runId(): string | undefined {
    return this.child;
  }
  begin(): boolean {
    if (this.busy) return false;
    this.admitting = true;
    return true;
  }
  accepted(runId?: string): void {
    this.admitting = false;
    this.child = runId;
  }
  rejected(): void {
    this.admitting = false;
  }
  observed(runId: string, status: CountReport['status']): void {
    if (this.admitting || (this.child && this.child !== runId)) return;
    this.child = status === 'running' ? runId : undefined;
  }
}
