import type {
  OperationInputs,
  OperationOutputs,
} from '../operations.generated';
import { functionSelection } from '../call-counts';
import { badge, button, el, field, lockFields } from '../dom';
import type { ViewContext } from '../investigation/view';
type CountParams = OperationInputs['calls.measure'];

/** Own editable counting selection and explicit rebuild arguments. */
export function countForm(
  context: ViewContext,
  actions: { inspect: () => void; sourceRunId: () => string },
) {
  const state = context.investigation.calls;
  const scope = el(
    'select',
    { 'aria-label': 'Functions to count' },
    el('option', { value: 'all' }, 'All functions'),
    el('option', { value: 'selected' }, 'Selected functions'),
  );
  scope.value = state.functions.length ? 'selected' : 'all';
  const selectors = el('div', { class: 'stack' });
  const rows: {
    name: HTMLInputElement;
    file: HTMLInputElement;
    line: HTMLInputElement;
    element: HTMLElement;
  }[] = [];
  function addSelector(value = { name: '', file: '', line: '' }): void {
    const name = el('input', {
      value: value.name,
      placeholder: 'Function name',
      required: true,
    });
    const file = el('input', {
      value: value.file,
      placeholder: 'Optional exact source path',
    });
    const line = el('input', {
      value: value.line,
      type: 'number',
      min: 1,
      step: 1,
    });
    const element = el(
      'div',
      { class: 'call-selector' },
      field('Function name', name),
      field('Source file', file),
      field('Declaration line', line),
    );
    const row = { name, file, line, element };
    element.append(
      button(
        'Remove',
        () => {
          rows.splice(rows.indexOf(row), 1);
          element.remove();
        },
        'button subtle small',
      ),
    );
    rows.push(row);
    selectors.append(element);
  }
  for (const value of state.functions)
    addSelector({
      name: value.name,
      file: value.file ?? '',
      line: value.line?.toString() ?? '',
    });
  if (!rows.length) addSelector();
  const selected = el(
    'div',
    { class: 'stack' },
    selectors,
    button('Add function', () => addSelector(), 'button subtle small'),
    el(
      'p',
      { class: 'metric-note' },
      'Names can identify several functions. Add the source path and declaration line when needed; the counting method reports any unresolved selection.',
    ),
  );
  const rebuild = el('input', { type: 'checkbox' });
  const executable = el('input', { placeholder: 'cc' });
  const argumentsInput = el('textarea', {
    rows: 4,
    placeholder: '-O2\nmain.c\n-o\n{output}',
  });
  const buildFields = el(
    'div',
    { class: 'stack hidden' },
    field('Build executable', executable),
    field('Build arguments · one argument per line', argumentsInput),
    el(
      'p',
      { class: 'metric-note' },
      'Use {output} for Hunin’s separate counting binary. Arguments are passed directly; shell syntax is not evaluated.',
    ),
  );
  rebuild.addEventListener('change', () => {
    buildFields.classList.toggle('hidden', !rebuild.checked);
  });
  const planBody = el('div', { role: 'status' });
  const feedback = el('div', { role: 'status' });
  let unlock: (() => void) | undefined;
  function parameters(): CountParams {
    const functions =
      scope.value === 'all'
        ? []
        : rows.map((row) =>
            functionSelection(row.name.value, row.file.value, row.line.value),
          );
    if (scope.value === 'selected' && !functions.length)
      throw new Error('Add at least one selected function.');
    state.functions = functions;
    context.persist();
    const params: CountParams = {
      run_id: actions.sourceRunId(),
      functions,
      allow_rebuild: rebuild.checked,
    };
    if (rebuild.checked && executable.value.trim())
      params.build_command = [
        executable.value.trim(),
        ...argumentsInput.value
          .split(/\r?\n/)
          .filter((value) => value.length > 0),
      ];
    return params;
  }
  const check = button(
    'Check method',
    () => {
      actions.inspect();
    },
    'button subtle',
  );
  const measure = el(
    'button',
    { type: 'submit', class: 'button primary' },
    'Measure function calls',
  );
  const form = el(
    'form',
    { class: 'panel panel-body stack' },
    field('Functions to count', scope),
    selected,
    el('label', { class: 'toolbar' }, rebuild, 'Allow a counting rebuild'),
    buildFields,
    el('div', { class: 'toolbar' }, measure, check),
    planBody,
    feedback,
  );
  function syncScope(): void {
    selected.classList.toggle('hidden', scope.value !== 'selected');
    for (const row of rows) row.name.required = scope.value === 'selected';
  }
  scope.addEventListener('change', syncScope);
  syncScope();
  function renderPlan(plan: OperationOutputs['calls.plan']['plan']): void {
    planBody.replaceChildren(
      el(
        'div',
        { class: 'toolbar' },
        badge(plan.status),
        el('span', {}, plan.method ?? 'No eligible method'),
      ),
      ...(plan.reason ? [el('p', {}, plan.reason)] : []),
      el(
        'p',
        { class: 'metric-note' },
        plan.requires_rebuild
          ? 'A separate counting build is required.'
          : 'Uses a separate counting execution.',
      ),
      el(
        'details',
        {},
        el('summary', {}, 'Method scope'),
        el(
          'div',
          { class: 'stack' },
          el(
            'p',
            {},
            `Requested: ${plan.requested_scope}. Collection: ${plan.collection_scope.replaceAll('_', ' ')}.`,
          ),
          ...plan.limitations.map((text) =>
            el('p', { class: 'metric-note' }, text),
          ),
        ),
      ),
    );
  }
  function setBusy(value: boolean): void {
    measure.disabled = value;
    check.disabled = value;
    if (value && !unlock) unlock = lockFields(form);
    if (!value) {
      unlock?.();
      unlock = undefined;
    }
  }
  return {
    element: form,
    parameters,
    renderPlan,
    setBusy,
    planBody,
    feedback,
    measure,
  };
}
