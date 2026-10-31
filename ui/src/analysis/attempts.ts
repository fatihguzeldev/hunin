import { operation, type AnalysisResult } from '../api';
import { badge, button, el, empty, errorView } from '../dom';
import { date } from '../format';
import type { ViewContext } from '../investigation/view';
import { analysisAttemptMetadata } from './metadata';
import { recordingUrl } from '../projects/context';

export function analyzerAttempts(
  items: AnalysisResult[],
  context: ViewContext,
  feedback: HTMLElement,
  refreshAttempts: () => void,
): HTMLElement {
  const published = el(
    'section',
    { class: 'panel' },
    el('div', { class: 'panel-head' }, el('h2', {}, 'Results')),
  );
  if (!items.length)
    published.append(
      empty(
        'No analyzer results',
        'Run a registered analyzer on this recording to see its results here.',
      ),
    );
  for (const item of items) {
    const result = analysisAttemptMetadata(item);
    const executionId = result.id;
    published.append(
      el(
        'div',
        { class: 'result-item' },
        el(
          'div',
          { class: 'toolbar' },
          el(
            'h3',
            { title: result.id },
            result.created_at
              ? date(result.created_at)
              : 'Attempt time unknown',
          ),
          badge(
            result.status,
            ['accepted', 'replayed'].includes(result.status)
              ? 'good'
              : ['failed', 'timed_out'].includes(result.status)
                ? 'bad'
                : 'warn',
          ),
          result.selected
            ? badge(
                result.result_set_id || result.id
                  ? 'selected result'
                  : 'selected flag · identity unavailable',
                result.result_set_id || result.id ? 'good' : 'warn',
              )
            : result.selected === false &&
                ['accepted', 'replayed'].includes(result.status)
              ? badge('historical')
              : null,
          result.status === 'running' && executionId
            ? button(
                'Cancel attempt',
                () => {
                  void operation('analyses.cancel', {
                    execution_id: executionId,
                  })
                    .then(() =>
                      context.refresh(
                        'Cancellation requested. Check the attempt status below.',
                      ),
                    )
                    .catch((error) =>
                      feedback.replaceChildren(
                        errorView(error, () => {
                          refreshAttempts();
                        }),
                      ),
                    );
                },
                'button small danger',
              )
            : null,
        ),
        el(
          'p',
          {},
          `Analyzer ${result.analysis_id?.slice(0, 12) ?? 'unknown'} · version ${result.version ?? 'unknown'}`,
        ),
        result.selected === true &&
          (result.result_set_id || result.id) &&
          ['accepted', 'replayed'].includes(result.status)
          ? el(
              'a',
              {
                href: recordingUrl(location.href, context.run.id, 'evidence')
                  .href,
              },
              'Inspect accepted evidence',
            )
          : null,
        ...result.diagnostics.map((message) => el('p', {}, message)),
        el(
          'details',
          {},
          el('summary', {}, 'Attempt details'),
          el('pre', {}, JSON.stringify(result.raw, null, 2)),
        ),
      ),
    );
  }
  return published;
}
