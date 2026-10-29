import { operation, type TreeNode } from '../api';
import { button, el, empty, errorView, loading } from '../dom';
import { numeric } from '../format';
import type { ViewContext } from '../workspace';
import { weight, weightShare, totalWeight } from '../profile-weight';
import { hasInspectableSource } from '../source-path';

export async function profile(context: ViewContext, unit = '', path: string[] = [], cursor?: string, breadcrumbs: string[] = [], initialMode: 'tree' | 'flame' = 'tree'): Promise<HTMLElement> {
  const data = await operation('profiles.tree', { run_id: context.run.id, weight_unit: unit || undefined, limit: 1000, depth_limit: 16, path, cursor });
  const { roots, weight_unit } = data;
  const content = el('div');
  let mode = initialMode;
  let focused: TreeNode | undefined;
  let focusedPath: string[] = [];
  let focusedNames: string[] = [];
  const query = el('input', { type: 'search', placeholder: 'Find a loaded function', 'aria-label': 'Find a function' });
  const treeButton = button('Call tree', () => { mode = 'tree'; render(); });
  const flameButton = button('Flamegraph', () => { mode = 'flame'; render(); });
  const reset = button('Reset stack', () => { focused = undefined; render(); }, 'button subtle small');
  const selection = el('div', { class: 'metric-note' });
  const units = el('select', { 'aria-label': 'Stack weight unit' });
  for (const value of ['', 'samples', 'microseconds', 'bytes', 'count']) units.append(el('option', { value }, value || 'Automatic weight unit'));
  units.value = unit;
  let request = 0;
  function navigate(nextPath: string[], nextCursor?: string, names = breadcrumbs, selectedUnit = units.value): void {
    const epoch = ++request;
    content.replaceChildren(loading('Loading stack weights…'));
    void profile(context, selectedUnit, nextPath, nextCursor, names, mode).then(next => { if (epoch === request) body.replaceWith(next); })
      .catch(error => { if (epoch === request) content.replaceChildren(errorView(error, () => navigate(nextPath, nextCursor, names, selectedUnit))); });
  }
  units.addEventListener('change', () => navigate([], undefined, []));
  const expanded = new Set<string>(roots.map((_, index) => String(index)));
  query.addEventListener('input', render);
  const body = el('div', { class: 'stack' }, el('div', { class: 'toolbar' }, treeButton, flameButton, reset, el('span', { class: 'spacer' }), units, query), selection, content,
    el('div', { class: 'toolbar' },
      path.length ? button('All stack roots', () => navigate([], undefined, []), 'button small') : null,
      path.length ? button('Parent stack scope', () => navigate(path.slice(0, -1), undefined, breadcrumbs.slice(0, -1)), 'button small') : null,
      cursor ? button('First sibling page', () => navigate(path), 'button small') : null,
      data.next_cursor ? button('Next sibling page', () => navigate(path, data.next_cursor!), 'button small') : null),
    path.length ? el('p', { class: 'mono metric-note' }, `Children of ${breadcrumbs.join(' → ')}`) : null,
    el('p', { class: 'metric-note' }, `${numeric(data.returned_nodes ?? 0)} loaded frames · query node limit ${data.node_limit ?? 1000}, depth ${data.depth_limit ?? 16}. Search covers loaded frames. Inspect omitted children to continue a truncated path.`),
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
      focused.children_truncated ? button('Inspect omitted children', () => navigate(focusedPath, undefined, focusedNames), 'button small') : null,
      hasInspectableSource(focused.file, focused.code_id) ? button('Open source', () => context.source(focused!.file!, focused!.line, focused!.code_id ?? undefined), 'text-button') : el('span', {}, focused.file ?? 'Source unavailable')));
    treeButton.setAttribute('aria-pressed', String(mode === 'tree'));
    flameButton.setAttribute('aria-pressed', String(mode === 'flame'));
    treeButton.className = `button ${mode === 'tree' ? 'primary' : 'subtle'}`;
    flameButton.className = `button ${mode === 'flame' ? 'primary' : 'subtle'}`;
    content.replaceChildren(!roots.length ? empty('No stack profile for this unit', 'Choose another weight unit or inspect the collector diagnostics.') : mode === 'tree' ? tree() : flamegraph());
  }
  function tree(): HTMLElement {
    const rows = el('tbody');
    let count = 0;
    function add(node: TreeNode, depth: number, key: string, ancestors: string[], names: string[]): void {
      if ((query.value && !contains(node)) || count++ >= 2_000) return;
      const toggle = button(node.children.length ? (expanded.has(key) || query.value ? '▾' : '▸') : '·', () => {
        if (expanded.has(key)) expanded.delete(key); else expanded.add(key);
        render();
      }, 'tree-toggle');
      toggle.disabled = !node.children.length;
      toggle.setAttribute('aria-label', `${expanded.has(key) ? 'Collapse' : 'Expand'} ${node.name}`);
      if (node.children.length) toggle.setAttribute('aria-expanded', String(expanded.has(key)));
      const name = button(node.name, () => { if (hasInspectableSource(node.file, node.code_id)) context.source(node.file, node.line, node.code_id ?? undefined); }, 'text-button mono');
      name.disabled = !hasInspectableSource(node.file, node.code_id);
      const label = el('div', { class: 'tree-name' }, toggle, name);
      if (node.children_truncated) label.append(button('Inspect children…', () => navigate([...ancestors, node.id], undefined, [...names, node.name]), 'text-button small'));
      label.style.paddingLeft = `${depth * 16}px`;
      rows.append(el('tr', {}, el('td', {}, label), el('td', { class: 'numeric' }, weight(node.self_weight, weight_unit)), el('td', { class: 'numeric' }, weight(node.total_weight, weight_unit))));
      if (expanded.has(key) || query.value) node.children.forEach((child, index) => add(child, depth + 1, `${key}.${index}`, [...ancestors, node.id], [...names, node.name]));
    }
    roots.forEach((node, index) => add(node, 0, String(index), path, breadcrumbs));
    return el('section', { class: 'panel' }, el('div', { class: 'table-scroll' }, el('table', {},
      el('thead', {}, el('tr', {}, el('th', {}, 'Stack path'), el('th', { class: 'numeric' }, 'Self weight'), el('th', { class: 'numeric' }, 'Inclusive weight'))), rows)),
      el('p', { class: 'table-foot' }, count >= 2_000 ? 'Display limit reached. Narrow the function filter.' : `${numeric(count)} visible frames`));
  }
  function flamegraph(): HTMLElement {
    let count = 0;
    function branch(node: TreeNode, parentWeight: number | null, ancestors: string[], names: string[]): HTMLElement {
      const group = el('div');
      group.style.width = `${weightShare(node.total_weight, parentWeight) ?? 0}%`;
      group.style.minWidth = '1px';
      if (count++ >= 2_000) return group;
      const frame = button(node.name, () => { focused = node; focusedPath = [...ancestors, node.id]; focusedNames = [...names, node.name]; render(); }, 'flame-frame');
      frame.style.width = '100%';
      frame.style.height = '29px';
      let color = 0;
      for (const character of node.name) color = (color * 31 + character.charCodeAt(0)) | 0;
      frame.style.background = ['#e1b96a', '#b2c9b8', '#9fb8ca', '#d8b88a', '#d4c998', '#91b6b3'][Math.abs(color) % 6];
      frame.style.opacity = query.value && !matches(node) ? '.3' : '1';
      frame.title = `${node.name}\nInclusive: ${weight(node.total_weight, weight_unit)}\nSelf: ${weight(node.self_weight, weight_unit)}${node.file ? `\n${node.file}:${node.line ?? ''}` : '\nSource unavailable'}`;
      frame.setAttribute('aria-label', frame.title.replaceAll('\n', ', '));
      group.append(frame);
      if (node.children.length) group.append(el('div', { class: 'flame-children' }, ...node.children.map(child => branch(child, node.total_weight, [...ancestors, node.id], [...names, node.name]))));
      return group;
    }
    const shown = focused ? [focused] : roots;
    const total = totalWeight(shown.map(root => root.total_weight));
    const graph = el('div', { class: 'flamegraph' }, el('div', { class: 'flame-children' }, ...shown.map(root => branch(root, total, focused ? focusedPath.slice(0, -1) : path, focused ? focusedNames.slice(0, -1) : breadcrumbs))));
    return el('section', { class: 'panel' }, el('div', { class: 'panel-head' }, el('h2', {}, 'Sampled stacks'), el('small', {}, 'Click to focus · hover for weight')),
      el('div', { class: 'table-scroll' }, graph), count >= 2_000 ? el('p', { class: 'table-foot' }, 'First 2,000 frames shown. Use the call tree to inspect specific paths.') : null);
  }
  render();
  return body;
}
