import { operation, type Analysis } from '../api';
import { badge, button, el, empty, errorView, loading } from '../dom';
import type { ViewContext } from '../investigation/view';
import { analysisMetadata } from './metadata';

export function analyzerPackages(
  items: Analysis[],
  context: ViewContext,
  feedback: HTMLElement,
  refreshAttempts: () => void,
): HTMLElement {
  const list = el(
    'section',
    { class: 'panel' },
    el('div', { class: 'panel-head' }, el('h2', {}, 'Registered packages')),
  );
  if (!items.length)
    list.append(
      empty(
        'No registered analyzers',
        'Register a local analyzer to produce derived observations from this recording.',
      ),
    );
  for (const item of items) {
    const analysis = analysisMetadata(item);
    const analysisId = analysis.id;
    const run = button(
      'Run on this recording',
      () => {
        if (!analysisId) return;
        run.disabled = true;
        feedback.replaceChildren(
          loading('Running analyzer against this recording…'),
        );
        void operation('analyses.run', {
          run_id: context.run.id,
          analysis_id: analysisId,
        })
          .then((result) => {
            if (result.status !== 'running') {
              context.refresh(
                `Analyzer attempt ${result.result_set_id}: ${result.status}.`,
              );
              return;
            }
            context.followAttempt(result.result_set_id);
            if (context.signal.aborted) return;
            feedback.replaceChildren(
              el(
                'p',
                { class: 'notice' },
                result.status === 'running'
                  ? 'Analyzer started. Follow its progress in Results below.'
                  : `${result.status} · ${result.result_set_id}`,
                ...result.diagnostics.map((value) =>
                  el('span', { class: 'source-label' }, value),
                ),
              ),
            );
            refreshAttempts();
          })
          .catch((error) => {
            if (context.signal.aborted) return;
            feedback.replaceChildren(errorView(error, () => run.click()));
          })
          .finally(() => {
            run.disabled = false;
          });
      },
      'button small',
    );
    run.disabled = !analysisId || !analysis.command?.length;
    list.append(
      el(
        'div',
        { class: 'result-item' },
        el(
          'div',
          { class: 'toolbar' },
          el('h3', {}, analysis.name),
          badge(analysis.version ?? 'unknown version'),
          el('span', { class: 'spacer' }),
          run,
        ),
        el(
          'p',
          { class: 'mono' },
          analysis.command?.length
            ? analysis.command.join(' ')
            : 'Command unavailable',
        ),
        el(
          'details',
          {},
          el(
            'summary',
            { class: 'metric-note' },
            'Applicability and dependencies',
          ),
          el(
            'pre',
            {},
            JSON.stringify(
              {
                requirements: analysis.requirements,
                dependencies: analysis.dependencies,
              },
              null,
              2,
            ),
          ),
        ),
        run.disabled
          ? el(
              'details',
              {},
              el(
                'summary',
                {},
                'Retained package metadata · unavailable fields',
              ),
              el('pre', {}, JSON.stringify(analysis.raw, null, 2)),
            )
          : null,
      ),
    );
  }
  return list;
}
