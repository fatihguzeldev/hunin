type Child = Node | string | number | null | undefined | false;

/** All recording content enters the DOM as text, never as HTML. */
export function el<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attributes: Record<string, string | number | boolean | undefined> = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) {
    if (value === undefined || value === false) continue;
    node.setAttribute(name, value === true ? '' : String(value));
  }
  append(node, ...children);
  return node;
}

export function append(parent: Node, ...children: Child[]): void {
  for (const child of children) {
    if (child !== null && child !== undefined && child !== false) {
      parent.appendChild(child instanceof Node ? child : document.createTextNode(String(child)));
    }
  }
}

export function button(label: string, action: () => void, className = 'button'): HTMLButtonElement {
  const node = el('button', { type: 'button', class: className }, label);
  node.addEventListener('click', action);
  return node;
}

export function field(label: string, input: HTMLElement): HTMLLabelElement {
  return el('label', { class: 'field' }, el('span', {}, label), input);
}

export function empty(title: string, detail: string, action?: HTMLButtonElement): HTMLElement {
  return el('div', { class: 'empty', role: 'status' }, el('h2', {}, title), el('p', {}, detail), action);
}

export function errorView(error: unknown, retry: () => void): HTMLElement {
  const message = error instanceof Error ? error.message : String(error);
  return el('div', { class: 'error-state', role: 'alert' },
    el('h2', {}, 'Unable to load this view'), el('p', {}, message), button('Try again', retry));
}

export function loading(label: string): HTMLElement {
  return el('div', { class: 'loading', role: 'status', 'aria-busy': true },
    el('span', { class: 'pulse' }), label);
}

export function badge(value: string, tone = ''): HTMLElement {
  return el('span', { class: `badge ${tone}` }, value.replaceAll('_', ' '));
}
