import { operation } from '../api';
import { button, el, field, lockFields } from '../dom';
import { parseCommand } from '../command';
import { recipeFields } from '../workload/fields';
import { rejectedBeforeAdmission } from './submission';

export function recordDialog(actions: {
  recorded: (id: string, foreground: boolean) => void;
  unconfirmed: () => void;
}): void {
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
  const feedback = el('p', { class: 'metric-note', role: 'status' });
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
      el('h2', { id: 'record-title' }, 'New recording'),
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
        'Runs locally. Arguments are passed literally; shell syntax is not expanded.',
      ),
      field('Collection mode', mode.select),
      mode.description,
      field('Timeout · seconds', timeout),
      workload.element,
      feedback,
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
    let submitted = false;
    try {
      const args = parseCommand(command.value);
      const seconds = Number(timeout.value);
      if (!Number.isFinite(seconds) || seconds < 1 || seconds > 86400)
        throw new Error('Timeout must be between 1 and 86400 seconds.');
      const options = workload.value();
      const launch = mode.value();
      submit.disabled = true;
      submit.textContent = 'Starting…';
      form.setAttribute('aria-busy', 'true');
      unlock = lockFields(form);
      cancel.textContent = 'Close dialog';
      feedback.setAttribute('role', 'status');
      feedback.textContent =
        'Starting recording. You can close this dialog; recording will continue.';
      submitted = true;
      const result = await operation('runs.start', {
        name: name.value || undefined,
        command: args,
        cwd: cwd.value,
        ...launch,
        timeout_seconds: seconds,
        ...options,
      });
      const foreground = dialog.open;
      if (dialog.open) dialog.close();
      actions.recorded(result.run.id, foreground);
    } catch (failure) {
      const message =
        failure instanceof Error ? failure.message : String(failure);
      const unconfirmed =
        submitted && !rejectedBeforeAdmission(failure, 'runs.start');
      feedback.setAttribute('role', 'alert');
      feedback.textContent = unconfirmed
        ? `Recording start is unconfirmed. ${message} Check Recordings before retrying.`
        : message;
      if (unconfirmed) actions.unconfirmed();
    } finally {
      form.removeAttribute('aria-busy');
      unlock?.();
      submit.disabled = false;
      submit.textContent = 'Start recording';
      cancel.textContent = 'Cancel';
    }
  }
}
