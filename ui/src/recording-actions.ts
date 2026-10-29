import { operation, type Run } from './api';
import { button, el, field, lockFields } from './dom';
import { importFormat } from './import-format';

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

export function removeRecordingDialog(
  run: Run,
  actions: {
    removed: () => void;
    unconfirmed: () => void;
  },
): void {
  const dialog = el('dialog', {
    'aria-labelledby': 'remove-title',
    'aria-describedby': 'remove-description',
  });
  const feedback = el('p', { role: 'status', class: 'metric-note' });
  const cancel = button('Cancel', () => dialog.close(), 'button subtle');
  const submit = button(
    'Remove permanently',
    () => {
      void remove();
    },
    'button danger',
  );
  dialog.append(
    el(
      'div',
      { class: 'dialog-head' },
      el('h2', { id: 'remove-title' }, 'Remove recording'),
    ),
    el(
      'div',
      { class: 'dialog-body stack' },
      el('p', {}, run.name),
      el('p', { class: 'mono' }, run.id),
      el(
        'p',
        { id: 'remove-description' },
        'This removes the recording, its analyzer attempt history and artifact references from this local store. Unreferenced Hunin-owned files are collected; shared files stay with other recordings. Files outside Hunin-managed storage stay at their selected paths. There is no undo in the app.',
      ),
      feedback,
    ),
    el('div', { class: 'dialog-actions' }, cancel, submit),
  );
  document.body.append(dialog);
  dialog.addEventListener('close', () => dialog.remove());
  dialog.showModal();
  cancel.focus();
  async function remove(): Promise<void> {
    if (submit.disabled) return;
    submit.disabled = true;
    cancel.textContent = 'Close dialog';
    feedback.textContent =
      'Removing recording… Closing this dialog does not undo the submitted request.';
    try {
      await operation('runs.remove', { run_id: run.id });
      if (dialog.open) dialog.close();
      actions.removed();
    } catch (error) {
      feedback.textContent = `Removal did not return a confirmation. ${error instanceof Error ? error.message : String(error)} Refresh Recordings to confirm the stored state before retrying.`;
      actions.unconfirmed();
    } finally {
      submit.disabled = false;
    }
  }
}
