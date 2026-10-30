import { analysisAttemptMetadata } from './metadata';
import { analyzerPackages } from './packages';
import { analyzerAttempts } from './attempts';
import { registerAnalysis } from './registration';
import { runSource } from '../recordings/presentation';
import { operation, type Analysis, type AnalysisResult } from '../api';
import { button, el } from '../dom';
import type { ViewContext } from '../investigation/view';
import { pagedList } from '../paged-list';

export async function analyses(context: ViewContext): Promise<HTMLElement> {
  const source = runSource(context.run);
  const feedback = el('div', { role: 'status' });
  const packages = pagedList<Analysis>({
    label: 'analyzer packages',
    signal: context.signal,
    nextLabel: 'More analyzer packages',
    load: async (cursor, readSignal) => {
      const page = await operation(
        'analyses.list',
        { cursor, limit: 25 },
        readSignal,
      );
      return { items: page.analyses, next_cursor: page.next_cursor };
    },
    render: (items) =>
      analyzerPackages(items, context, feedback, () => {
        void attempts.refresh(true);
      }),
  });
  const attempts = pagedList<AnalysisResult>({
    label: 'analyzer attempts',
    signal: context.signal,
    nextLabel: 'Older attempts',
    load: async (cursor, readSignal) => {
      const page = await operation(
        'analyses.results',
        {
          run_id: context.run.id,
          cursor,
          limit: 25,
        },
        readSignal,
      );
      if (!readSignal.aborted)
        for (const row of page.results) {
          const attempt = analysisAttemptMetadata(row);
          if (attempt.id && attempt.status === 'running')
            context.followAttempt(attempt.id);
        }
      return { items: page.results, next_cursor: page.next_cursor };
    },
    render: (items) =>
      analyzerAttempts(items, context, feedback, () => {
        void attempts.refresh();
      }),
  });
  const body = el(
    'div',
    { class: 'stack' },
    el(
      'div',
      { class: 'view-title' },
      el('h2', {}, 'Durable analyzers'),
      el('p', {}, 'Managed packages · fresh inputs · atomic publication'),
    ),
    el(
      'p',
      { class: 'notice' },
      'Applicability checks constrain reuse. A passing declaration does not prove the semantic correctness of an analyzer’s conclusions.',
    ),
    el(
      'div',
      { class: 'toolbar' },
      button('Register analyzer', () =>
        registerAnalysis(context.run.runtime, source, () =>
          packages.refresh(true),
        ),
      ),
      button(
        'Refresh results',
        () => {
          void Promise.all([packages.refresh(), attempts.refresh()]);
        },
        'button subtle',
      ),
    ),
    feedback,
    packages.element,
    attempts.element,
  );
  await Promise.all([packages.refresh(), attempts.refresh()]);
  return body;
}
