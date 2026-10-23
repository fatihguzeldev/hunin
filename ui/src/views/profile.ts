import { operation, type TreeNode } from '../api';
import { button, el, empty, errorView, loading } from '../dom';
import { numeric } from '../format';
import type { ViewContext } from '../workspace';
import { weight } from './overview';
import { hasLocalSource } from '../source-path';

export async function profile(context: ViewContext, unit = ''): Promise<HTMLElement> {
  const { roots, weight_unit } = await operation('profiles.tree', { run_id: context.run.id, weight_unit: unit || undefined });
  const content = el('div');
  let mode: 'tree' | 'flame' = 'tree';
  let focused: TreeNode | undefined;
  const query = el('input', { type: 'search', placeholder: 'Find a function', 'aria-label': 'Find a function' });
  const treeButton = button('Call tree', () => { mode = 'tree'; render(); });
  const flameButton = button('Flamegraph', () => { mode = 'flame'; render(); });
  const reset = button('Reset stack', () => { focused = undefined; render(); }, 'button subtle small');
  const selection = el('div', { class: 'metric-note' });
  const units = el('select', { 'aria-label': 'Stack weight unit' });
  for (const value of ['', 'samples', 'microseconds', 'bytes', 'count']) units.append(el('option', { value }, value || 'Automatic weight unit'));
  units.value = unit;
  let request = 0;
  units.addEventListener('change', () => {
    const epoch = ++request;
    content.replaceChildren(loading('Loading stack weights…'));
    void profile(context, units.value).then(next => { if (epoch === request) body.replaceWith(next); })
      .catch(error => { if (epoch === request) content.replaceChildren(errorView(error, () => units.dispatchEvent(new Event('change')))); });
  });
  const expanded = new Set<string>(roots.map((_, index) => String(index)));
  query.addEventListener('input', render);
  const body = el('div', { class: 'stack' }, el('div', { class: 'toolbar' }, treeButton, flameButton, reset, el('span', { class: 'spacer' }), units, query), selection, content,
    el('p', { class: 'metric-note' }, `Width and weights represent ${weight_unit}. A sampled stack tree does not establish request ownership or function invocation counts.`));
  function matches(node: TreeNode): boolean {
    return node.name.toLowerCase().includes(query.value.toLowerCase()) || Boolean(node.file?.toLowerCase().includes(query.value.toLowerCase()));
  }
  function contains(node: TreeNode): boolean { return matches(node) || node.children.some(contains); }
  function render(): void {
    reset.classList.toggle('hidden', !focused || mode !== 'flame');
    selection.replaceChildren();
    if (focused && mode === 'flame') selection.append(el('div', { class: 'toolbar' },
      el('span', { class: 'mono' }, focused.name), el('span', {}, `inclusive ${weight(focused.total_weight, weight_unit)} · self ${weight(focused.self_weight, weight_unit)}`),
      hasLocalSource(focused.file) ? button('Open source', () => context.source(focused!.file!, focused!.line), 'text-button') : el('span', {}, focused.file ?? 'Source unavailable')));
    treeButton.setAttribute('aria-pressed', String(mode === 'tree'));
    flameButton.setAttribute('aria-pressed', String(mode === 'flame'));
    treeButton.className = `button ${mode === 'tree' ? 'primary' : 'subtle'}`;
    flameButton.className = `button ${mode === 'flame' ? 'primary' : 'subtle'}`;
    content.replaceChildren(!roots.length ? empty('No stack profile for this unit', 'Choose another weight unit or inspect the collector diagnostics.') : mode === 'tree' ? tree() : flamegraph());
  }
  function tree(): HTMLElement {
    const rows = el('tbody');
    let count = 0;
    function add(node: TreeNode, depth: number, path: string): void {
      if ((query.value && !contains(node)) || count++ >= 2_000) return;
      const toggle = button(node.children.length ? (expanded.has(path) || query.value ? '▾' : '▸') : '·', () => {
        if (expanded.has(path)) expanded.delete(path); else expanded.add(path);
        render();
      }, 'tree-toggle');
      toggle.disabled = !node.children.length;
      toggle.setAttribute('aria-label', `${expanded.has(path) ? 'Collapse' : 'Expand'} ${node.name}`);
      if (node.children.length) toggle.setAttribute('aria-expanded', String(expanded.has(path)));
      const name = button(node.name, () => { if (hasLocalSource(node.file)) context.source(node.file, node.line); }, 'text-button mono');
      name.disabled = !hasLocalSource(node.file);
      const label = el('div', { class: 'tree-name' }, toggle, name);
      label.style.paddingLeft = `${depth * 16}px`;
      rows.append(el('tr', {}, el('td', {}, label), el('td', { class: 'numeric' }, weight(node.self_weight, weight_unit)), el('td', { class: 'numeric' }, weight(node.total_weight, weight_unit))));
      if (expanded.has(path) || query.value) node.children.forEach((child, index) => add(child, depth + 1, `${path}.${index}`));
    }
    roots.forEach((node, index) => add(node, 0, String(index)));
    return el('section', { class: 'panel' }, el('div', { class: 'table-scroll' }, el('table', {},
      el('thead', {}, el('tr', {}, el('th', {}, 'Stack path'), el('th', { class: 'numeric' }, 'Self weight'), el('th', { class: 'numeric' }, 'Inclusive weight'))), rows)),
      el('p', { class: 'table-foot' }, count >= 2_000 ? 'Display limit reached. Narrow the function filter.' : `${numeric(count)} visible frames`));
  }
  function flamegraph(): HTMLElement {
    let count = 0;
    function branch(node: TreeNode, parentWeight: number): HTMLElement {
      const group = el('div');
      group.style.width = `${parentWeight > 0 ? Math.min(100, node.total_weight / parentWeight * 100) : 0}%`;
      group.style.minWidth = '1px';
      if (count++ >= 2_000) return group;
      const frame = button(node.name, () => { focused = node; render(); }, 'flame-frame');
      frame.style.width = '100%';
      frame.style.height = '29px';
      let color = 0;
      for (const character of node.name) color = (color * 31 + character.charCodeAt(0)) | 0;
      frame.style.background = ['#e1b96a', '#b2c9b8', '#9fb8ca', '#d8b88a', '#d4c998', '#91b6b3'][Math.abs(color) % 6];
      frame.style.opacity = query.value && !matches(node) ? '.3' : '1';
      frame.title = `${node.name}\nInclusive: ${weight(node.total_weight, weight_unit)}\nSelf: ${weight(node.self_weight, weight_unit)}${node.file ? `\n${node.file}:${node.line ?? ''}` : '\nSource unavailable'}`;
      frame.setAttribute('aria-label', frame.title.replaceAll('\n', ', '));
      group.append(frame);
      if (node.children.length) group.append(el('div', { class: 'flame-children' }, ...node.children.map(child => branch(child, node.total_weight))));
      return group;
    }
    const shown = focused ? [focused] : roots;
    const total = shown.reduce((value, root) => value + root.total_weight, 0);
    const graph = el('div', { class: 'flamegraph' }, el('div', { class: 'flame-children' }, ...shown.map(root => branch(root, total))));
    return el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Sampled stacks'), el('small', {}, 'Click to focus · hover for weight')),
      el('div', { class: 'table-scroll' }, graph), count >= 2_000 ? el('p', { class: 'table-foot' }, 'First 2,000 frames shown. Use the call tree to inspect specific paths.') : null);
  }
  render();
  return body;
}
