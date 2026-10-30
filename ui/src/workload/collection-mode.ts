import { operation } from '../api';
import { el } from '../dom';
import { launchChoices, type LaunchChoice } from './choices';

/** Share one backend-owned mode picker without rewriting an explicit workload command. */
export function collectionMode(
  command: HTMLTextAreaElement,
  signal: AbortSignal,
) {
  const runtime = el('select');
  runtime.append(el('option', { value: '' }, 'Detect automatically'));
  const choices = new Map<string, LaunchChoice>();
  const launchInfo = el(
    'p',
    { class: 'metric-note', role: 'status' },
    'Loading registered collection modes…',
  );
  function describeChoice(): void {
    const choice = choices.get(runtime.value);
    launchInfo.textContent = choice
      ? [
          choice.commandRequirement,
          `Discovery: ${choice.availability}.`,
          choice.defaultExecutable
            ? `Default interpreter/tool: ${choice.defaultExecutable}.`
            : 'Supply the target command explicitly.',
          choice.recordingVerified
            ? 'Discovery reports recording verification.'
            : 'Discovery has not verified a recording session.',
        ]
          .filter(Boolean)
          .join(' ')
      : 'The service detects the runtime from your explicit command. Recording checks its prerequisites when it starts.';
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
      launchInfo.textContent = `Collection-mode discovery failed. Automatic detection remains available for an explicit command. ${failure instanceof Error ? failure.message : String(failure)}`;
    });
  return {
    select: runtime,
    description: launchInfo,
    value: (): string | undefined => {
      const choice = choices.get(runtime.value);
      if (runtime.value && !choice?.launchRuntime)
        throw new Error(
          'Choose a selectable collection mode or automatic detection.',
        );
      return choice?.launchRuntime;
    },
  };
}
