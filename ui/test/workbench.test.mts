import test from 'node:test';
import assert from 'node:assert/strict';
import {
  clampWindow,
  commonChain,
  panWindow,
  resizeWindow,
  zoomWindow,
} from '../src/investigation/workbench/selection.js';

const bounds = {
  start_us: 1_700_000_000_000_000,
  end_us: 1_700_000_010_000_000,
};

test('time selection preserves its width when restored beyond either recording edge', () => {
  assert.deepEqual(
    clampWindow(
      { start_us: bounds.end_us - 200, end_us: bounds.end_us + 800 },
      bounds,
    ),
    {
      start_us: bounds.end_us - 1000,
      end_us: bounds.end_us,
    },
  );
  assert.deepEqual(
    clampWindow(
      { start_us: bounds.start_us - 800, end_us: bounds.start_us + 200 },
      bounds,
    ),
    {
      start_us: bounds.start_us,
      end_us: bounds.start_us + 1000,
    },
  );
  assert.deepEqual(
    clampWindow({ start_us: NaN, end_us: Infinity }, bounds),
    bounds,
  );
});

test('zoom retains the pointer anchor instead of jumping to the recording midpoint', () => {
  const current = {
    start_us: bounds.start_us + 2_000_000,
    end_us: bounds.start_us + 6_000_000,
  };
  const next = zoomWindow(current, bounds, 0.5, 0.25);
  assert.deepEqual(next, {
    start_us: bounds.start_us + 2_500_000,
    end_us: bounds.start_us + 4_500_000,
  });
  assert.equal(
    next.start_us + (next.end_us - next.start_us) * 0.25,
    current.start_us + (current.end_us - current.start_us) * 0.25,
  );
});

test('panning and reversed edge drags remain bounded and never erase the selected window', () => {
  const current = {
    start_us: bounds.start_us + 200,
    end_us: bounds.start_us + 400,
  };
  assert.deepEqual(panWindow(current, bounds, -1000), {
    start_us: bounds.start_us,
    end_us: bounds.start_us + 200,
  });
  assert.deepEqual(resizeWindow(current, bounds, 'start', bounds.end_us), {
    start_us: current.end_us - 1,
    end_us: current.end_us,
  });
  assert.deepEqual(resizeWindow(current, bounds, 'end', bounds.start_us), {
    start_us: current.start_us,
    end_us: current.start_us + 1,
  });
});

type Node = {
  id: string;
  frame_id: string;
  self_weight: number | null;
  total_weight: number | null;
  child_count: number;
  children_truncated: boolean;
  depth_truncated: boolean;
  children: Node[];
};
const node = (id: string, children: Node[] = [], self = 0): Node => ({
  id,
  frame_id: id,
  self_weight: self,
  total_weight: 10,
  child_count: children.length,
  children_truncated: false,
  depth_truncated: false,
  children,
});

test('common stack chains collapse only complete zero-self single-child ancestry', () => {
  const leaf = node('work', [], 10);
  const root = node('runtime', [node('entry', [leaf])]);
  assert.deepEqual(
    commonChain([root]).ancestors.map((n) => n.id),
    ['runtime', 'entry'],
  );
  assert.deepEqual(
    commonChain([root]).roots.map((n) => n.id),
    ['work'],
  );
  assert.deepEqual(
    commonChain([root], 'entry').roots.map((n) => n.id),
    ['entry'],
  );
  root.self_weight = 1;
  assert.equal(commonChain([root]).ancestors.length, 0);
});

test('stack collapse never hides a branch, unavailable weight or bounded-away child', () => {
  for (const alter of [
    (root: Node) => {
      root.children.push(node('other'));
      root.child_count = 2;
    },
    (root: Node) => {
      root.child_count = 2;
      root.children_truncated = true;
    },
    (root: Node) => {
      root.depth_truncated = true;
    },
    (root: Node) => {
      root.self_weight = null;
    },
  ]) {
    const root = node('entry', [node('work', [], 10)]);
    alter(root);
    assert.equal(commonChain([root]).ancestors.length, 0);
  }
});
