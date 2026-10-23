import { operation } from '../api';
import { parseCommand } from '../command';
import { badge, button, el, empty, errorView, field, loading } from '../dom';

export async function scenarios(onRun: (id: string) => void): Promise<HTMLElement> {
  const list = el('div');
  const feedback = el('div', { role: 'status' });
  const name = el('input', { required: true, placeholder: 'e.g. renderer reference workload' });
  const command = el('textarea', { class: 'mono', required: true, rows: 2 });
  command.value = '["node", "app.js"]';
  const cwd = el('input', { required: true, placeholder: 'Absolute project directory' });
  const runtime = el('input', { placeholder: 'Detect automatically' });
  const timeout = el('input', { type: 'number', min: 1, max: 86400, value: 60 });
  const expected = el('input', { type: 'number', min: 0, max: 255, value: 0 });
  const submit = button('Save scenario', () => { void save(); }, 'button primary');
  const form = el('form', { class: 'stack panel-body' }, field('Name', name), field('Working directory', cwd), field('Command · JSON argument array', command),
    el('div', { class: 'toolbar' }, field('Runtime', runtime), field('Timeout · seconds', timeout), field('Expected exit code', expected)), submit);
  form.addEventListener('submit', event => { event.preventDefault(); void save(); });
  const body = el('div', { class: 'stack' }, el('div', { class: 'view-title' }, el('h1', {}, 'Workload scenarios')),
    el('p', { class: 'muted' }, 'Retain the command, working directory, timeout, and correctness expectation for repeatable recordings.'), feedback, list,
    el('details', { class: 'panel' }, el('summary', { class: 'panel-body' }, 'Save a workload'), form));
  async function load(): Promise<void> {
    list.replaceChildren(loading('Loading scenarios…'));
    try {
      const { scenarios } = await operation('scenarios.list', {});
      if (!scenarios.length) { list.replaceChildren(empty('No saved workloads', 'Save a scenario below, then reuse it before and after a code change.')); return; }
      list.replaceChildren(el('section', { class: 'panel' }, ...scenarios.map(scenario => {
        const run = button('Record scenario', () => {
          run.disabled = true;
          feedback.replaceChildren(loading('Starting scenario…'));
          void operation('scenarios.run', { scenario_id: scenario.id }).then(data => onRun(data.run.id)).catch(error => {
            feedback.replaceChildren(errorView(error, () => run.click()));
          }).finally(() => { run.disabled = false; });
        }, 'button small');
        return el('div', { class: 'result-item' }, el('div', { class: 'toolbar' }, el('h2', {}, scenario.name), badge(scenario.runtime || 'auto'), el('span', { class: 'spacer' }), run),
          el('p', { class: 'mono' }, scenario.command.join(' ')), el('p', {}, `${scenario.cwd} · timeout ${scenario.timeout_seconds}s · expected exit ${scenario.expected_exit_code}`));
      })));
    } catch (error) { list.replaceChildren(errorView(error, () => { void load(); })); }
  }
  async function save(): Promise<void> {
    if (submit.disabled || !form.reportValidity()) return;
    try {
      const args = parseCommand(command.value);
      submit.disabled = true;
      await operation('scenarios.save', { name: name.value, command: args, cwd: cwd.value, runtime: runtime.value || undefined, timeout_seconds: Number(timeout.value), expected_exit_code: Number(expected.value) });
      feedback.replaceChildren(el('p', { class: 'notice' }, 'Scenario saved. Recording executes the command with your local user permissions.'));
      await load();
    } catch (error) { feedback.replaceChildren(errorView(error, () => { void save(); })); }
    finally { submit.disabled = false; }
  }
  await load();
  return body;
}
