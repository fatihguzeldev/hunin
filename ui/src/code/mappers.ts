import { operation } from '../api';
import { badge, button, el, errorView, loading } from '../dom';
import type { ViewContext } from '../investigation/view';
import type { CodePlan, OperationInputs } from '../operations.generated';
import { analysisMetadata } from '../analysis/metadata';
import { mapperAvailability } from './mapper-state';

export function mapperActions(
  context: ViewContext,
  anchor: OperationInputs['code.inspect'],
  signal: AbortSignal,
): HTMLElement {
  const list = el('select', { 'aria-label': 'Registered code mapper' });
  const availability = el('div', { class: 'stack', role: 'status' });
  const feedback = el('div', { class: 'stack', role: 'status' });
  const choose = button(
    'Check mapper packages',
    () => {
      void load();
    },
    'button subtle',
  );
  const apply = button(
    'Apply mapper',
    () => {
      void run();
    },
    'button primary',
  );
  const root = el(
    'section',
    { class: 'stack' },
    el('h3', {}, 'Source-aware mapper'),
    choose,
    list,
    availability,
    apply,
    feedback,
  );
  let cursor: string | undefined;
  let loadingPackages = false;
  let running = false;
  let plan: CodePlan | undefined;
  const known = new Map<string, ReturnType<typeof analysisMetadata>>();
  function sync(): void {
    const selected = mapperAvailability(plan, list.value, context.run.id);
    choose.disabled = loadingPackages || running;
    apply.disabled =
      loadingPackages ||
      running ||
      !known.has(list.value) ||
      selected.status !== 'available';
    list.disabled = loadingPackages || running || !known.size;
    availability.replaceChildren(
      ...(known.has(list.value)
        ? [
            badge(
              selected.status === 'unassessed'
                ? 'Not assessed'
                : selected.status,
            ),
            el('p', { class: 'metric-note' }, selected.reason),
          ]
        : [
            el(
              'p',
              { class: 'metric-note' },
              'Load mapper packages to check their applicability to this recording.',
            ),
          ]),
    );
  }
  async function load(): Promise<void> {
    if (signal.aborted || loadingPackages || running) return;
    loadingPackages = true;
    plan = undefined;
    sync();
    feedback.replaceChildren(
      loading('Checking registered mappers and input requirements…'),
    );
    try {
      const [packages, assessed] = await Promise.allSettled([
        operation('analyses.list', { cursor, limit: 25 }, signal),
        operation(
          'code.plan',
          { ...anchor, goal: 'semantic_origin', limit: 500 },
          signal,
        ),
      ]);
      if (signal.aborted) return;
      const failures: unknown[] = [];
      if (packages.status === 'fulfilled') {
        const page = packages.value;
        for (const value of page.analyses) {
          const metadata = analysisMetadata(value);
          const raw = metadata.raw;
          const contract =
            raw && typeof raw === 'object' && 'output_contract' in raw
              ? raw.output_contract
              : undefined;
          if (
            contract &&
            typeof contract === 'object' &&
            'name' in contract &&
            contract.name === 'code-mapping' &&
            'version' in contract &&
            contract.version === 1 &&
            typeof metadata.id === 'string'
          )
            known.set(metadata.id, metadata);
        }
        const previous = list.value;
        list.replaceChildren(
          ...[...known].map(([id, value]) =>
            el(
              'option',
              { value: id },
              typeof value.name === 'string' ? value.name : id.slice(0, 12),
            ),
          ),
        );
        if (known.has(previous)) list.value = previous;
        cursor = page.next_cursor ?? undefined;
        choose.textContent = cursor
          ? 'More mapper packages'
          : 'Refresh mapper packages';
      } else failures.push(packages.reason);
      if (assessed.status === 'fulfilled') plan = assessed.value;
      else failures.push(assessed.reason);
      feedback.replaceChildren(
        ...failures.map((error) =>
          errorView(error, () => {
            void load();
          }),
        ),
      );
      if (!known.size && packages.status === 'fulfilled')
        feedback.append(
          el(
            'p',
            { class: 'metric-note' },
            cursor
              ? 'No code mapper on this package page. Load more packages.'
              : 'Register a versioned code mapper in Analyzers to apply source-aware mappings.',
          ),
        );
    } finally {
      loadingPackages = false;
      sync();
    }
  }
  async function run(): Promise<void> {
    const selected = mapperAvailability(plan, list.value, context.run.id);
    if (
      signal.aborted ||
      running ||
      loadingPackages ||
      !known.has(list.value) ||
      selected.status !== 'available'
    )
      return;
    running = true;
    sync();
    feedback.replaceChildren(loading('Starting mapper…'));
    try {
      const result = await operation('analyses.run', selected.params);
      if (result.status === 'running')
        context.followAttempt(result.result_set_id);
      if (signal.aborted) return;
      feedback.replaceChildren(
        el(
          'p',
          { class: 'notice' },
          result.status === 'running'
            ? 'Mapper started. Its progress is available in Analyzers.'
            : `Mapper ${result.status}.`,
        ),
      );
      if (result.status !== 'running')
        context.refresh(`Mapper ${result.status}.`);
    } catch (error) {
      if (!signal.aborted)
        feedback.replaceChildren(
          errorView(error, () => {
            void run();
          }),
        );
    } finally {
      running = false;
      sync();
    }
  }
  list.addEventListener('change', sync);
  sync();
  return root;
}
