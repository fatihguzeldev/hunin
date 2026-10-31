import { el, field } from '../dom';
import type { CodeCaptureValues } from './capture';

export function captureFields() {
  let active = false;
  const compilerExport = el('input', { type: 'checkbox' });
  const filter = el('input', {
    placeholder: 'Optional compiler function filter',
    maxlength: 256,
  });
  const wasmFunctionIndex = el('input', {
    type: 'number',
    min: 0,
    max: 2_147_483_647,
    step: 1,
    placeholder: 'All functions',
  });
  const interval = el('input', {
    type: 'number',
    min: 100,
    max: 1_000_000,
    step: 1,
    placeholder: '1000',
  });
  const budget = el('input', {
    type: 'number',
    min: 1,
    max: 536_870_912,
    step: 1,
    placeholder: '67108864',
  });
  function syncDisabled(): void {
    compilerExport.disabled = !active;
    interval.disabled = !active;
    budget.disabled = !active;
    filter.disabled = !active || !compilerExport.checked;
    wasmFunctionIndex.disabled = !active || !compilerExport.checked;
  }
  compilerExport.addEventListener('change', syncDisabled);
  syncDisabled();
  const options = el(
    'div',
    { class: 'stack hidden' },
    el(
      'p',
      { class: 'metric-note' },
      'Code origin capture currently requires Node. It creates a new recording; collected origins do not add evidence to an earlier recording.',
    ),
    el(
      'details',
      {},
      el('summary', {}, 'Code capture options'),
      el(
        'div',
        { class: 'stack' },
        el(
          'label',
          { class: 'toolbar' },
          compilerExport,
          'Retain compiler export',
        ),
        el(
          'p',
          { class: 'metric-note' },
          'Compiler tracing adds overhead and V8 diagnostic output to stdout.',
        ),
        field('Compiler function filter', filter),
        field('Wasm disassembly function index', wasmFunctionIndex),
        el(
          'p',
          { class: 'metric-note' },
          'The index limits disassembly in every Wasm module. Native sampling and compiler tracing keep their configured scope.',
        ),
        field('Sampling interval · µs', interval),
        field('Artifact budget · bytes', budget),
      ),
    ),
  );
  return {
    element: options,
    setActive: (value: boolean): void => {
      active = value;
      options.classList.toggle('hidden', !active);
      syncDisabled();
    },
    value: (): CodeCaptureValues => ({
      compilerExport: compilerExport.checked,
      filter: compilerExport.checked ? filter.value : '',
      wasmFunctionIndex: compilerExport.checked ? wasmFunctionIndex.value : '',
      interval: interval.value,
      budget: budget.value,
    }),
  };
}
