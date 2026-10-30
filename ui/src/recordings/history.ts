import { operation, type Run } from '../api';
import { badge, button, el } from '../dom';
import { pagedList } from '../paged-list';

/** Browse bounded history pages without loading the entire recording index into a select. */
export function recordingHistory(
  signal: AbortSignal,
  choose: (run: Run, side: 'before' | 'after') => void,
): HTMLElement {
  const list = pagedList<Run>({
    label: 'recording history',
    nextLabel: 'Older recordings',
    signal,
    load: async (cursor, readSignal) => {
      const page = await operation(
        'runs.list',
        { limit: 50, cursor },
        readSignal,
      );
      return { items: page.runs, next_cursor: page.next_cursor };
    },
    render: (runs) =>
      el(
        'div',
        { class: 'table-scroll' },
        el(
          'table',
          {},
          el(
            'thead',
            {},
            el(
              'tr',
              {},
              el('th', {}, 'Recording'),
              el('th', {}, 'Runtime / status'),
              el('th', {}, 'Comparison selection'),
            ),
          ),
          el(
            'tbody',
            {},
            ...runs.map((run) => {
              const before = button(
                'Use as baseline',
                () => choose(run, 'before'),
                'button small',
              );
              before.setAttribute(
                'aria-label',
                `Use recording ${run.id} as baseline`,
              );
              const after = button(
                'Use as candidate',
                () => choose(run, 'after'),
                'button small',
              );
              after.setAttribute(
                'aria-label',
                `Use recording ${run.id} as candidate`,
              );
              return el(
                'tr',
                {},
                el(
                  'td',
                  {},
                  run.name,
                  el('small', { class: 'source-label mono' }, run.id),
                ),
                el('td', {}, run.runtime, ' ', badge(run.status)),
                el('td', {}, el('div', { class: 'toolbar' }, before, after)),
              );
            }),
          ),
        ),
      ),
  });
  const details = el(
    'details',
    { class: 'panel', 'aria-label': 'Recording history' },
    el('summary', { class: 'panel-body' }, 'Choose from recording history'),
    el('div', { class: 'panel-body' }, list.element),
  );
  let loaded = false;
  details.addEventListener('toggle', () => {
    if (details.open && !loaded) {
      loaded = true;
      void list.refresh();
    }
  });
  return details;
}
