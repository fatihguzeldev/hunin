import { operation, type Run } from '../api';
import { button, el } from '../dom';
import {
  inspectRemovalReceipt,
  removalFailureMessage,
} from './removal-feedback';

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
  const receiptFeedback = el('div', { role: 'status', class: 'stack' });
  const receiptRead = new AbortController();
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
      receiptFeedback,
    ),
    el('div', { class: 'dialog-actions' }, cancel, submit),
  );
  document.body.append(dialog);
  dialog.addEventListener('close', () => {
    receiptRead.abort();
    dialog.remove();
  });
  dialog.showModal();
  cancel.focus();
  async function remove(): Promise<void> {
    if (submit.disabled) return;
    submit.disabled = true;
    cancel.textContent = 'Close dialog';
    feedback.textContent =
      'Removing recording… Closing this dialog does not undo the submitted request.';
    receiptFeedback.replaceChildren();
    try {
      await operation('runs.remove', { run_id: run.id });
      if (dialog.open) dialog.close();
      actions.removed();
    } catch (error) {
      feedback.textContent = removalFailureMessage(error);
      actions.unconfirmed();
      if (!dialog.open || receiptRead.signal.aborted) return;
      receiptFeedback.textContent = 'Checking retained cleanup receipt…';
      const notice = await inspectRemovalReceipt(run.id, async () => {
        const result = await operation(
          'runs.removal',
          { run_id: run.id },
          receiptRead.signal,
        );
        return result.receipt;
      });
      if (!dialog.open || receiptRead.signal.aborted) return;
      receiptFeedback.replaceChildren(
        el('p', { class: 'metric-note' }, notice.message),
      );
      if (notice.lastError)
        receiptFeedback.append(
          el(
            'p',
            { class: 'metric-note' },
            `Retained cleanup error: ${notice.lastError}`,
          ),
        );
      submit.textContent =
        notice.kind === 'pending'
          ? 'Retry same removal'
          : 'Retry removal request';
    } finally {
      submit.disabled = false;
    }
  }
}
