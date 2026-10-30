import { operation } from '../api';
import { button, el, field } from '../dom';
import { parseCommand } from '../command';
import type { runSource } from '../recordings/presentation';

export function registerAnalysis(
  recordingRuntime: string,
  source: ReturnType<typeof runSource>,
  registered: () => Promise<unknown>,
): void {
  const dialog = el('dialog', { 'aria-labelledby': 'analysis-title' });
  const name = el('input', { required: true });
  const version = el('input', { value: '1', required: true });
  const cwd = el('input', {
    required: true,
    placeholder: 'Absolute analyzer package directory',
  });
  const command = el('textarea', { class: 'mono', required: true, rows: 3 });
  command.value = '["python3", "analyze.py"]';
  const runtime = el('input', { value: recordingRuntime });
  const fingerprint = el('input', {
    class: 'mono',
    value: source.fingerprint,
  });
  const scope = el('select');
  scope.append(
    el('option', { value: 'source' }, 'This measured source build'),
    el('option', { value: 'generic' }, 'Source-independent analyzer'),
  );
  scope.addEventListener('change', () => {
    fingerprint.disabled = scope.value === 'generic';
    runtime.value = scope.value === 'generic' ? '*' : recordingRuntime;
  });
  const dependencies = el('textarea', {
    class: 'mono',
    rows: 3,
    spellcheck: false,
  });
  dependencies.value = '{}';
  const message = el('p', { role: 'alert', class: 'metric-note' });
  const submit = button(
    'Register package',
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
      el('h2', { id: 'analysis-title' }, 'Register analyzer'),
    ),
    el(
      'div',
      { class: 'dialog-body' },
      field('Name', name),
      field('Version', version),
      field('Package directory', cwd),
      field('Command · JSON argument array', command),
      field('Applicability', scope),
      field('Required runtime · * for any runtime', runtime),
      field('Required source fingerprint', fingerprint),
      el(
        'p',
        { class: 'metric-note' },
        'Source-specific registration also pins the available executable digest and runtime version. This does not establish a verified build-to-source mapping.',
      ),
      field('Dependencies · JSON declaration', dependencies),
      el(
        'p',
        { class: 'metric-note' },
        'Declare python_requirements, node_lockfile, or setup_commands when needed. Lockfile paths are relative to the package. Hunin prepares dependencies in a managed attempt workspace. The command executes with your local user permissions.',
      ),
      message,
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
    void save();
  });
  dialog.append(form);
  document.body.append(dialog);
  dialog.addEventListener('close', () => dialog.remove());
  dialog.showModal();
  name.focus();
  async function save(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    try {
      const args = parseCommand(command.value);
      const declared: unknown = JSON.parse(dependencies.value);
      if (!declared || typeof declared !== 'object' || Array.isArray(declared))
        throw new Error('Dependencies must be a JSON object.');
      submit.disabled = true;
      await operation('analyses.register', {
        name: name.value,
        version: version.value,
        cwd: cwd.value,
        command: args,
        requirements: {
          runtime: runtime.value || undefined,
          ...(scope.value === 'generic'
            ? { source_independent: true }
            : {
                source_fingerprint: fingerprint.value || undefined,
                executable_sha256: source.executable_sha256 || undefined,
                runtime_version: source.runtime_version || undefined,
              }),
        },
        dependencies: declared as Record<string, unknown>,
      });
      dialog.close();
      await registered();
    } catch (error) {
      message.textContent =
        error instanceof Error ? error.message : String(error);
      submit.disabled = false;
    }
  }
}
