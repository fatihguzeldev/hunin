import { operation } from '../api';
import { button, el, field, lockFields } from '../dom';
import { parseCommand } from '../command';
import type { runSource } from '../recordings/presentation';
import { analyzerRequirements } from './requirements';

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
  const runtime = el('input', { value: recordingRuntime, required: true });
  const fingerprint = el('input', {
    class: 'mono',
    value: source.fingerprint,
    required: true,
  });
  const scope = el('select');
  const output = el(
    'select',
    {},
    el('option', { value: 'observations' }, 'Derived observations'),
    el('option', { value: 'code-mapping' }, 'Code mappings · v1'),
  );
  scope.append(
    el('option', { value: 'recording' }, 'This recording’s source'),
    el('option', { value: 'independent' }, 'Source-independent'),
  );
  scope.addEventListener('change', () => {
    fingerprint.disabled = scope.value === 'independent';
    runtime.value = scope.value === 'independent' ? '*' : recordingRuntime;
  });
  const dependencies = el('textarea', {
    class: 'mono',
    rows: 3,
    spellcheck: false,
  });
  dependencies.value = '{}';
  const message = el('p', { role: 'status', class: 'metric-note' });
  const submit = button(
    'Register analyzer',
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
      field('Output contract', output),
      field('Package directory', cwd),
      field('Command · JSON argument array', command),
      field('Applicability', scope),
      field('Runtime · * accepts any', runtime),
      field('Source fingerprint', fingerprint),
      el(
        'p',
        { class: 'metric-note' },
        'Source-specific analyzers also match known executable and runtime identities. This does not verify the source-to-build mapping.',
      ),
      el(
        'details',
        { class: 'panel' },
        el('summary', { class: 'panel-body' }, 'Dependencies'),
        el(
          'div',
          { class: 'panel-body stack' },
          field('Dependency declaration · JSON', dependencies),
          el(
            'p',
            { class: 'metric-note' },
            'Optional python_requirements, node_lockfile or setup_commands. File paths are relative to the package directory.',
          ),
        ),
      ),
      el(
        'details',
        { class: 'panel' },
        el('summary', { class: 'panel-body' }, 'Analyzer input and output'),
        el(
          'div',
          { class: 'panel-body stack' },
          el(
            'p',
            {},
            'Reads JSON from stdin and writes observation JSON lines to stdout. Each derived observation must reference input record IDs in evidence.',
          ),
          el('p', {}, 'The command runs locally with your user permissions.'),
          el('code', {}, 'hunin catalog analyses.register'),
        ),
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
    let unlock: (() => void) | undefined;
    try {
      const args = parseCommand(command.value);
      const declared: unknown = JSON.parse(dependencies.value);
      if (!declared || typeof declared !== 'object' || Array.isArray(declared))
        throw new Error('Dependencies must be a JSON object.');
      const requirements = analyzerRequirements(
        scope.value === 'independent' ? 'independent' : 'recording',
        runtime.value,
        { ...source, fingerprint: fingerprint.value },
      );
      const outputContract =
        output.value === 'code-mapping'
          ? { name: 'code-mapping' as const, version: 1 as const }
          : undefined;
      submit.disabled = true;
      submit.textContent = 'Registering…';
      form.setAttribute('aria-busy', 'true');
      unlock = lockFields(form);
      message.setAttribute('role', 'status');
      message.textContent = 'Registering analyzer…';
      await operation('analyses.register', {
        name: name.value,
        version: version.value,
        cwd: cwd.value,
        command: args,
        requirements,
        dependencies: declared as Record<string, unknown>,
        ...(outputContract ? { output_contract: outputContract } : {}),
      });
      dialog.close();
      await registered();
    } catch (error) {
      message.setAttribute('role', 'alert');
      message.textContent =
        error instanceof Error ? error.message : String(error);
    } finally {
      form.removeAttribute('aria-busy');
      unlock?.();
      submit.disabled = false;
      submit.textContent = 'Register analyzer';
    }
  }
}
