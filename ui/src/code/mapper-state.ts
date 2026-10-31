import type { CodePlan, OperationInputs } from '../operations.generated.js';

type Availability =
  | {
      status: 'available';
      reason: string;
      params: OperationInputs['analyses.run'];
    }
  | { status: 'blocked' | 'unassessed'; reason: string };

/** Package registration does not authorize a mapper for a particular recording. */
export function mapperAvailability(
  plan: CodePlan | undefined,
  analysisId: string,
  runId: string,
): Availability {
  const step = plan?.steps.find((value) => value.id === `mapper:${analysisId}`);
  if (!step)
    return {
      status: 'unassessed',
      reason: plan?.truncated
        ? 'This mapper was not assessed in the returned plan. The plan reached its limit.'
        : 'Applicability has not been assessed for this mapper and recording.',
    };
  if (step.status === 'blocked')
    return { status: 'blocked', reason: step.reason };
  if (
    step.operation !== 'analyses.run' ||
    step.params.run_id !== runId ||
    step.params.analysis_id !== analysisId
  )
    return {
      status: 'unassessed',
      reason: 'The returned plan has no matching action for this recording.',
    };
  return {
    status: 'available',
    reason: step.reason,
    params: {
      run_id: step.params.run_id,
      analysis_id: step.params.analysis_id,
    },
  };
}
