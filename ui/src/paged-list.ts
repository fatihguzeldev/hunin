import { button, el, errorView, loading } from './dom';
import { PagedQuery, type Page } from './pagination';

export function pagedList<T>(options: {
  label: string;
  nextLabel: string;
  load: (cursor?: string) => Promise<Page<T>>;
  render: (items: T[]) => HTMLElement;
  pollWhen?: (items: T[]) => boolean;
}): { element: HTMLElement; refresh: (reset?: boolean) => Promise<void> } {
  const element = el('div', { class: 'stack' });
  const query = new PagedQuery(options.load);
  let poll: ReturnType<typeof setTimeout> | undefined;

  function navigation(): HTMLElement {
    return el('div', { class: 'toolbar', 'aria-label': `${options.label} pages` },
      query.hasPrevious ? button('Previous page', () => { void update(() => query.previous()); }, 'button subtle') : null,
      query.page.next_cursor ? button(options.nextLabel, () => { void update(() => query.next()); }, 'button subtle') : null,
      query.hasPrevious ? button('First page', () => { void refresh(true); }, 'button subtle') : null);
  }

  async function update(request: () => Promise<boolean>): Promise<void> {
    clearTimeout(poll);
    element.replaceChildren(loading(`Loading ${options.label}…`));
    try {
      if (!await request()) return;
      element.replaceChildren(options.render(query.page.items), navigation());
      if (options.pollWhen?.(query.page.items)) poll = setTimeout(() => {
        if (element.isConnected) void refresh();
      }, 1_500);
    } catch (error) {
      element.replaceChildren(errorView(error, () => { void update(request); }), navigation());
    }
  }

  async function refresh(reset = false): Promise<void> { await update(() => query.refresh(reset)); }
  return { element, refresh };
}
