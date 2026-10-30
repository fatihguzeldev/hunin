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

type PrivateStorage = Pick<Storage, 'getItem' | 'setItem' | 'removeItem'>;

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
