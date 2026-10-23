import { operation } from '../api';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { date } from '../format';
import { parseCommand } from '../command';
import type { ViewContext } from '../workspace';

export async function analyses(context: ViewContext): Promise<HTMLElement> {
  const content = el('div');
  const feedback = el('div', { role: 'status' });
  const body = el('div', { class: 'stack' },
    el('div', { class: 'view-title' }, el('h2', {}, 'Durable analyzers'), el('p', {}, 'Managed packages · fresh inputs · atomic publication')),
    el('p', { class: 'notice' }, 'Applicability checks constrain reuse. A passing declaration does not prove the semantic correctness of an analyzer’s conclusions.'),
    el('div', { class: 'toolbar' }, button('Register analyzer', register), button('Refresh results', () => { void load(); }, 'button subtle')),
    feedback, content);
  async function load(): Promise<void> {
    content.replaceChildren(loading('Loading analyzer packages and results…'));
    try {
      const [packages, results] = await Promise.all([operation('analyses.list', { run_id: context.run.id }), operation('analyses.results', { run_id: context.run.id })]);
      const list = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Registered packages')));
      if (!packages.analyses.length) list.append(empty('No registered analyzers', 'Register an analyzer package to derive additional relationships from declared recording inputs.'));
      for (const analysis of packages.analyses) {
        const run = button('Run on this recording', () => {
          run.disabled = true;
          feedback.replaceChildren(loading('Running analyzer against this recording…'));
          void operation('analyses.run', { run_id: context.run.id, analysis_id: analysis.id }).then(result => {
            feedback.replaceChildren(el('p', { class: 'notice' }, `${result.status} · result set ${result.result_set_id}`, ...result.diagnostics.map(value => el('span', { class: 'source-label' }, value))));
            void load();
          }).catch(error => { feedback.replaceChildren(errorView(error, () => run.click())); }).finally(() => { run.disabled = false; });
        }, 'button small');
        list.append(el('div', { class: 'result-item' }, el('div', { class: 'toolbar' }, el('h3', {}, analysis.name), badge(analysis.version), el('span', { class: 'spacer' }), run),
          el('p', { class: 'mono' }, analysis.command.join(' ')), el('details', {}, el('summary', { class: 'metric-note' }, 'Declared applicability'), el('pre', {}, JSON.stringify(analysis.requirements, null, 2)))));
      }
      const published = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Result sets'), el('small', {}, 'Accepted observations join Timeline and Evidence')));
      if (!results.results.length) published.append(empty('No analyzer results', 'Completed attempts and accepted result identities will be retained with the recording.'));
      for (const result of results.results) published.append(el('div', { class: 'result-item' },
        el('div', { class: 'toolbar' }, el('h3', { class: 'mono' }, result.id), badge(result.status, result.status === 'accepted' ? 'good' : 'warn')),
        el('p', {}, `${result.analysis_id} · ${result.version} · ${date(new Date(result.created_at_us / 1_000).toISOString())}`),
        el('p', { class: 'mono source-label' }, `Input fingerprint ${result.input_fingerprint}`), ...result.diagnostics.map(message => el('p', {}, message))));
      content.replaceChildren(el('div', { class: 'stack' }, list, published));
    } catch (error) { content.replaceChildren(errorView(error, () => { void load(); })); }
  }
  function register(): void {
    const dialog = el('dialog', { 'aria-labelledby': 'analysis-title' });
    const name = el('input', { required: true });
    const version = el('input', { value: '1', required: true });
    const cwd = el('input', { required: true, placeholder: 'Absolute analyzer package directory' });
    const command = el('textarea', { class: 'mono', required: true, rows: 3 });
    command.value = '["python3", "analyze.py"]';
    const runtime = el('input', { value: context.run.runtime });
    const fingerprint = el('input', { class: 'mono', value: context.run.source.fingerprint });
    const message = el('p', { role: 'alert', class: 'metric-note' });
    const submit = button('Register package', () => { void save(); }, 'button primary');
    const form = el('form', {}, el('div', { class: 'dialog-head' }, el('h2', { id: 'analysis-title' }, 'Register analyzer')),
      el('div', { class: 'dialog-body' }, field('Name', name), field('Version', version), field('Package directory', cwd), field('Command · JSON argument array', command),
        field('Required runtime', runtime), field('Required source fingerprint', fingerprint), el('p', { class: 'metric-note' }, 'The package is retained by Hunin. The command executes with your local user permissions. Dependencies must be available in the execution environment.'), message),
      el('div', { class: 'dialog-actions' }, button('Cancel', () => dialog.close(), 'button subtle'), submit));
    form.addEventListener('submit', event => { event.preventDefault(); void save(); });
    dialog.append(form); document.body.append(dialog); dialog.addEventListener('close', () => dialog.remove()); dialog.showModal(); name.focus();
    async function save(): Promise<void> {
      if (submit.disabled || !form.reportValidity()) return;
      try {
        const args = parseCommand(command.value);
        submit.disabled = true;
        await operation('analyses.register', { name: name.value, version: version.value, cwd: cwd.value, command: args,
          requirements: { runtime: runtime.value || undefined, source_fingerprint: fingerprint.value || undefined } });
        dialog.close(); void load();
      } catch (error) { message.textContent = error instanceof Error ? error.message : String(error); submit.disabled = false; }
    }
  }
  await load();
  return body;
}
