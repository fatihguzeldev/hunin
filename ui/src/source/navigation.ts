import type { OperationOutputs } from '../operations.generated';
import { button, el, field } from '../dom';

export function sourceNavigation(
  data: OperationOutputs['source.read'],
  frameLine: number | undefined,
  change: (line: number) => void,
): HTMLElement {
  const start = el('input', {
    type: 'number',
    min: 1,
    max: Number.MAX_SAFE_INTEGER,
    step: 1,
    value: data.start_line,
  });
  const feedback = el('span', { role: 'alert', class: 'metric-note' });
  const count =
    data.content === ''
      ? data.total_lines !== undefined && data.start_line <= data.total_lines
        ? 1
        : 0
      : data.content.split('\n').length;
  const description = count
    ? `Lines ${data.start_line}–${data.start_line + count - 1}${data.total_lines !== undefined ? ` of ${data.total_lines}` : ''}`
    : data.total_lines === undefined
      ? `No source text returned at line ${data.start_line}; the line count is unavailable.`
      : `No source lines at ${data.start_line} · ${data.total_lines} lines in this source`;
  const form = el(
    'form',
    { class: 'toolbar' },
    field('Source window starts at line', start),
    button('Go to source line', go, 'button small'),
    feedback,
  );
  function go(): void {
    const line = Number(start.value);
    if (!start.reportValidity()) return;
    if (!Number.isSafeInteger(line) || line < 1) {
      feedback.textContent =
        'Enter a positive whole line number within the supported integer range.';
      return;
    }
    change(line);
  }
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    go();
  });
  const previous = button(
    'Previous source window',
    () => change(Math.max(1, data.start_line - 120)),
    'button small',
  );
  previous.disabled = data.start_line <= 1;
  return el(
    'section',
    { class: 'stack', 'aria-label': 'Source navigation' },
    el('p', { class: 'metric-note' }, description),
    el(
      'div',
      { class: 'toolbar' },
      previous,
      typeof data.next_line === 'number'
        ? button(
            'Next source window',
            () => {
              if (typeof data.next_line === 'number') change(data.next_line);
            },
            'button small',
          )
        : data.next_line === null
          ? el('span', { class: 'metric-note' }, 'End of source')
          : null,
      frameLine !== undefined
        ? button(
            `Frame line ${frameLine}`,
            () => change(frameLine),
            'button small',
          )
        : null,
    ),
    data.next_line === undefined
      ? el(
          'p',
          { class: 'metric-note' },
          'This service did not provide continuation metadata. Enter a source line to inspect another window.',
        )
      : null,
    form,
  );
}
