import { button, el } from '../../dom';
import { weight, weightShare } from '../../profile-weight';
import type { Investigation } from '../state';
import { commonChain } from './selection';
import { nodeAt, type Inspection, type ProfileNode } from './model';
import { stackScope } from './geometry';

export function stacks(
  data: Inspection,
  state: Investigation['workbench'],
  select: (id: string, path?: string[]) => void,
  persist: () => void,
): HTMLElement {
  const root = el('div', { class: 'wb-stacks' });
  const focused = nodeAt(data.roots, state.focusPath);
  if (state.focusPath.length && !focused) state.focusPath = [];
  const initial = focused ? [focused] : data.roots;
  const chain = commonChain(initial, state.selectedFrameId);
  const rootPaths = new Map<string, string[]>();
  function paths(nodes: ProfileNode[], prefix: string[]) {
    for (const node of nodes) {
      const path = [...prefix, node.id];
      rootPaths.set(node.id, path);
      paths(node.children, path);
    }
  }
  paths(data.roots, []);
  const ancestors = el('div', { class: 'wb-stack-context' });
  const coverage = stackScope(
    data.total_weight,
    initial.map((node) => node.total_weight),
    Boolean(focused),
  );
  if (focused) {
    const names: string[] = [];
    for (let depth = 1; depth <= state.focusPath.length; depth++) {
      const ancestor = nodeAt(data.roots, state.focusPath.slice(0, depth));
      if (ancestor) names.push(ancestor.name);
    }
    const path = el(
      'details',
      { class: 'wb-focused-path' },
      el('summary', {}, `Call path: ${focused.name}`),
    );
    path.append(
      el('ol', {}, ...names.map((name) => el('li', { class: 'mono' }, name))),
    );
    const share = weightShare(focused.total_weight, data.total_weight);
    ancestors.append(
      path,
      el(
        'span',
        {},
        `${weight(focused.total_weight, data.weight_unit)} in this path${share === undefined ? '' : ` · ${Math.round(share * 10) / 10}% of window weight`}. Function totals include every path.`,
      ),
      button(
        'Show full stack',
        () => {
          state.focusPath = [];
          persist();
          root.replaceWith(stacks(data, state, select, persist));
        },
        'button subtle',
      ),
    );
  }
  if (chain.ancestors.length) {
    const details = el(
      'details',
      { class: 'wb-common-frames' },
      el('summary', {}, `${chain.ancestors.length} shared parent frames`),
    );
    for (const node of chain.ancestors) {
      const choose = button(
        node.name,
        () => select(node.frame_id, rootPaths.get(node.id)),
        'wb-common-frame mono',
      );
      choose.setAttribute(
        'data-wb-path',
        JSON.stringify(rootPaths.get(node.id) ?? []),
      );
      choose.title = weight(node.total_weight, data.weight_unit);
      details.append(choose);
    }
    ancestors.append(details);
  }
  const tree = el('div', {
    class: 'wb-flame',
    role: 'group',
    'aria-label':
      'Sampled call paths. Select a frame to inspect; Shift and Enter focuses its call path.',
  });
  const denominator = coverage.denominator;
  const visible: ProfileNode[] = [];
  function draw(
    nodes: ProfileNode[],
    parent: HTMLElement,
    level: number,
    parentWeight: number | null,
  ) {
    const line = el('div', { class: 'wb-flame-level' });
    for (const node of nodes) {
      visible.push(node);
      const share = weightShare(node.total_weight, parentWeight);
      const branch = el('div', { class: `wb-flame-branch level-${level % 3}` });
      branch.style.width = `${share ?? 100}%`;
      const frame = button(
        node.name,
        () => select(node.frame_id, rootPaths.get(node.id)),
        'wb-flame-frame',
      );
      frame.setAttribute(
        'data-wb-path',
        JSON.stringify(rootPaths.get(node.id) ?? []),
      );
      frame.setAttribute(
        'aria-pressed',
        String(node.frame_id === state.selectedFrameId),
      );
      frame.title = `${node.name}\n${weight(node.total_weight, data.weight_unit)} inclusive · ${weight(node.self_weight, data.weight_unit)} self${node.children_truncated || node.depth_truncated ? '\nSome child frames are outside this query.' : ''}`;
      frame.setAttribute(
        'aria-label',
        `${node.name}, ${weight(node.total_weight, data.weight_unit)} inclusive${node.children_truncated || node.depth_truncated ? ', children truncated' : ''}`,
      );
      frame.addEventListener('keydown', (event) => {
        if (event.key === 'Enter' && event.shiftKey) {
          event.preventDefault();
          focus(node);
        }
      });
      branch.append(frame);
      if (node.children.length) {
        const children = el('div', { class: 'wb-flame-children' });
        draw(node.children, children, level + 1, node.total_weight);
        branch.append(children);
      }
      line.append(branch);
    }
    parent.append(line);
  }
  function focus(node: ProfileNode) {
    select(node.frame_id, rootPaths.get(node.id));
    state.focusPath = rootPaths.get(node.id) ?? [];
    persist();
    root.replaceWith(stacks(data, state, select, persist));
  }
  if (denominator === null) {
    tree.append(
      el('p', { class: 'wb-read-status' }, 'Stack weights are unavailable.'),
    );
    function unweighted(nodes: ProfileNode[], level: number) {
      for (const node of nodes) {
        visible.push(node);
        const frame = button(
          node.name,
          () => select(node.frame_id, rootPaths.get(node.id)),
          'wb-unweighted-frame mono',
        );
        frame.setAttribute(
          'data-wb-path',
          JSON.stringify(rootPaths.get(node.id) ?? []),
        );
        frame.style.paddingLeft = `${level * 16}px`;
        tree.append(frame);
        unweighted(node.children, level + 1);
      }
    }
    unweighted(chain.roots, 0);
  } else draw(chain.roots, tree, 0, denominator);
  if (!visible.length)
    tree.append(
      el(
        'p',
        { class: 'wb-no-results' },
        data.sample_count
          ? 'The samples in this window have no captured stack frames.'
          : 'No samples in this window.',
      ),
    );
  const omitted =
    coverage.omitted !== null && coverage.omitted > 0
      ? el(
          'p',
          { class: 'wb-stack-omitted' },
          `${weight(coverage.omitted, data.weight_unit)} has no displayed root call path.`,
        )
      : null;
  root.append(
    ancestors,
    tree,
    ...(omitted ? [omitted] : []),
    el(
      'div',
      { class: 'wb-results-footer' },
      el(
        'span',
        {},
        data.tree_truncated
          ? 'Some frames are outside the query limit.'
          : focused
            ? 'Width uses this call path’s weight.'
            : 'Width uses selected-window weight.',
      ),
      el('span', {}, 'Shift + Enter focuses a path'),
    ),
  );
  return root;
}
