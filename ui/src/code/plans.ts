import { operation } from '../api';
import { badge, button, el, errorView, field, loading } from '../dom';
import type { CodePlan, OperationInputs } from '../operations.generated';

export function mappingPlans(
  params: () => OperationInputs['code.inspect'],
  signal: AbortSignal,
): HTMLElement {
  const goal = el(
    'select',
    {},
    el('option', { value: 'code_origin' }, 'Trace code origin'),
    el('option', { value: 'source' }, 'Find retained source'),
    el('option', { value: 'semantic_origin' }, 'Trace semantic origin'),
  );
  const result = el('div', { class: 'stack', role: 'status' });
  const check = button(
    'Plan next step',
    () => {
      void read();
    },
    'button subtle',
  );
  const root = el(
    'section',
    { class: 'stack' },
    el('h3', {}, 'Next step'),
    field('Investigation goal', goal),
    check,
    result,
  );
  async function read(): Promise<void> {
    if (signal.aborted) return;
    check.disabled = true;
    goal.disabled = true;
    result.replaceChildren(
      loading('Checking available evidence and capture options…'),
    );
    try {
      const plan: CodePlan = await operation(
        'code.plan',
        {
          ...params(),
          goal: goal.value as NonNullable<OperationInputs['code.plan']['goal']>,
        },
        signal,
      );
      if (signal.aborted) return;
      result.replaceChildren(
        ...plan.steps.map((step) =>
          el(
            'section',
            { class: 'panel panel-body stack' },
            badge(step.status),
            el('p', {}, step.reason),
            step.operation
              ? el(
                  'details',
                  {},
                  el('summary', {}, `${step.operation} · prepared parameters`),
                  el('pre', {}, JSON.stringify(step.params, null, 2)),
                )
              : null,
          ),
        ),
      );
      if (!plan.steps.length)
        result.append(
          el('p', {}, 'No further step was returned for this goal.'),
        );
      if (plan.truncated)
        result.append(
          el(
            'p',
            { class: 'notice' },
            'This plan reached a query limit. Additional mappings, mapper packages or steps may be omitted.',
          ),
        );
    } catch (error) {
      if (!signal.aborted)
        result.replaceChildren(
          errorView(error, () => {
            void read();
          }),
        );
    } finally {
      check.disabled = false;
      goal.disabled = false;
    }
  }
  return root;
}
