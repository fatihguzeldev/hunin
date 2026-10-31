import { button, el } from '../../dom';
import { icon } from '../../design/icons';
import { numeric } from '../../format';
import { weight, weightShare } from '../../profile-weight';
import type { Investigation } from '../state';
import { rows, shortPath, type Inspection } from './model';

export function functions(
  data: Inspection,
  state: Investigation['workbench'],
  select: (id: string) => void,
  persist: () => void,
): HTMLElement {
  const root = el('div', { class: 'wb-functions' });
  const search = el('input', {
    type: 'search',
    placeholder: 'Filter functions',
    'aria-label': 'Filter loaded functions',
    class: 'wb-function-search',
  });
  search.value = state.query;
  const table = el('table', { class: 'wb-function-table' });
  const footer = el('div', { class: 'wb-results-footer' });
  const sort = (key: 'self' | 'inclusive') => {
    state.sort = key;
    persist();
    draw();
    table
      .querySelector<HTMLButtonElement>(`[aria-label="Sort by ${key} weight"]`)
      ?.focus({ preventScroll: true });
  };
  function draw() {
    const matches = rows(data.functions, state.query, state.sort);
    const head = el('tr', {}, el('th', { scope: 'col' }, 'Function'));
    for (const [key, label, detail] of [
      ['self', 'Self', 'Sampled weight where the function is the leaf frame'],
      [
        'inclusive',
        'Inclusive',
        'Sampled weight including child calls, once per sample',
      ],
    ] as const) {
      const control = button(label, () => sort(key), 'wb-sort');
      control.title = detail;
      control.setAttribute(
        'aria-label',
        `Sort by ${label.toLowerCase()} weight`,
      );
      head.append(
        el(
          'th',
          {
            scope: 'col',
            'aria-sort': state.sort === key ? 'descending' : 'none',
          },
          control,
        ),
      );
    }
    const body = el('tbody');
    for (const frame of matches) {
      const choose = button('', () => select(frame.id), 'wb-function');
      choose.setAttribute('data-wb-frame', frame.id);
      choose.setAttribute(
        'aria-pressed',
        String(frame.id === state.selectedFrameId),
      );
      choose.append(
        el('span', { class: 'wb-function-name mono' }, frame.function),
        el(
          'span',
          {
            class: 'wb-function-file',
            title: frame.file ?? frame.module ?? 'No source location',
          },
          frame.file ? shortPath(frame.file) : frame.module || 'Runtime',
        ),
      );
      const row = el(
        'tr',
        { class: frame.id === state.selectedFrameId ? 'selected' : '' },
        el('td', {}, choose),
      );
      for (const value of [frame.self_weight, frame.inclusive_weight]) {
        const percent = weightShare(value, data.total_weight);
        const cell = el(
          'td',
          {
            class: 'wb-function-weight mono',
            title: weight(value, data.weight_unit),
          },
          percent === undefined ? '—' : `${numeric(percent)}%`,
        );
        if (percent !== undefined) {
          const bar = el('span', {
            class: 'wb-weight-bar',
            'aria-hidden': true,
          });
          bar.style.setProperty('--wb-fill', `${percent}%`);
          cell.prepend(bar);
        }
        row.append(cell);
      }
      body.append(row);
    }
    if (!matches.length)
      body.append(
        el(
          'tr',
          {},
          el(
            'td',
            { colspan: 3, class: 'wb-no-results' },
            state.query
              ? 'No loaded functions match this filter.'
              : data.sample_count
                ? 'The samples in this window have no captured stack frames.'
                : 'No samples in this window.',
          ),
        ),
      );
    table.replaceChildren(el('thead', {}, head), body);
    footer.replaceChildren(
      el(
        'span',
        {},
        state.query
          ? `${numeric(matches.length)} matches`
          : `${numeric(data.functions.length)} functions`,
      ),
      el(
        'span',
        {},
        data.functions_truncated
          ? `${numeric(data.functions.length)} of ${numeric(data.total_functions)} loaded`
          : 'Share of selected-window weight',
      ),
    );
    if (data.functions_truncated) {
      const url = new URL(location.href);
      url.searchParams.set('view', 'profile');
      url.hash = '';
      footer.append(el('a', { href: url.href }, 'Browse call tree'));
    }
  }
  search.addEventListener('input', () => {
    state.query = search.value;
    persist();
    draw();
  });
  const filter = el(
    'label',
    { class: 'wb-function-filter' },
    icon('search'),
    search,
  );
  root.append(
    filter,
    el('div', { class: 'wb-functions-scroll' }, table),
    footer,
  );
  draw();
  return root;
}
