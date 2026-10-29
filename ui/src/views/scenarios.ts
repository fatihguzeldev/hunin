import { scenarioMetadata } from '../metadata';
import { operation, type Scenario } from '../api';
import { parseCommand } from '../command';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { workloadFields } from '../workload-fields';
import { pagedList } from '../paged-list';

export async function scenarios(onRun: (id: string) => void): Promise<HTMLElement> {
  const list = pagedList<Scenario>({
    label: 'scenarios', nextLabel: 'More scenarios',
    load: async cursor => { const page = await operation('scenarios.list', { cursor, limit: 25 }); return { items: page.scenarios, next_cursor: page.next_cursor }; },
    render: renderScenarios,
  });
  const feedback = el('div', { role: 'status' });
  const name = el('input', { required: true, placeholder: 'e.g. renderer reference workload' });
  const command = el('textarea', { class: 'mono', required: true, rows: 2 });
  command.value = '["node", "app.js"]';
  const cwd = el('input', { required: true, placeholder: 'Absolute project directory' });
  const runtime = el('input', { placeholder: 'Detect automatically' });
  const timeout = el('input', { type: 'number', min: 1, max: 86400, value: 60 });
  const workload = workloadFields();
  const submit = button('Save scenario', () => { void save(); }, 'button primary');
  const form = el('form', { class: 'stack panel-body' }, field('Name', name), field('Working directory', cwd), field('Command · JSON argument array', command),
    el('div', { class: 'toolbar' }, field('Runtime', runtime), field('Timeout · seconds', timeout)), workload.element, submit);
  form.addEventListener('submit', event => { event.preventDefault(); void save(); });
  const body = el('div', { class: 'stack' }, el('div', { class: 'view-title' }, el('h1', {}, 'Workload scenarios')),
    el('p', { class: 'muted' }, 'Retain the command, working directory, timeout, and correctness expectation for repeatable recordings.'), feedback, list.element,
    el('details', { class: 'panel' }, el('summary', { class: 'panel-body' }, 'Save a workload'), form));
  function renderScenarios(items: Scenario[]): HTMLElement {
    if (!items.length) return empty('No saved workloads', 'Save a scenario below, then reuse it before and after a code change.');
    return el('section', { class: 'panel' }, ...items.map(item => {
      const scenario = scenarioMetadata(item);
      const scenarioId = scenario.id;
      const run = button('Record scenario', () => {
        if (!scenarioId) return;
        run.disabled = true;
        feedback.replaceChildren(loading('Starting scenario…'));
        void operation('scenarios.run', { scenario_id: scenarioId }).then(data => onRun(data.run.id)).catch(error => {
          feedback.replaceChildren(errorView(error, () => run.click()));
        }).finally(() => { run.disabled = false; });
      }, 'button small');
      run.disabled = !scenarioId || !scenario.command?.length;
      return el('div', { class: 'result-item' }, el('div', { class: 'toolbar' }, el('h2', {}, scenario.name), badge(scenario.runtime || 'auto'), el('span', { class: 'spacer' }), run),
        el('p', { class: 'mono' }, scenario.command?.length ? scenario.command.join(' ') : 'Command unavailable'), el('p', {}, `${scenario.cwd ?? 'default working directory'} · timeout ${scenario.timeout_seconds === undefined ? 'default' : `${scenario.timeout_seconds}s`} · expected exit ${scenario.expected_exit_code ?? 'default'}`),
        el('details', {}, el('summary', {}, 'Workload input and output expectations'), el('pre', {}, JSON.stringify({ env: scenario.env ?? {}, input_files: scenario.input_files ?? [], expected_stdout: scenario.expected_stdout, expected_stdout_sha256: scenario.expected_stdout_sha256 }, null, 2))),
        run.disabled ? el('details', {}, el('summary', {}, 'Retained workload metadata · unavailable fields'), el('pre', {}, JSON.stringify(scenario.raw, null, 2))) : null);
    }));
  }
  async function save(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    try {
      const args = parseCommand(command.value);
      const options = workload.value();
      submit.disabled = true;
      await operation('scenarios.save', { name: name.value, command: args, cwd: cwd.value, runtime: runtime.value || undefined, timeout_seconds: Number(timeout.value), ...options });
      feedback.replaceChildren(el('p', { class: 'notice' }, 'Scenario saved. Recording executes the command with your local user permissions.'));
      await list.refresh(true);
    } catch (error) { feedback.replaceChildren(errorView(error, () => { void save(); })); }
    finally { submit.disabled = false; }
  }
  await list.refresh(true);
  return body;
}
