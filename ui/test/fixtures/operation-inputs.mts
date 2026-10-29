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
