import type { Observation } from './record.generated';
import { isHttpFailure, isHttpSuccess, validateOperationOutput, type OperationInputs, type OperationOutputs, type Run as RunWire, type Summary as SummaryWire } from './operations.generated.js';

export type Run = RunWire;
export type Summary = SummaryWire;
export type Capability = OperationOutputs['capabilities']['runtimes'][number];
export type Hotspot = { function: string; file?: string; line?: number; code_id?: string | null; self_weight: number; inclusive_weight: number; weight_unit: string };
export type TreeNode = { id: string; name: string; file?: string; line?: number; code_id?: string | null; self_weight: number; total_weight: number; child_count?: number; children_truncated?: boolean; depth_truncated?: boolean; children: TreeNode[] };
export type Artifact = OperationOutputs['artifacts.list']['artifacts'][number];
export type Analysis = OperationOutputs['analyses.list']['analyses'][number];
export type AnalysisResult = OperationOutputs['analyses.results']['results'][number];
export type Series = { id: string; name: string; unit: string; scope: string; pid: number; tid?: string | null; producer_id?: string | null; context_id?: string | null; thread_id_source?: string | null; clock?: string | null; clock_alignment?: string | null; process_started_at_seconds?: number | null; process_role?: string | null; point_count?: number; returned_points?: number; truncated?: boolean; points: { id: string; timestamp_us: number; value: number }[] };
export type Comparison = { before: Run; after: Run; comparable: boolean; reasons: string[]; metrics: { name: string; unit: string; before: number; after: number; delta: number | null; percent_change?: number | null; comparable?: boolean; measurement_quality?: string; diagnostic?: string; before_observations?: number; after_observations?: number }[]; correctness: { before: unknown; after: unknown }; hotspots: unknown; limitations?: string[] };
export type Scenario = OperationOutputs['scenarios.list']['scenarios'][number];

type Responses = {
  'profiles.hotspots': { hotspots: Hotspot[]; total_weight: number; weight_unit: string };
  'profiles.tree': { roots: TreeNode[]; weight_unit: string; next_cursor?: string | null; returned_nodes?: number; node_limit?: number; depth_limit?: number; diagnostics?: string[] };
  'timeline.query': { records: Observation[]; unaligned_records?: Observation[]; next_cursor?: string; unaligned_count?: number; bounds: { start_us: number; end_us: number } | null };
  'resources.query': { series: Series[]; next_cursor?: string; returned_points?: number; total_points?: number };
  'contexts.query': { contexts: Observation[]; relations: Observation[]; next_cursor?: string };
  'evidence.get': { records: Observation[] };
  'comparisons.create': Comparison;
} & OperationOutputs;

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
    if (!isHttpFailure(envelope)) throw new Error('Hunin returned an invalid error envelope.');
    throw new ApiError(envelope.error.code, envelope.error.message, envelope.error.details);
  }
  if (!response.ok || !isHttpSuccess(envelope)) throw new Error(`The local service returned HTTP ${response.status}.`);
  validateOperationOutput(name, envelope.result);
  return envelope.result as Operations[K]['output'];
}
