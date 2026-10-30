import { operation } from '../api';
import { button, el, field, lockFields } from '../dom';
import { parseCommand } from '../command';
import { recipeFields } from '../workload/fields';

export function recordDialog(
  onRecorded: (id: string, foreground: boolean) => void,
): void {
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
  const discovery = new AbortController();
  const {
    mode,
    timeout,
    expectations: workload,
  } = recipeFields(command, discovery.signal);
  const error = el('p', { class: 'metric-note', role: 'alert' });
  const cancel = button('Cancel', () => dialog.close(), 'button subtle');
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
      field('Collection mode', mode.select),
      mode.description,
      field('Timeout · seconds', timeout),
      workload.element,
      error,
    ),
    el('div', { class: 'dialog-actions' }, cancel, submit),
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
    let unlock: (() => void) | undefined;
    try {
      const args = parseCommand(command.value);
      const seconds = Number(timeout.value);
      if (!Number.isFinite(seconds) || seconds < 1 || seconds > 86400)
        throw new Error('Timeout must be between 1 and 86400 seconds.');
      const options = workload.value();
      const runtime = mode.value();
      submit.disabled = true;
      unlock = lockFields(form);
      cancel.textContent = 'Close dialog';
      error.textContent =
        'Starting collector… Closing this dialog does not cancel the submitted recording.';
      const result = await operation('runs.start', {
        name: name.value || undefined,
        command: args,
        cwd: cwd.value,
        runtime,
        timeout_seconds: seconds,
        ...options,
      });
      const foreground = dialog.open;
      if (dialog.open) dialog.close();
      onRecorded(result.run.id, foreground);
    } catch (failure) {
      error.textContent =
        failure instanceof Error ? failure.message : String(failure);
      submit.disabled = false;
    } finally {
      unlock?.();
    }
  }
}
