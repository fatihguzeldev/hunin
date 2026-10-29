import { operation } from './api.js';
import { el, field } from './dom.js';
import type { ImportFormat } from './operations.generated.js';

type ImportChoice = {
  value: string;
  label: string;
  description: string;
  disabled: boolean;
};

/** Parser keys and recording ownership come from the installed service. */
export function importChoices(
  formats: readonly ImportFormat[] | undefined,
): ImportChoice[] | undefined {
  return formats?.map((format) => ({
    value: format.format,
    label: format.label,
    disabled: format.requires_existing_recording,
    description: [
      format.description,
      format.requires.length
        ? `Requirements: ${format.requires.join('; ')}.`
        : '',
      format.aliases.length
        ? `Accepted aliases: ${format.aliases.join(', ')}.`
        : '',
      format.requires_existing_recording
        ? 'Import into an existing recording with matching run IDs. This dialog creates a new recording.'
        : '',
    ]
      .filter(Boolean)
      .join(' '),
  }));
}

export function importFormat(signal: AbortSignal): {
  element: HTMLElement;
  value: () => string;
} {
  const select = el('select', { required: true });
  select.append(el('option', { value: '' }, 'Loading import formats…'));
  const control = el('div', {}, field('Profile format', select));
  const description = el(
    'p',
    { class: 'metric-note', role: 'status' },
    'Loading registered profile formats from the local service…',
  );
  const ownership = el('p', { class: 'metric-note' });
  const choices = new Map<string, ImportChoice>();
  let manual: HTMLInputElement | undefined;
  function describe(): void {
    description.textContent =
      choices.get(select.value)?.description ??
      'Choose the format produced by your profiler. This catalog lists supported inputs; it does not verify decoder dependencies or a completed import.';
  }
  function manualFallback(message: string): void {
    const focused = control.contains(document.activeElement);
    manual = el('input', {
      required: true,
      placeholder: 'Exact format accepted by the installed service',
      autocomplete: 'off',
    });
    control.replaceChildren(field('Profile format', manual));
    description.textContent = `${message} Enter the exact format accepted by the installed service. A filename extension alone does not establish its format.`;
    if (focused) manual.focus();
  }
  select.addEventListener('change', describe);
  void operation('capabilities', {}, signal)
    .then((data) => {
      if (signal.aborted) return;
      const formats = importChoices(data.import_formats);
      if (formats === undefined) {
        manualFallback(
          'This service did not provide an import format catalog.',
        );
        return;
      }
      select.replaceChildren(
        el(
          'option',
          { value: '' },
          formats.length
            ? 'Choose a profile format'
            : 'No import formats registered',
        ),
        ...formats.map((choice) => {
          choices.set(choice.value, choice);
          return el(
            'option',
            { value: choice.value, disabled: choice.disabled },
            choice.disabled
              ? `${choice.label} · existing recording required`
              : choice.label,
          );
        }),
      );
      description.textContent = formats.length
        ? 'Choose the format produced by your profiler. The service checks required inputs and decoder dependencies during import.'
        : 'This service has no registered import formats. A new recording cannot be imported here.';
      ownership.textContent = formats.some((choice) => choice.disabled)
        ? 'Formats marked “existing recording required” retain their run IDs and must be imported into that recording.'
        : '';
    })
    .catch((failure: unknown) => {
      if (signal.aborted) return;
      manualFallback(
        `Import format discovery failed. ${failure instanceof Error ? failure.message : String(failure)}`,
      );
    });
  return {
    element: el('div', { class: 'stack' }, control, description, ownership),
    value: () => manual?.value ?? select.value,
  };
}
