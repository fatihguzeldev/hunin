import { button, el } from '../dom.js';
import type { AnalysisWatch } from './watch.js';

export function attemptActivity(
  watch: AnalysisWatch,
  runId: string,
  actions: { recheck: () => void; open: () => void },
): HTMLElement | undefined {
  const attempts = watch.forRun(runId);
  if (!attempts.length) return;
  return el(
    'section',
    { class: 'notice stack', role: 'status' },
    el(
      'p',
      {},
      'Known analyzer attempts may change selected results. Their retained status is checked independently of the open recording view.',
    ),
    ...attempts.map((attempt) =>
      el(
        'p',
        { class: 'mono' },
        `${attempt.id} · ${attempt.status ?? 'status not yet confirmed'}${attempt.diagnostic ? ` · ${attempt.diagnostic}` : ''}`,
      ),
    ),
    !watch.persistent
      ? el(
          'p',
          {},
          'Attempt tracking is remembered on this page only because browser session storage is unavailable.',
        )
      : null,
    el(
      'div',
      { class: 'toolbar' },
      button('Recheck attempt status', actions.recheck, 'button small'),
      button('Open Analyzers', actions.open, 'button small'),
    ),
  );
}
