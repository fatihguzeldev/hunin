import { operation } from '../api';
import { el } from '../dom';
import { launchChoices, launchRequest, type LaunchChoice } from './choices';
import { captureFields } from '../code/capture-fields';

/** Share one backend-owned mode picker without rewriting an explicit workload command. */
export function collectionMode(
  command: HTMLTextAreaElement,
  signal: AbortSignal,
) {
  const runtime = el('select');
  runtime.append(el('option', { value: '' }, 'Detect automatically'));
  const choices = new Map<string, LaunchChoice>();
  const capture = captureFields();
  const launchInfo = el(
    'p',
    { class: 'metric-note', role: 'status' },
    'Loading collection methods…',
  );
  const discovery = el('p', { class: 'metric-note' });
  const details = el(
    'details',
    { class: 'collection-details hidden' },
    el('summary', {}, 'Collection prerequisites'),
    discovery,
  );
  function describeChoice(): void {
    const choice = choices.get(runtime.value);
    capture.setActive(
      choice?.captureMode === 'code-origin' && Boolean(choice.launchRuntime),
    );
    launchInfo.setAttribute('role', 'status');
    launchInfo.textContent = choice
      ? choice.commandRequirement || 'Use an explicit target command.'
      : 'Detects the runtime from your command and checks prerequisites at launch.';
    details.classList.toggle('hidden', !choice);
    discovery.textContent = choice
      ? `Prerequisites: ${choice.availability.replaceAll('_', ' ')}. ${choice.recordingVerified ? 'A recording session was verified.' : 'A recording session has not been checked.'}${choice.defaultExecutable ? ` Default executable: ${choice.defaultExecutable}.` : ''}`
      : '';
    command.placeholder = choice?.defaultExecutable
      ? JSON.stringify([choice.defaultExecutable, 'argument'])
      : '["/path/to/program", "argument"]';
  }
  runtime.addEventListener('change', describeChoice);
  void operation('capabilities', {}, signal)
    .then((data) => {
      if (signal.aborted) return;
      for (const choice of launchChoices(data.runtimes)) {
        choices.set(choice.value, choice);
        runtime.append(
          el(
            'option',
            { value: choice.value, disabled: !choice.launchRuntime },
            choice.unavailableReason
              ? `${choice.label} · unavailable`
              : choice.label,
          ),
        );
      }
      describeChoice();
    })
    .catch((failure: unknown) => {
      if (signal.aborted) return;
      launchInfo.setAttribute('role', 'alert');
      launchInfo.textContent = `Collection methods could not be loaded. Automatic detection remains available. ${failure instanceof Error ? failure.message : String(failure)}`;
    });
  return {
    select: runtime,
    description: el(
      'div',
      { class: 'stack' },
      launchInfo,
      details,
      capture.element,
    ),
    value: () => {
      const choice = choices.get(runtime.value);
      if (runtime.value && !choice)
        throw new Error(
          'Choose a selectable collection mode or automatic detection.',
        );
      return launchRequest(choice, capture.value());
    },
  };
}
