import { operation } from './api';
import { button, el, field } from './dom';
import { parseCommand } from './command';
import { workloadFields } from './workload-fields';
import { launchChoices, type LaunchChoice } from './launch';

export function recordDialog(onRecorded: (id: string) => void): void {
  const dialog = el('dialog', { 'aria-labelledby': 'record-title' });
  const name = el('input', {
    placeholder: 'e.g. renderer baseline',
    autocomplete: 'off',
  });
  const command = el('textarea', {
    class: 'mono',
    rows: 3,
    required: true,
    spellcheck: false,
  });
  command.placeholder = '["/path/to/program", "argument"]';
  const cwd = el('input', {
    placeholder: 'Absolute project directory',
    required: true,
    autocomplete: 'off',
  });
  const runtime = el('select');
  runtime.append(el('option', { value: '' }, 'Detect automatically'));
  const discovery = new AbortController();
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
  void operation('capabilities', {}, discovery.signal)
    .then((data) => {
      if (discovery.signal.aborted) return;
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
      if (discovery.signal.aborted) return;
      launchInfo.textContent = `Collection-mode discovery failed. Automatic detection remains available for an explicit command. ${failure instanceof Error ? failure.message : String(failure)}`;
    });
  const timeout = el('input', {
    type: 'number',
    min: 1,
    max: 86400,
    value: 60,
  });
  const workload = workloadFields();
  const error = el('p', { class: 'metric-note', role: 'alert' });
  const submit = button(
    'Start recording',
    () => {
      void start();
    },
    'button primary',
  );
  const form = el(
    'form',
    {},
    el(
      'div',
      { class: 'dialog-head' },
      el('h2', { id: 'record-title' }, 'Record a workload'),
      button('Close', () => dialog.close(), 'button subtle small'),
    ),
    el(
      'div',
      { class: 'dialog-body' },
      field('Name', name),
      field('Working directory', cwd),
      field('Command · JSON argument array', command),
      el(
        'p',
        { class: 'metric-note' },
        'The command runs locally with your user permissions. Arguments are passed directly to the executable.',
      ),
      field('Collection mode', runtime),
      launchInfo,
      field('Timeout · seconds', timeout),
      workload.element,
      error,
    ),
    el(
      'div',
      { class: 'dialog-actions' },
      button('Cancel', () => dialog.close(), 'button subtle'),
      submit,
    ),
  );
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    void start();
  });
  dialog.append(form);
  document.body.append(dialog);
  dialog.addEventListener('close', () => {
    discovery.abort();
    dialog.remove();
  });
  dialog.showModal();
  name.focus();
  async function start(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    try {
      const args = parseCommand(command.value);
      const seconds = Number(timeout.value);
      if (!Number.isFinite(seconds) || seconds < 1 || seconds > 86400)
        throw new Error('Timeout must be between 1 and 86400 seconds.');
      const options = workload.value();
      const choice = choices.get(runtime.value);
      if (runtime.value && !choice?.launchRuntime)
        throw new Error(
          'Choose a selectable collection mode or automatic detection.',
        );
      submit.disabled = true;
      error.textContent = 'Starting collector…';
      const result = await operation('runs.start', {
        name: name.value || undefined,
        command: args,
        cwd: cwd.value,
        runtime: choice?.launchRuntime,
        timeout_seconds: seconds,
        ...options,
      });
      dialog.close();
      onRecorded(result.run.id);
    } catch (failure) {
      error.textContent =
        failure instanceof Error ? failure.message : String(failure);
      submit.disabled = false;
    }
  }
}
