import type { Project, ProjectCatalog } from '../operations.generated.js';

/** A document owns one project. Switching projects performs a full navigation. */
export function projectFromUrl(href?: string): string | undefined {
  return href
    ? new URL(href).searchParams.get('project') || undefined
    : undefined;
}

export function projectUrl(href: string, projectId?: string): URL {
  const url = new URL(href);
  // An explicit empty selection opens the picker even when --project supplied a default.
  url.searchParams.set('project', projectId ?? '');
  url.searchParams.delete('run');
  url.searchParams.delete('view');
  url.hash = '';
  return url;
}

/** Recording navigation keeps the document's selected project and server. */
export function recordingUrl(href: string, runId: string, view: string): URL {
  const url = new URL(href);
  url.searchParams.set('run', runId);
  url.searchParams.set('view', view);
  url.hash = '';
  return url;
}

type PrivateStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

export function projectStorageScope(project: Project): string {
  return project.legacy
    ? `legacy:${project.store_path}`
    : `project:${project.id}`;
}

/** Namespace private preferences and read interest, never durable job authority. */
export function projectStorage(
  storage: PrivateStorage,
  scope: string,
): PrivateStorage {
  const prefix = `hunin.project.v1:${JSON.stringify(scope)}:`;
  return {
    getItem: (key) => storage.getItem(prefix + key),
    setItem: (key, value) => storage.setItem(prefix + key, value),
    removeItem: (key) => storage.removeItem(prefix + key),
  };
}

type ProjectSelection =
  | { kind: 'project'; project: Project }
  | { kind: 'direct'; storageScope: string }
  | { kind: 'redirect'; url: URL }
  | { kind: 'picker'; problem?: string };

/** Startup defaults only choose a URL. Requests always carry that URL's selection. */
export function resolveProjectSelection(
  catalog: ProjectCatalog,
  href: string,
): ProjectSelection {
  const url = new URL(href);
  const id = projectFromUrl(href);
  if (catalog.direct_store) {
    if (id)
      return {
        kind: 'picker',
        problem:
          'This server opens an explicit store. It cannot open the project in this URL.',
      };
    if (!catalog.storage_scope)
      throw new Error(
        'The local service did not return a direct-store identity.',
      );
    return { kind: 'direct', storageScope: catalog.storage_scope };
  }
  if (id) {
    const project = catalog.projects.find((project) => project.id === id);
    return project
      ? { kind: 'project', project }
      : {
          kind: 'picker',
          problem:
            'The project in this URL is not registered. Choose a project or register its store.',
        };
  }
  if (!url.searchParams.has('project') && catalog.selected_project_id) {
    if (
      !catalog.projects.some(
        (project) => project.id === catalog.selected_project_id,
      )
    )
      throw new Error('The startup project is not available in the registry.');
    url.searchParams.set('project', catalog.selected_project_id);
    return { kind: 'redirect', url };
  }
  return { kind: 'picker' };
}
