import {
  isHttpFailure,
  isOperationFailure,
  type OperationFailure,
  isHttpSuccess,
  validateOperationOutput,
  type OperationInputs,
  type OperationOutputs,
  type Run as RunWire,
  type Summary as SummaryWire,
} from './operations.generated.js';

export type Run = RunWire;
export type Summary = SummaryWire;
export type Capability = OperationOutputs['capabilities']['runtimes'][number];
export type Hotspot = OperationOutputs['profiles.hotspots']['hotspots'][number];
export type TreeNode = OperationOutputs['profiles.tree']['roots'][number];
export type Artifact = OperationOutputs['artifacts.list']['artifacts'][number];
export type Analysis = OperationOutputs['analyses.list']['analyses'][number];
export type AnalysisResult =
  OperationOutputs['analyses.results']['results'][number];
export type Series = OperationOutputs['resources.query']['series'][number];
export type Comparison = OperationOutputs['comparisons.create'];
export type Scenario = OperationOutputs['scenarios.list']['scenarios'][number];

export type Operations = {
  [K in keyof OperationInputs]: {
    input: OperationInputs[K];
    output: OperationOutputs[K];
  };
};

export class ApiError extends Error {
  readonly operationFailure?: OperationFailure;
  constructor(
    public code: string,
    message: string,
    public details?: unknown,
  ) {
    super(message);
    this.name = 'ApiError';
    if (
      details &&
      typeof details === 'object' &&
      'operation_error' in details &&
      isOperationFailure(details.operation_error)
    )
      this.operationFailure = details.operation_error;
  }
}

export async function operation<K extends keyof Operations>(
  name: K,
  params: Operations[K]['input'],
  signal?: AbortSignal,
): Promise<Operations[K]['output']> {
  const response = await fetch('/api/operations', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    credentials: 'same-origin',
    body: JSON.stringify({ operation: name, params }),
    signal,
  });
  const envelope: unknown = await response.json().catch(() => {
    throw new Error(
      'Hunin returned an invalid response. Check that the local service is running.',
    );
  });
  if (!envelope || typeof envelope !== 'object')
    throw new Error('Hunin returned an invalid response envelope.');
  if ('error' in envelope) {
    if (!isHttpFailure(envelope))
      throw new Error('Hunin returned an invalid error envelope.');
    throw new ApiError(
      envelope.error.code,
      envelope.error.message,
      envelope.error.details,
    );
  }
  if (!response.ok || !isHttpSuccess(envelope))
    throw new Error(`The local service returned HTTP ${response.status}.`);
  validateOperationOutput(name, envelope.result);
  return envelope.result as Operations[K]['output'];
}
