import type { OperationInputs } from '../../src/operations.generated.js';

export const stringEnvironment = {
  command: ['workload'],
  env: { COUNT: '7' },
} satisfies OperationInputs['runs.start'];

export const inheritedEnvironment = {
  command: ['workload'],
} satisfies OperationInputs['runs.start'];

export const numericEnvironment = {
  command: ['workload'],
  env: {
    // @ts-expect-error Explicit environment override values must be strings.
    COUNT: 7,
  },
} satisfies OperationInputs['runs.start'];

export const exactAttemptLookup = {
  run_id: 'recording',
  execution_id: 'attempt',
} satisfies OperationInputs['analyses.results'];

export const malformedAttemptLookup = {
  run_id: 'recording',
  // @ts-expect-error Attempt handles are strings; they are not numeric page offsets.
  execution_id: 7,
} satisfies OperationInputs['analyses.results'];
