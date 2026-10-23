import { operation } from './api';
import { button, el, field } from './dom';
import { parseCommand } from './command';

export function recordDialog(onRecorded: (id: string) => void): void {
  const dialog = el('dialog', { 'aria-labelledby': 'record-title' });
  const name = el('input', { placeholder: 'e.g. renderer baseline', autocomplete: 'off' });
  const command = el('textarea', { class: 'mono', rows: 3, required: true, spellcheck: false });
  command.value = '["node", "app.js"]';
  const cwd = el('input', { placeholder: 'Absolute project directory', required: true, autocomplete: 'off' });
  const runtime = el('select');
  for (const value of ['', 'node', 'bun', 'deno', 'cpython', 'pypy', 'graalpy', 'go', 'native']) {
    runtime.append(el('option', { value }, value || 'Detect automatically'));
  }
  const timeout = el('input', { type: 'number', min: 1, max: 86400, value: 60 });
  const error = el('p', { class: 'metric-note', role: 'alert' });
  const submit = button('Start recording', () => { void start(); }, 'button primary');
  const form = el('form', {},
    el('div', { class: 'dialog-head' }, el('h2', { id: 'record-title' }, 'Record a workload'), button('Close', () => dialog.close(), 'button subtle small')),
    el('div', { class: 'dialog-body' }, field('Name', name), field('Working directory', cwd),
      field('Command · JSON argument array', command), el('p', { class: 'metric-note' }, 'The command runs locally with your user permissions. Arguments are passed directly to the executable.'),
      field('Runtime', runtime), field('Timeout · seconds', timeout), error),
    el('div', { class: 'dialog-actions' }, button('Cancel', () => dialog.close(), 'button subtle'), submit));
  form.addEventListener('submit', event => { event.preventDefault(); void start(); });
  dialog.append(form);
  document.body.append(dialog);
  dialog.addEventListener('close', () => dialog.remove());
  dialog.showModal();
  name.focus();
  async function start(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    try {
      const args = parseCommand(command.value);
      const seconds = Number(timeout.value);
      if (!Number.isFinite(seconds) || seconds < 1 || seconds > 86400) throw new Error('Timeout must be between 1 and 86400 seconds.');
      submit.disabled = true;
      error.textContent = 'Starting collector…';
      const result = await operation('runs.start', { name: name.value || undefined, command: args, cwd: cwd.value, runtime: runtime.value || undefined, timeout_seconds: seconds });
      dialog.close();
      onRecorded(result.run.id);
    } catch (failure) {
      error.textContent = failure instanceof Error ? failure.message : String(failure);
      submit.disabled = false;
    }
  }
}
