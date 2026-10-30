import { el } from '../dom';
import { collectionMode } from './collection-mode';
import { workloadFields } from './expectations';

/** One workload recipe supplies recording and saved-scenario controls. */
export function recipeFields(
  command: HTMLTextAreaElement,
  signal: AbortSignal,
) {
  const mode = collectionMode(command, signal);
  const timeout = el('input', {
    type: 'number',
    min: 1,
    max: 86400,
    value: 60,
  });
  const expectations = workloadFields();
  return { mode, timeout, expectations };
}
