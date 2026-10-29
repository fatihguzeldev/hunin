import { operation, type Comparison } from '../api';
import { comparisonResults } from './comparison-results';
import { linkedRequest } from '../requests';
import { recordingHistory } from '../recording-history';
import { button, el, empty, errorView, field, loading } from '../dom';
import type { ViewContext } from '../workspace';

export async function comparison(context: ViewContext): Promise<HTMLElement> {
  if (context.runs.length < 2)
    return empty(
      'Record the same workload again',
      'Comparison needs at least two recordings. Run your workload after a change, then select a baseline here.',
    );
  const state = context.investigation.comparison;
  state.after ??= context.run.id;
  state.before ??= context.runs.find((run) => run.id !== state.after)?.id;
  const known = new Map(context.runs.map((run) => [run.id, run]));
  const before = el('select');
  const after = el('select');
  const unit = el('select');
  unit.append(
    el('option', { value: '' }, 'Automatic sample unit · service default'),
  );
  if (state.unit) unit.append(el('option', { value: state.unit }, state.unit));
  unit.value = state.unit ?? '';
  const unitInfo = el('p', { class: 'metric-note', role: 'status' });
  const content = el('div');
  let request: AbortController | undefined;
  let epoch = 0;
  const compare = button(
    'Compare recordings',
    () => {
      void load();
    },
    'button primary',
  );
  function populate(): void {
    const allowed = new Set([
      ...context.runs.map((run) => run.id),
      state.before,
      state.after,
    ]);
    for (const id of known.keys()) if (!allowed.has(id)) known.delete(id);
    for (const select of [before, after]) {
      select.replaceChildren(
        ...[...known.values()].map((run) =>
          el(
            'option',
            { value: run.id },
            `${run.name} · ${run.id.slice(0, 8)}`,
          ),
        ),
      );
      const selected = select === before ? state.before : state.after;
      if (selected && !known.has(selected))
        select.append(
          el(
            'option',
            { value: selected },
            `${selected} · metadata not loaded`,
          ),
        );
      select.value = selected ?? '';
    }
  }
  function changed(): void {
    state.before = before.value;
    state.after = after.value;
    state.unit = unit.value || undefined;
    epoch++;
    request?.abort();
    compare.disabled = false;
    context.persist();
    content.replaceChildren(
      empty(
        'Comparison selection changed',
        'Compare these recordings to read their current selected results and limitations.',
      ),
    );
  }
  populate();
  before.addEventListener('change', changed);
  after.addEventListener('change', changed);
  unit.addEventListener('change', changed);
  function populateUnits(data: Comparison): void {
    const available = new Set([
      ...data.sample_weight_units.before,
      ...data.sample_weight_units.after,
      ...(state.unit ? [state.unit] : []),
    ]);
    unit.replaceChildren(
      el(
        'option',
        { value: '' },
        `Automatic sample unit · ${data.hotspot_selection?.weight_unit ?? 'service default'}`,
      ),
      ...[...available].sort().map((value) => el('option', { value }, value)),
    );
    unit.value = state.unit ?? '';
    unit.disabled = data.hotspot_selection === undefined;
    unitInfo.textContent = data.hotspot_selection
      ? 'Choose the sampled-weight unit to compare. Other units are separate measurements; sample weights do not establish elapsed time or a speed improvement.'
      : 'This service did not provide explicit sample-unit selection and coverage metadata. Its default comparison remains visible.';
  }
  const historyPicker = recordingHistory(context.signal, (run, side) => {
    known.set(run.id, run);
    state[side] = run.id;
    populate();
    changed();
  });
  const body = el(
    'div',
    { class: 'stack' },
    el(
      'div',
      { class: 'toolbar' },
      field('Before · baseline', before),
      field('After · candidate', after),
      field('Sample weight unit', unit),
      compare,
    ),
    unitInfo,
    historyPicker,
    content,
  );
  async function load(): Promise<void> {
    if (context.signal.aborted) return;
    const pair = {
      before: before.value,
      after: after.value,
      weight_unit: unit.value || undefined,
    };
    if (!pair.before || !pair.after || pair.before === pair.after) {
      content.replaceChildren(
        empty(
          'Choose two different recordings',
          'A recording cannot serve as its own before/after comparison.',
        ),
      );
      return;
    }
    const version = ++epoch;
    request?.abort();
    request = new AbortController();
    const linked = linkedRequest(context.signal, request.signal);
    state.before = pair.before;
    state.after = pair.after;
    state.unit = pair.weight_unit;
    context.persist();
    compare.disabled = true;
    content.replaceChildren(loading('Assessing comparability…'));
    try {
      await Promise.all(
        [...new Set([pair.before, pair.after])]
          .filter((id) => !known.has(id))
          .map(async (id) => {
            const { run } = await operation(
              'runs.get',
              { run_id: id },
              linked.signal,
            );
            if (!linked.signal.aborted) known.set(run.id, run);
          }),
      );
      if (version !== epoch || linked.signal.aborted) return;
      populate();
      const data = await operation('comparisons.create', pair, linked.signal);
      if (
        version !== epoch ||
        linked.signal.aborted ||
        before.value !== pair.before ||
        after.value !== pair.after ||
        (unit.value || undefined) !== pair.weight_unit
      )
        return;
      populateUnits(data);
      content.replaceChildren(comparisonResults(data, context));
    } catch (error) {
      if (version === epoch && !linked.signal.aborted)
        content.replaceChildren(
          errorView(error, () => {
            void load();
          }),
        );
    } finally {
      linked.finish();
      if (version === epoch) compare.disabled = false;
    }
  }
  await load();
  return body;
}
