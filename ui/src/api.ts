import type { Observation } from './record.generated';
import type { OperationInputs } from './operations.generated';

export type Run = {
  id: string; name: string; status: 'running' | 'completed' | 'failed' | 'canceled';
  command: string[]; cwd: string; runtime: string; started_at_us: number;
  finished_at_us?: number; exit_code?: number; diagnostics: string[];
  source: { revision?: string; dirty?: boolean; fingerprint: string };
  correctness: { passed: boolean | null; expected_exit_code?: number; actual_exit_code?: number | null; reason?: string };
};
export type Summary = { sample_count: number; total_weight: number; weight_unit: string; span_count: number; metric_count: number; relation_count: number; diagnostics: string[] };
export type Capability = { runtime: string; available: boolean; version?: string; capabilities: string[]; diagnostics: string[] };
export type Hotspot = { function: string; file?: string; line?: number; self_weight: number; inclusive_weight: number; weight_unit: string };
export type TreeNode = { name: string; file?: string; line?: number; self_weight: number; total_weight: number; children: TreeNode[] };
export type Artifact = { id: string; name: string; size: number; sha256: string; format: string };
export type Analysis = { id: string; name: string; version: string; command: string[]; requirements: unknown; status?: string };
export type AnalysisResult = { id: string; analysis_id: string; version: string; status: string; diagnostics: string[]; created_at_us: number; input_fingerprint: string };
export type Series = { name: string; unit: string; scope: string; points: { timestamp_us: number; value: number; pid: number }[] };
export type Comparison = { before: Run; after: Run; comparable: boolean; reasons: string[]; metrics: { name: string; unit: string; before: number; after: number; delta: number; percent_change?: number | null }[]; correctness: { before: unknown; after: unknown }; hotspots: unknown; limitations?: string[] };
export type Scenario = { id: string; name: string; command: string[]; cwd: string; runtime: string; timeout_seconds: number; expected_exit_code: number };

type Responses = {
  capabilities: { host: { os: string; arch: string }; runtimes: Capability[]; schema_version: number };
  'runs.list': { runs: Run[]; next_cursor?: string };
  'runs.get': { run: Run; summary: Summary };
  'runs.start': { run: Run };
  'runs.cancel': { run: Run };
  'profiles.hotspots': { hotspots: Hotspot[]; total_weight: number; weight_unit: string };
  'profiles.tree': { roots: TreeNode[]; weight_unit: string };
  'timeline.query': { records: Observation[]; next_cursor?: string; unaligned_count?: number; bounds: { start_us: number; end_us: number } };
  'resources.query': { series: Series[]; next_cursor?: string };
  'contexts.query': { contexts: Observation[]; relations: Observation[]; next_cursor?: string };
  'evidence.get': { records: Observation[] };
  'artifacts.list': { artifacts: Artifact[] };
  'source.read': { path: string; content: string; start_line: number; identity_verified: boolean };
  'analyses.list': { analyses: Analysis[] };
  'analyses.register': { analysis: Analysis };
  'analyses.results': { results: AnalysisResult[] };
  'analyses.run': { result_set_id: string; status: string; diagnostics: string[] };
  'comparisons.create': Comparison;
  'scenarios.list': { scenarios: Scenario[] };
  'scenarios.save': { scenario: Scenario };
  'scenarios.run': { run: Run };
};

export type Operations = { [K in keyof Responses]: { input: OperationInputs[K]; output: Responses[K] } };

export class ApiError extends Error {
  constructor(public code: string, message: string, public details?: unknown) {
    super(message);
    this.name = 'ApiError';
  }
}

export async function operation<K extends keyof Operations>(
  name: K,
  params: Operations[K]['input'],
  signal?: AbortSignal,
): Promise<Operations[K]['output']> {
  const response = await fetch('/api/operations', {
    method: 'POST', headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin', body: JSON.stringify({ operation: name, params }), signal,
  });
  const envelope: unknown = await response.json().catch(() => { throw new Error('Hunin returned an invalid response. Check that the local service is running.'); });
  if (!envelope || typeof envelope !== 'object') throw new Error('Hunin returned an invalid response envelope.');
  if ('error' in envelope) {
    if (!envelope.error || typeof envelope.error !== 'object') throw new Error('Hunin returned an invalid error envelope.');
    const error = envelope.error as { code?: string; message?: string; details?: unknown };
    throw new ApiError(error.code ?? 'request_failed', error.message ?? 'The operation failed.', error.details);
  }
  if (!response.ok || !('result' in envelope)) throw new Error(`The local service returned HTTP ${response.status}.`);
  return envelope.result as Operations[K]['output'];
}
