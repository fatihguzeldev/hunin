import type { OperationOutputs } from '../operations.generated';
import { countLabel, countStatus } from '../call-counts';
import { badge, button, el, empty, loading } from '../dom';
type CountReport = OperationOutputs['calls.query'];

function recordingLink(id: string, text: string, href: string): HTMLElement {
  return el('a', { href, title: id, 'aria-label': `${text}: ${id}` }, text);
}
export function countReport(
  data: CountReport,
  actions: {
    cancel: () => Promise<void>;
    refresh: () => void;
    more: (cursor: string) => void;
    recordingHref: (id: string) => string;
  },
): HTMLElement {
  const content = el(
    'section',
    { class: 'panel panel-body stack' },
    el(
      'div',
      { class: 'toolbar' },
      el('h2', {}, 'Function call counts'),
      badge(countStatus(data.status)),
    ),
    el(
      'div',
      {},
      recordingLink(
        data.run_id,
        data.parent_run_id ? 'Counting recording' : 'Recording',
        actions.recordingHref(data.run_id),
      ),
    ),
    data.parent_run_id
      ? el(
          'div',
          {},
          recordingLink(
            data.parent_run_id,
            'Original recording',
            actions.recordingHref(data.parent_run_id),
          ),
        )
      : null,
    el(
      'p',
      { class: 'metric-note' },
      data.parent_run_id
        ? 'Counts come from a separate execution, not the original sampled recording.'
        : data.status === 'not_collected'
          ? 'Counting requires a new execution. Stack samples do not measure call counts.'
          : 'Counts describe this recording’s execution.',
    ),
    el(
      'details',
      {},
      el('summary', {}, 'Collection method'),
      el(
        'div',
        { class: 'stack' },
        el('p', {}, data.method ?? 'Method not recorded'),
        el(
          'p',
          {},
          `Requested: ${data.requested_scope === 'all' ? 'all functions' : 'selected functions'}. Collection: ${data.collection_scope.replaceAll('_', ' ')}.`,
        ),
        ...data.limitations.map((text) =>
          el('p', { class: 'metric-note' }, text),
        ),
        el('p', { class: 'mono' }, `Counting recording ${data.run_id}`),
        data.parent_run_id
          ? el(
              'p',
              { class: 'mono' },
              `Original recording ${data.parent_run_id}`,
            )
          : null,
      ),
    ),
    ...data.diagnostics.map((text) => el('p', { class: 'notice' }, text)),
  );
  if (data.status === 'running') {
    const cancel = button('Cancel counting run', () => {
      cancel.disabled = true;
      void actions.cancel().catch(() => {
        cancel.disabled = false;
      });
    });
    content.append(loading('Counting in the separate recording…'), cancel);
  }
  if (data.counts.length) {
    content.append(
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
              el('th', {}, 'Function / source'),
              el('th', { class: 'numeric' }, 'Calls'),
              el('th', { class: 'numeric' }, 'Resumes'),
            ),
          ),
          el(
            'tbody',
            {},
            ...data.counts.map((row) =>
              el(
                'tr',
                {},
                el(
                  'td',
                  {},
                  el('span', { class: 'mono' }, row.function.name),
                  el(
                    'small',
                    { class: 'source-label mono' },
                    `${row.function.file ?? 'Source unavailable'}${row.function.line == null ? '' : `:${row.function.line}`}`,
                  ),
                  'code_id' in row.function &&
                    typeof row.function.code_id === 'string'
                    ? el(
                        'details',
                        {},
                        el(
                          'summary',
                          { class: 'source-label' },
                          'Code identity',
                        ),
                        el('code', {}, row.function.code_id),
                      )
                    : null,
                ),
                el('td', { class: 'numeric' }, countLabel(row.calls)),
                el('td', { class: 'numeric' }, countLabel(row.resumes)),
              ),
            ),
          ),
        ),
      ),
    );
    content.append(
      el(
        'p',
        { class: 'metric-note' },
        `${data.counts.length.toLocaleString()} displayed · ${data.total_count.toLocaleString()} retained functions. Missing functions are not measured zeros.`,
      ),
    );
  } else if (data.status !== 'running')
    content.append(
      empty(
        countStatus(data.status),
        data.status === 'not_collected'
          ? 'Choose all functions or select functions, then measure calls in a new recording.'
          : 'No function counts were retained for this scope. Inspect the diagnostics and limitations.',
      ),
    );
  if (data.next_cursor)
    content.append(
      button(
        'More function counts',
        () => {
          actions.more(data.next_cursor!);
        },
        'button subtle',
      ),
    );
  content.append(
    button(
      'Refresh counts',
      () => {
        actions.refresh();
      },
      'button subtle',
    ),
  );
  return content;
}
