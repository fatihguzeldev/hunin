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
  const outputField = field('Expected stdout', output);
  outputField.classList.add('hidden');
  mode.addEventListener('change', () => {
    output.disabled = mode.value === 'exit';
    outputField.classList.toggle('hidden', output.disabled);
    output.placeholder =
      mode.value === 'sha256'
        ? '64-character SHA-256 digest'
        : 'Exact output, including whitespace and trailing newlines';
  });
  const element = el(
    'details',
    { class: 'panel' },
    el('summary', { class: 'panel-body' }, 'Inputs and output checks'),
    el(
      'div',
      { class: 'panel-body stack' },
      field('Environment · JSON object', environment),
      field('Input files · JSON array', files),
      el(
        'p',
        { class: 'metric-note' },
        'Input paths are relative to the working directory. Their content is fingerprinted for later comparisons.',
      ),
      field('Expected exit code', exit),
      field('Output validation', mode),
      outputField,
      el(
        'p',
        { class: 'metric-note' },
        'Exact output includes whitespace and trailing newlines. Exit-code-only checks do not validate output.',
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
