import { el, field } from '../dom';
import { parseWorkloadOptions } from './options';

export function workloadFields(): {
  element: HTMLElement;
  value: () => ReturnType<typeof parseWorkloadOptions>;
} {
  const environment = el('textarea', {
    class: 'mono',
    rows: 2,
    spellcheck: false,
  });
  environment.value = '{}';
  const files = el('textarea', { class: 'mono', rows: 2, spellcheck: false });
  files.value = '[]';
  const exit = el('input', { type: 'number', min: 0, max: 255, value: 0 });
  const mode = el('select');
  mode.append(
    el('option', { value: 'exit' }, 'Exit code only'),
    el('option', { value: 'text' }, 'Exact stdout text'),
    el('option', { value: 'sha256' }, 'Stdout SHA-256'),
  );
  const output = el('textarea', {
    class: 'mono',
    rows: 3,
    spellcheck: false,
    disabled: true,
  });
  mode.addEventListener('change', () => {
    output.disabled = mode.value === 'exit';
  });
  const element = el(
    'details',
    { class: 'panel' },
    el(
      'summary',
      { class: 'panel-body' },
      'Workload inputs and correctness checks',
    ),
    el(
      'div',
      { class: 'panel-body stack' },
      field('Environment overrides · JSON object', environment),
      field('Input files · JSON path array', files),
      el(
        'p',
        { class: 'metric-note' },
        'Declared input files are fingerprinted relative to the working directory. Include the workload data needed to assess repeatability.',
      ),
      field('Expected exit code', exit),
      field('Output validation', mode),
      field('Expected stdout · exact text or SHA-256', output),
      el(
        'p',
        { class: 'metric-note' },
        'Exact stdout includes whitespace and trailing newlines. An exit code check alone does not validate the program’s output.',
      ),
    ),
  );
  return {
    element,
    value: () =>
      parseWorkloadOptions(
        environment.value,
        files.value,
        exit.value,
        mode.value,
        output.value,
      ),
  };
}
