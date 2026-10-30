import { operation } from '../api';
import { button, el, field, lockFields } from '../dom';
import { importFormat } from './formats';

export function importRecordingDialog(actions: {
  imported: (id: string, foreground: boolean) => void;
  unconfirmed: () => void;
}): void {
  const dialog = el('dialog', { 'aria-labelledby': 'import-title' });
  const path = el('input', {
    required: true,
    placeholder: 'Absolute local profile path',
    autocomplete: 'off',
  });
  const discovery = new AbortController();
  const format = importFormat(discovery.signal);
  const name = el('input', {
    placeholder: 'Optional recording name',
    autocomplete: 'off',
  });
  const feedback = el('p', { role: 'status', class: 'metric-note' });
  const close = button('Close', () => dialog.close(), 'button subtle small');
  const cancel = button('Cancel', () => dialog.close(), 'button subtle');
  const submit = button(
    'Import profile',
    () => {
      void save();
    },
    'button primary',
  );
  const form = el(
    'form',
    {},
    el(
      'div',
      { class: 'dialog-head' },
      el('h2', { id: 'import-title' }, 'Import retained profile'),
      close,
    ),
    el(
      'div',
      { class: 'dialog-body stack' },
      field('Local profile path', path),
      format.element,
      field('Name', name),
      el(
        'p',
        { class: 'metric-note' },
        'The local service reads this path and retains the original profile and supported sidecars. Imported observations retain their collection limits; import does not establish workload correctness.',
      ),
      feedback,
    ),
    el('div', { class: 'dialog-actions' }, cancel, submit),
  );
  form.addEventListener('submit', (event) => {
    event.preventDefault();
    void save();
  });
  dialog.append(form);
  document.body.append(dialog);
  dialog.addEventListener('close', () => {
    discovery.abort();
    dialog.remove();
  });
  dialog.showModal();
  path.focus();
  async function save(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    const unlock = lockFields(form);
    submit.disabled = true;
    cancel.textContent = 'Close dialog';
    feedback.textContent =
      'Importing profile… Closing this dialog does not cancel the submitted import.';
    try {
      const result = await operation('recordings.import', {
        path: path.value,
        format: format.value(),
        name: name.value || undefined,
      });
      const foreground = dialog.open;
      if (dialog.open) dialog.close();
      actions.imported(result.run.id, foreground);
    } catch (error) {
      feedback.textContent = `Import did not return a recording confirmation. ${error instanceof Error ? error.message : String(error)} Check Recordings for any retained input before retrying.`;
      actions.unconfirmed();
    } finally {
      submit.disabled = false;
      unlock();
    }
  }
}
