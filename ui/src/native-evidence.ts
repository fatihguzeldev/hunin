import { el } from './dom';
import type { Observation } from './record.generated';
import { isModuleLifetime, nativeEvidenceRows } from './observation';

export function nativeEvidence(record: Observation): HTMLElement | null {
  const rows = nativeEvidenceRows(record);
  if (!rows.length) return null;
  return el(
    'div',
    { class: 'stack' },
    el('h3', {}, 'Native code instance evidence'),
    isModuleLifetime(record)
      ? el(
          'p',
          { class: 'notice' },
          'These are recorded mapping boundaries. Censored or observation-window bounds do not establish exact loader event times. A null window endpoint is unknown. A module lifetime does not measure time spent executing that module.',
        )
      : null,
    ...rows.map(([label, value]) =>
      el(
        'div',
        {},
        el('small', { class: 'metric-note' }, label),
        el('p', { class: 'mono source-label' }, value),
      ),
    ),
  );
}
