import { operation, type AnalysisResult } from '../api';
import { badge, button, el, empty, errorView } from '../dom';
import { date } from '../format';
import type { ViewContext } from '../investigation/view';
import { analysisAttemptMetadata } from './metadata';

export function analyzerAttempts(
  items: AnalysisResult[],
  context: ViewContext,
  feedback: HTMLElement,
  refreshAttempts: () => void,
): HTMLElement {
  const published = el(
    'section',
    { class: 'panel' },
    el(
      'div',
      { class: 'panel-head' },
      el('h2', {}, 'Attempts and accepted results'),
      el('small', {}, 'Accepted observations join Timeline and Evidence'),
    ),
  );
  if (!items.length)
    published.append(
      empty(
        'No analyzer results',
        'Completed attempts and accepted result identities will be retained with the recording.',
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
          el('h3', { class: 'mono' }, result.id ?? 'Unknown attempt'),
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
                        'Analyzer cancellation request completed. The retained attempt status is authoritative.',
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
          `${result.analysis_id ?? 'Unknown analyzer'} · ${result.version ?? 'unknown version'} · ${result.created_at ? date(result.created_at) : 'creation time unknown'}`,
        ),
        result.result_set_id && ['accepted', 'replayed'].includes(result.status)
          ? el(
              'p',
              { class: 'mono source-label' },
              `Accepted result set ${result.result_set_id}`,
            )
          : null,
        el(
          'p',
          { class: 'mono source-label' },
          `Input fingerprint ${result.input_fingerprint ?? 'unknown'}`,
        ),
        ...result.diagnostics.map((message) => el('p', {}, message)),
        !result.created_at || !result.analysis_id || !result.input_fingerprint
          ? el(
              'details',
              {},
              el(
                'summary',
                {},
                'Retained attempt metadata · unavailable fields',
              ),
              el('pre', {}, JSON.stringify(result.raw, null, 2)),
            )
          : null,
      ),
    );
  }
  return published;
}
