import { badge, el, empty, errorView, loading } from '../dom.js';
import {
  isProjectCatalog,
  type ProjectCatalog,
} from '../operations.generated.js';
import {
  projectUrl,
  projectStorageScope,
  resolveProjectSelection,
} from './context.js';
import './selection.css';
import { themeControl } from '../design/theme';

export type WorkspaceProject = {
  name: string;
  storageScope: string;
  pickerHref?: string;
  storePath: string;
  legacy: boolean;
};

function picker(catalog: ProjectCatalog, problem?: string): HTMLElement {
  const content = el(
    'main',
    { class: 'project-picker', id: 'main' },
    el(
      'div',
      { class: 'project-heading' },
      el(
        'a',
        { class: 'picker-wordmark', href: projectUrl(location.href).href },
        'hunin',
      ),
      el('h1', {}, 'Projects'),
      themeControl(),
    ),
  );
  if (problem)
    content.append(
      el('p', { class: 'project-problem', role: 'alert' }, problem),
    );
  if (catalog.direct_store) {
    content.append(
      el(
        'a',
        {
          class: 'button project-open-direct',
          href: projectUrl(location.href).href,
        },
        'Open this server’s store',
      ),
    );
    return content;
  }
  const list = el('ul', { class: 'project-list', 'aria-label': 'Projects' });
  for (const project of catalog.projects) {
    list.append(
      el(
        'li',
        {},
        el(
          'a',
          {
            class: 'project-link',
            href: projectUrl(location.href, project.id).href,
          },
          el(
            'div',
            { class: 'project-title' },
            el('strong', {}, project.name),
            project.legacy && badge('Legacy store'),
          ),
          project.workspace_roots.length
            ? el(
                'span',
                { class: 'project-workspaces' },
                project.workspace_roots.join(' · '),
              )
            : undefined,
          project.workspace_roots.length
            ? null
            : el('code', { class: 'muted' }, project.store_path),
          project.legacy
            ? el(
                'span',
                { class: 'muted' },
                'Existing shared recordings remain together in this store.',
              )
            : undefined,
        ),
      ),
    );
  }
  content.append(
    catalog.projects.length
      ? list
      : empty(
          'No projects yet',
          'Create a project from the terminal, then refresh this page.',
        ),
  );
  content.append(
    el(
      'details',
      { class: 'project-guidance', 'aria-label': 'Add a project' },
      el('summary', {}, 'Add a project'),
      el(
        'p',
        { class: 'muted' },
        'Create an independent store and associate a workspace:',
      ),
      el(
        'pre',
        {},
        'hunin projects create my-project --workspace /path/to/project',
      ),
      el(
        'p',
        { class: 'muted' },
        'Existing Hunin stores can be registered without moving their recordings:',
      ),
      el(
        'pre',
        {},
        'hunin projects register my-project --store /path/to/store --workspace /path/to/project',
      ),
      el(
        'p',
        { class: 'muted' },
        'Use this server’s --registry-dir when it uses a custom registry. Refresh the page after registration.',
      ),
    ),
  );
  return content;
}

/** No recording operation runs until an explicit project (or direct store) is resolved. */
export async function selectProject(
  root: HTMLElement,
  mount: (project: WorkspaceProject) => void,
): Promise<void> {
  root.replaceChildren(loading('Loading projects…'));
  try {
    const response = await fetch('/api/projects', {
      credentials: 'same-origin',
    });
    if (!response.ok)
      throw new Error(`Unable to load projects (HTTP ${response.status}).`);
    const catalog: unknown = await response.json();
    if (!isProjectCatalog(catalog))
      throw new Error('The local service returned an invalid project catalog.');
    const selected = resolveProjectSelection(catalog, location.href);
    if (selected.kind === 'redirect') {
      location.replace(selected.url);
      return;
    }
    if (selected.kind === 'picker') {
      document.title = 'Projects · hunin';
      root.replaceChildren(picker(catalog, selected.problem));
      return;
    }
    const project: WorkspaceProject =
      selected.kind === 'project'
        ? {
            name: selected.project.name,
            storageScope: projectStorageScope(selected.project),
            pickerHref: projectUrl(location.href).href,
            storePath: selected.project.store_path,
            legacy: selected.project.legacy,
          }
        : {
            name: 'Direct store',
            storageScope: `direct:${selected.storageScope}`,
            storePath: selected.storageScope,
            legacy: false,
          };
    document.title = `${project.name} · hunin`;
    mount(project);
  } catch (error) {
    root.replaceChildren(
      errorView(error, () => void selectProject(root, mount)),
    );
  }
}
