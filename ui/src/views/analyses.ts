import { operation, type Analysis, type AnalysisResult } from '../api';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { date } from '../format';
import { parseCommand } from '../command';
import type { ViewContext } from '../workspace';
import { pagedList } from '../paged-list';

export async function analyses(context: ViewContext): Promise<HTMLElement> {
  const feedback = el('div', { role: 'status' });
  const packages = pagedList<Analysis>({
    label: 'analyzer packages', nextLabel: 'More analyzer packages',
    load: async cursor => { const page = await operation('analyses.list', { cursor, limit: 25 }); return { items: page.analyses, next_cursor: page.next_cursor }; },
    render: renderPackages,
  });
  const attempts = pagedList<AnalysisResult>({
    label: 'analyzer attempts', nextLabel: 'Older attempts',
    load: async cursor => { const page = await operation('analyses.results', { run_id: context.run.id, cursor, limit: 25 }); return { items: page.results, next_cursor: page.next_cursor }; },
    render: renderAttempts, pollWhen: items => items.some(result => result.status === 'running'),
  });
  const body = el('div', { class: 'stack' },
    el('div', { class: 'view-title' }, el('h2', {}, 'Durable analyzers'), el('p', {}, 'Managed packages · fresh inputs · atomic publication')),
    el('p', { class: 'notice' }, 'Applicability checks constrain reuse. A passing declaration does not prove the semantic correctness of an analyzer’s conclusions.'),
    el('div', { class: 'toolbar' }, button('Register analyzer', register), button('Refresh results', () => { void Promise.all([packages.refresh(), attempts.refresh()]); }, 'button subtle')),
    feedback, packages.element, attempts.element);
  function renderPackages(items: Analysis[]): HTMLElement {
    const list = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Registered packages')));
    if (!items.length) list.append(empty('No registered analyzers', 'Register an analyzer package to derive additional relationships from declared recording inputs.'));
    for (const analysis of items) {
      const run = button('Run on this recording', () => {
        run.disabled = true;
        feedback.replaceChildren(loading('Running analyzer against this recording…'));
        void operation('analyses.run', { run_id: context.run.id, analysis_id: analysis.id }).then(result => {
          feedback.replaceChildren(el('p', { class: 'notice' }, result.status === 'running' ? `Attempt ${result.result_set_id} started. Follow its status below; output is published only after acceptance.` : `${result.status} · ${result.result_set_id}`, ...result.diagnostics.map(value => el('span', { class: 'source-label' }, value))));
          void attempts.refresh(true);
        }).catch(error => { feedback.replaceChildren(errorView(error, () => run.click())); }).finally(() => { run.disabled = false; });
      }, 'button small');
      list.append(el('div', { class: 'result-item' }, el('div', { class: 'toolbar' }, el('h3', {}, analysis.name), badge(analysis.version), el('span', { class: 'spacer' }), run),
        el('p', { class: 'mono' }, analysis.command.join(' ')), el('details', {}, el('summary', { class: 'metric-note' }, 'Applicability and dependencies'), el('pre', {}, JSON.stringify({ requirements: analysis.requirements, dependencies: analysis.dependencies }, null, 2)))));
    }
    return list;
  }
  function renderAttempts(items: AnalysisResult[]): HTMLElement {
    const published = el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Attempts and accepted results'), el('small', {}, 'Accepted observations join Timeline and Evidence')));
    if (!items.length) published.append(empty('No analyzer results', 'Completed attempts and accepted result identities will be retained with the recording.'));
    for (const result of items) published.append(el('div', { class: 'result-item' },
      el('div', { class: 'toolbar' }, el('h3', { class: 'mono' }, result.id), badge(result.status, ['accepted', 'replayed'].includes(result.status) ? 'good' : ['failed', 'timed_out'].includes(result.status) ? 'bad' : 'warn'),
        result.selected ? badge('selected result', 'good') : result.selected === false && ['accepted', 'replayed'].includes(result.status) ? badge('historical') : null,
        result.status === 'running' ? button('Cancel attempt', () => {
          void operation('analyses.cancel', { execution_id: result.id }).then(() => attempts.refresh()).catch(error => feedback.replaceChildren(errorView(error, () => { void attempts.refresh(); })));
        }, 'button small danger') : null),
      el('p', {}, `${result.analysis_id} · ${result.version} · ${date(new Date(result.created_at_us / 1_000).toISOString())}`),
      result.result_set_id && ['accepted', 'replayed'].includes(result.status) ? el('p', { class: 'mono source-label' }, `Accepted result set ${result.result_set_id}`) : null,
      el('p', { class: 'mono source-label' }, `Input fingerprint ${result.input_fingerprint}`), ...result.diagnostics.map(message => el('p', {}, message))));
    return published;
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
    const scope = el('select');
    scope.append(el('option', { value: 'source' }, 'This measured source build'), el('option', { value: 'generic' }, 'Source-independent analyzer'));
    scope.addEventListener('change', () => { fingerprint.disabled = scope.value === 'generic'; runtime.value = scope.value === 'generic' ? '*' : context.run.runtime; });
    const dependencies = el('textarea', { class: 'mono', rows: 3, spellcheck: false });
    dependencies.value = '{}';
    const message = el('p', { role: 'alert', class: 'metric-note' });
    const submit = button('Register package', () => { void save(); }, 'button primary');
    const form = el('form', {}, el('div', { class: 'dialog-head' }, el('h2', { id: 'analysis-title' }, 'Register analyzer')),
      el('div', { class: 'dialog-body' }, field('Name', name), field('Version', version), field('Package directory', cwd), field('Command · JSON argument array', command),
        field('Applicability', scope), field('Required runtime · * for any runtime', runtime), field('Required source fingerprint', fingerprint),
        el('p', { class: 'metric-note' }, 'Source-specific registration also pins the available executable digest and runtime version. This does not establish a verified build-to-source mapping.'),
        field('Dependencies · JSON declaration', dependencies), el('p', { class: 'metric-note' }, 'Declare python_requirements, node_lockfile, or setup_commands when needed. Lockfile paths are relative to the package. Hunin prepares dependencies in a managed attempt workspace. The command executes with your local user permissions.'), message),
      el('div', { class: 'dialog-actions' }, button('Cancel', () => dialog.close(), 'button subtle'), submit));
    form.addEventListener('submit', event => { event.preventDefault(); void save(); });
    dialog.append(form); document.body.append(dialog); dialog.addEventListener('close', () => dialog.remove()); dialog.showModal(); name.focus();
    async function save(): Promise<void> {
      if (submit.disabled || !form.reportValidity()) return;
      try {
        const args = parseCommand(command.value);
        const declared: unknown = JSON.parse(dependencies.value);
        if (!declared || typeof declared !== 'object' || Array.isArray(declared)) throw new Error('Dependencies must be a JSON object.');
        submit.disabled = true;
        await operation('analyses.register', { name: name.value, version: version.value, cwd: cwd.value, command: args,
          requirements: { runtime: runtime.value || undefined, ...(scope.value === 'generic' ? { source_independent: true } : {
            source_fingerprint: fingerprint.value || undefined,
            executable_sha256: context.run.source.executable_sha256 || undefined,
            runtime_version: context.run.source.runtime_version || undefined,
          }) }, dependencies: declared as Record<string, unknown> });
        dialog.close(); await packages.refresh(true);
      } catch (error) { message.textContent = error instanceof Error ? error.message : String(error); submit.disabled = false; }
    }
  }
  await Promise.all([packages.refresh(), attempts.refresh()]);
  return body;
}
