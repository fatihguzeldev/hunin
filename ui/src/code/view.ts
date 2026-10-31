import { operation } from '../api';
import { badge, button, el, empty, errorView, field, loading } from '../dom';
import { icon } from '../design/icons';
import { numeric } from '../format';
import type { ViewContext } from '../investigation/view';
import type {
  CodeArtifactRef,
  CodeArtifactStatus,
  CodeInspection,
  OperationInputs,
  OperationOutputs,
} from '../operations.generated';
import { observationLabel, referenceIds } from '../observation';
import { linkedLifetime, linkedRequest } from '../requests';
import {
  mappingAnchors,
  mappingOccurrenceIndex,
  mappingScope,
} from './selection';
import { coverageText } from './coverage';
import { mappingPlans } from './plans';
import { mapperActions } from './mappers';
import { hexBytes, proofName } from './proof';

type Selected = NonNullable<OperationOutputs['profiles.inspect']['selected']>;

export async function mappedOrigins(
  context: ViewContext,
  selected: Selected,
  window: { start_us: number; end_us: number } | null,
  clockDomain: string | null,
  signal: AbortSignal,
): Promise<HTMLElement> {
  const evidence = await operation(
    'evidence.get',
    { run_id: context.run.id, ids: selected.evidence_ids },
    signal,
  );
  signal.throwIfAborted();
  const choices = mappingAnchors(evidence.records, selected);
  if (!choices.anchors.length)
    return empty(
      'No sampled frame to trace',
      selected.sample_count
        ? 'The selected function has no matching retained frame in its available evidence. Inspect its samples or choose another window.'
        : 'Choose a window containing samples for this function.',
    );
  const root = el('div', { class: 'stack' });
  const anchor = el('select', { 'aria-label': 'Sampled frame to trace' });
  for (const [index, value] of choices.anchors.entries())
    anchor.append(
      el(
        'option',
        { value: index },
        `Sample ${value.observation_id.slice(-8)} · frame ${value.frame_index + 1}${value.code_offset ? ` · ${value.code_offset}` : ''}`,
      ),
    );
  const scope = mappingScope(
    context.run.id,
    selected.id,
    window,
    clockDomain,
    selected.weight_unit,
  );
  anchor.value = String(
    mappingOccurrenceIndex(
      choices.anchors,
      scope,
      context.investigation.workbench.mappingSelection,
    ),
  );
  const content = el('div', { class: 'stack' });
  const inspection = el('div', {
    class: 'stack',
    role: 'region',
    'aria-label': 'Mapping evidence',
    'aria-live': 'polite',
    tabindex: -1,
  });
  function revealInspection(): void {
    inspection.focus({ preventScroll: true });
    inspection.scrollIntoView({ block: 'start' });
  }
  root.append(
    el(
      'div',
      { class: 'stack' },
      field('Sampled frame', anchor),
      choices.truncated
        ? el(
            'p',
            { class: 'metric-note' },
            `Showing ${choices.anchors.length} matching frame occurrences; the evidence list is limited.`,
          )
        : null,
      content,
      inspection,
    ),
  );
  let generation = 0;
  let current: AbortController | undefined;
  let detailRequest: AbortController | undefined;
  let detailGeneration = 0;
  let artifactMetadata = new Map<string, CodeArtifactStatus>();
  const unit = ['samples', 'microseconds', 'bytes', 'count'].includes(
    selected.weight_unit,
  )
    ? (selected.weight_unit as NonNullable<
        OperationInputs['code.inspect']['unit']
      >)
    : undefined;
  if (!unit)
    return empty(
      'Mapping coverage unavailable',
      `This profile uses an unsupported mapping weight unit: ${selected.weight_unit}.`,
    );
  function params(): OperationInputs['code.inspect'] {
    const value = choices.anchors[Number(anchor.value)];
    return {
      run_id: context.run.id,
      observation_id: value.observation_id,
      frame_index: value.frame_index,
      unit,
      clock_domain: clockDomain ?? undefined,
      ...(window ? { start_us: window.start_us, end_us: window.end_us } : {}),
      limit: 100,
    };
  }
  async function observation(id: string, lifetime: AbortSignal): Promise<void> {
    if (lifetime.aborted) return;
    detailRequest?.abort();
    detailRequest = new AbortController();
    const linked = linkedRequest(lifetime, detailRequest.signal);
    const epoch = ++detailGeneration;
    inspection.replaceChildren(loading('Loading mapping evidence…'));
    revealInspection();
    try {
      const data = await operation(
        'evidence.get',
        { run_id: context.run.id, ids: [id] },
        linked.signal,
      );
      if (linked.signal.aborted || epoch !== detailGeneration) return;
      const record = data.records.find((row) => row.id === id);
      if (!record) {
        inspection.replaceChildren(
          empty(
            'Evidence unavailable',
            'This observation is absent from the currently selected results.',
          ),
        );
        return;
      }
      const refs = referenceIds(record);
      inspection.replaceChildren(
        el(
          'section',
          { class: 'panel panel-body stack' },
          el('h3', {}, observationLabel(record)),
          badge(record.basis),
          el(
            'div',
            { class: 'toolbar' },
            ...refs.slice(0, 25).map((reference) =>
              button(
                `Inspect ${reference}`,
                () => {
                  void observation(reference, lifetime);
                },
                'button subtle small mono',
              ),
            ),
          ),
          refs.length > 25
            ? el(
                'p',
                { class: 'metric-note' },
                `${refs.length} references; the first 25 are linked here. All IDs remain in Observation details.`,
              )
            : null,
          el(
            'details',
            {},
            el('summary', {}, 'Observation details'),
            el('pre', {}, JSON.stringify(record, null, 2)),
          ),
        ),
      );
    } catch (error) {
      if (!linked.signal.aborted && epoch === detailGeneration)
        inspection.replaceChildren(
          errorView(error, () => {
            void observation(id, lifetime);
          }),
        );
    } finally {
      linked.finish();
    }
  }
  async function artifact(
    proof: CodeArtifactRef,
    lifetime: AbortSignal,
    offset = proof.offset ?? 0,
    encoding: 'hex' | 'utf8_lossy' = 'hex',
  ): Promise<void> {
    if (lifetime.aborted) return;
    detailRequest?.abort();
    detailRequest = new AbortController();
    const linked = linkedRequest(lifetime, detailRequest.signal);
    const epoch = ++detailGeneration;
    const metadata = artifactMetadata.get(proof.artifact_id);
    inspection.replaceChildren(loading('Loading retained proof…'));
    revealInspection();
    try {
      const end =
        proof.length === undefined
          ? undefined
          : (proof.offset ?? 0) + proof.length;
      const data = await operation(
        'artifacts.read',
        {
          run_id: context.run.id,
          artifact_id: proof.artifact_id,
          offset,
          encoding,
          limit: Math.min(5000, end === undefined ? 5000 : end - offset),
        },
        linked.signal,
      );
      if (linked.signal.aborted || epoch !== detailGeneration) return;
      const next = data.next_offset;
      const format = el(
        'select',
        { 'aria-label': 'Artifact proof representation' },
        el('option', { value: 'hex' }, 'Hex bytes'),
        el('option', { value: 'utf8_lossy' }, 'Text'),
      );
      format.value = encoding;
      format.addEventListener('change', () => {
        void artifact(
          proof,
          lifetime,
          offset,
          format.value as 'hex' | 'utf8_lossy',
        );
      });
      inspection.replaceChildren(
        el(
          'section',
          { class: 'panel panel-body stack' },
          el('h3', {}, 'Retained artifact proof'),
          el('p', {}, proofName(metadata?.name, 0)),
          el(
            'details',
            {},
            el('summary', {}, 'Artifact identity and declared range'),
            el(
              'pre',
              {},
              JSON.stringify(
                {
                  name: metadata?.name ?? null,
                  format: metadata?.format ?? null,
                  artifact_id: proof.artifact_id,
                  sha256: proof.sha256,
                  offset: proof.offset ?? 0,
                  length: proof.length ?? null,
                },
                null,
                2,
              ),
            ),
          ),
          el(
            'p',
            { class: 'metric-note' },
            `Byte offset 0x${offset.toString(16)}. ${data.encoding === 'hex' ? 'Exact bytes; their semantic meaning is not independently established.' : 'UTF-8 preview may replace binary or boundary bytes. Hex bytes preserves their exact representation.'}`,
          ),
          field('Representation', format),
          el(
            'pre',
            { class: data.encoding === 'hex' ? 'code-proof-bytes' : undefined },
            data.encoding === 'hex'
              ? hexBytes(data.content, offset)
              : data.content,
          ),
          next !== null && (end === undefined || next < end)
            ? button(
                'Next proof excerpt',
                () => {
                  void artifact(proof, lifetime, next, encoding);
                },
                'button subtle',
              )
            : null,
        ),
      );
    } catch (error) {
      if (!linked.signal.aborted && epoch === detailGeneration)
        inspection.replaceChildren(
          errorView(error, () => {
            void artifact(proof, lifetime, offset, encoding);
          }),
        );
    } finally {
      linked.finish();
    }
  }
  function draw(
    data: CodeInspection,
    lifetime: AbortSignal,
    inspectedParams: OperationInputs['code.inspect'],
  ): HTMLElement {
    const instances = new Map(data.instances.map((node) => [node.id, node]));
    artifactMetadata = new Map(
      data.input_artifacts.map((proof) => [proof.artifact_id, proof]),
    );
    function proofButton(
      proof: CodeArtifactRef,
      index: number,
    ): HTMLButtonElement {
      const metadata = artifactMetadata.get(proof.artifact_id);
      const action = button(
        `Inspect ${proofName(metadata?.name, index)}`,
        () => {
          void artifact(proof, lifetime);
        },
        'button subtle small',
      );
      if (metadata?.name) action.title = metadata.name;
      return action;
    }

    const body = el('div', { class: 'stack' }, el('h3', {}, 'Mapped origins'));
    const frame = data.anchor?.frame;
    if (data.anchor) {
      const association = data.anchor.association_status;
      const explanation = {
        inline:
          data.anchor.kind === 'sample'
            ? 'Code identity is part of the retained sampled frame.'
            : 'This observation declares the retained code identity.',
        derived:
          'Code identity comes from an accepted association. The retained sampled frame is unchanged.',
        ambiguous:
          'Accepted associations disagree. No effective code identity is selected.',
        unavailable: 'No applicable code identity is available for this frame.',
      }[association];
      body.append(
        el(
          'div',
          { class: 'stack' },
          badge(association),
          el('p', { class: 'metric-note' }, explanation),
        ),
      );
    }
    if (
      frame?.address ||
      data.anchor?.code_offset ||
      data.anchor?.code_address_basis
    )
      body.append(
        el(
          'p',
          { class: 'metric-note' },
          [
            frame?.address ? `Recorded address ${frame.address}` : null,
            frame?.code_offset
              ? `Recorded code coordinate ${frame.code_offset}`
              : null,
            data.anchor?.code_offset &&
            data.anchor.code_offset !== frame?.code_offset
              ? `Associated code coordinate ${data.anchor.code_offset}`
              : null,
            data.anchor?.code_address_basis
              ? `Coordinate basis: ${data.anchor.code_address_basis.replaceAll('_', ' ')}`
              : null,
          ]
            .filter(Boolean)
            .join(' · '),
        ),
      );
    for (const association of data.frame_mappings) {
      const mapping = association.frame_mapping;
      const target = instances.get(association.to);
      body.append(
        el(
          'section',
          { class: 'panel panel-body stack' },
          el('h4', {}, `Frame ${mapping.frame_index + 1} association`),
          badge(association.basis),
          button(
            target?.instance.label ||
              target?.instance.representation ||
              association.to,
            () => {
              void observation(association.to, lifetime);
            },
            'text-button',
          ),
          el('p', { class: 'metric-note' }, mapping.proof.method),
          el(
            'p',
            { class: 'metric-note' },
            mapping.proof.verification === 'semantic_checked'
              ? `Mapper reports a semantic check by ${mapping.proof.validator ?? 'unknown'}. This is the mapper’s claim.`
              : mapping.proof.verification === 'identity_checked'
                ? 'Artifact identity checked at publication; semantic correctness is separate.'
                : 'Mapper declaration; semantic correctness is not independently established.',
          ),
          button(
            'Inspect association evidence',
            () => {
              void observation(association.id, lifetime);
            },
            'button subtle small',
          ),
          ...mapping.proof.artifact_refs.map(proofButton),
          el(
            'details',
            {},
            el('summary', {}, 'Association coordinates and validity'),
            el(
              'pre',
              {},
              JSON.stringify(
                {
                  code_offset: mapping.code_offset,
                  code_address_basis: mapping.code_address_basis,
                  validity: mapping.validity,
                },
                null,
                2,
              ),
            ),
          ),
        ),
      );
    }
    const coverage = data.coverage;
    body.append(
      el(
        'p',
        { class: 'metric-note' },
        `Captured samples for this code instance${window ? ' in the selected window' : ''}.`,
      ),
    );
    body.append(
      el(
        'details',
        {},
        el('summary', {}, 'Coverage scope'),
        el(
          'p',
          { class: 'metric-note' },
          `Measured in ${coverage.unit}; each captured sample is counted once. This code-instance scope is independent of the selected function’s inclusive weight.`,
        ),
      ),
    );
    body.append(
      el(
        'div',
        { class: 'stack' },
        ...(['mapped', 'shared', 'unmapped', 'unknown'] as const).map((key) =>
          el(
            'div',
            { class: 'toolbar' },
            badge(key),
            el(
              'span',
              { class: 'mono' },
              coverage.status === 'unavailable'
                ? 'Sample count unknown · weight unavailable'
                : coverageText(coverage[key], coverage.unit),
            ),
          ),
        ),
      ),
    );
    if (coverage.status === 'complete')
      body.append(
        el(
          'p',
          { class: 'metric-note' },
          `${coverageText(coverage.total, coverage.unit)} total for this code instance.`,
        ),
      );
    if (coverage.status === 'unavailable')
      body.append(
        el(
          'p',
          { class: 'notice' },
          'Coverage is unavailable for this anchor. Unknown values are not zero.',
        ),
      );
    if (coverage.unaligned_sample_count)
      body.append(
        el(
          'p',
          { class: 'metric-note' },
          `${numeric(coverage.unaligned_sample_count)} samples are outside this selected clock domain and are counted separately.`,
        ),
      );
    if (data.truncated)
      body.append(
        el(
          'p',
          { class: 'notice' },
          'The mapping query reached its limit. The visible graph does not establish complete coverage.',
        ),
      );
    for (const unavailable of data.unavailable)
      body.append(
        el(
          'p',
          { class: 'notice' },
          `${unavailable.unavailable.state}: ${unavailable.unavailable.reason_code.replaceAll('_', ' ')}`,
        ),
      );
    if (
      !data.mappings.length &&
      !data.frame_mappings.length &&
      !data.diagnostics.length
    )
      body.append(
        el(
          'p',
          {},
          'No accepted code-origin mapping is available for this frame.',
        ),
      );
    for (const edge of data.mappings) {
      const from = instances.get(edge.from);
      const to = instances.get(edge.to);
      const mapping = edge.mapping;
      const tier =
        mapping.proof.verification === 'declared'
          ? 'Mapper declaration'
          : mapping.proof.verification === 'identity_checked'
            ? 'Artifact identity checked at publication'
            : 'Mapper reports a semantic check';
      body.append(
        el(
          'section',
          { class: 'panel panel-body stack' },
          el(
            'div',
            { class: 'toolbar' },
            button(
              from?.instance.label ||
                from?.instance.representation ||
                edge.from,
              () => {
                void observation(edge.from, lifetime);
              },
              'text-button',
            ),
            icon('arrow'),
            button(
              to?.instance.label || to?.instance.representation || edge.to,
              () => {
                void observation(edge.to, lifetime);
              },
              'text-button',
            ),
          ),
          el(
            'div',
            { class: 'toolbar' },
            badge(mapping.cardinality),
            badge(edge.basis),
            el('span', { class: 'metric-note' }, tier),
          ),
          el('p', { class: 'metric-note' }, mapping.proof.method),
          mapping.proof.verification === 'semantic_checked'
            ? el(
                'p',
                { class: 'metric-note' },
                `Validator: ${mapping.proof.validator ?? 'unknown'}. This records the mapper’s claim, not independent compiler correctness.`,
              )
            : null,
          el(
            'div',
            { class: 'toolbar' },
            button(
              'Inspect mapping evidence',
              () => {
                void observation(edge.id, lifetime);
              },
              'button subtle small',
            ),
            ...mapping.proof.artifact_refs.map(proofButton),
          ),
          el(
            'details',
            {},
            el('summary', {}, 'Mapping ranges and validity'),
            el(
              'pre',
              {},
              JSON.stringify(
                {
                  from_range: mapping.from_range,
                  to_range: mapping.to_range,
                  validity: mapping.validity,
                },
                null,
                2,
              ),
            ),
          ),
        ),
      );
    }
    for (const diagnostic of data.diagnostics)
      body.append(el('p', { class: 'metric-note' }, diagnostic));
    if (data.recording_diagnostics)
      body.append(
        el(
          'details',
          {},
          el('summary', {}, 'Recording diagnostics'),
          el(
            'p',
            { class: 'metric-note' },
            'Recording-wide messages may describe earlier collection or import stages. They do not establish this frame’s cause.',
          ),
          el(
            'ul',
            {},
            ...data.recording_diagnostics.messages.map((message) =>
              el('li', {}, message),
            ),
          ),
          data.recording_diagnostics.truncated
            ? el(
                'p',
                { class: 'metric-note' },
                'Additional recording messages are omitted.',
              )
            : null,
        ),
      );
    body.append(
      mappingPlans(() => inspectedParams, lifetime),
      mapperActions(context, inspectedParams, lifetime),
    );
    for (const node of data.instances)
      body.append(
        el(
          'details',
          {},
          el(
            'summary',
            {},
            `${node.instance.label || node.instance.representation} · ${node.instance.instance_kind === 'static' ? 'static unit' : 'runtime instance'}`,
          ),
          el(
            'div',
            { class: 'stack' },
            el(
              'p',
              { class: 'metric-note' },
              node.instance.runtime_instance_id ??
                'Static units do not declare a runtime lifetime.',
            ),
            node.instance.source
              ? el(
                  'p',
                  { class: 'metric-note mono' },
                  `${node.instance.source.path}${node.instance.source.line ? `:${node.instance.source.line}` : ''}${node.instance.source.column !== undefined && node.instance.source.column !== null ? ` · column ${node.instance.source.column}` : ''}`,
                )
              : null,
            node.instance.artifact
              ? button(
                  'Inspect retained code/source',
                  () => {
                    void artifact(node.instance.artifact!, lifetime);
                  },
                  'button subtle small',
                )
              : null,
            button(
              'Inspect instance evidence',
              () => {
                void observation(node.id, lifetime);
              },
              'button subtle small',
            ),
            el('pre', {}, JSON.stringify(node.instance, null, 2)),
          ),
        ),
      );
    for (const proof of data.input_artifacts.filter(
      (value) => value.status !== 'metadata_matches',
    ))
      body.append(
        el(
          'p',
          { class: 'notice' },
          `${proof.name ?? 'Artifact'}: identity mismatch.`,
        ),
      );
    if (data.input_artifacts.length)
      body.append(
        el(
          'details',
          {},
          el('summary', {}, 'Artifact identities'),
          ...data.input_artifacts.map((proof) =>
            el(
              'p',
              { class: 'metric-note' },
              `${proof.name ?? proof.artifact_id} · ${proof.status === 'metadata_matches' ? 'Metadata matches the declared identity' : 'Identity mismatch'}`,
            ),
          ),
        ),
      );
    return body;
  }
  async function load(): Promise<void> {
    if (signal.aborted) return;
    current?.abort();
    detailRequest?.abort();
    current = new AbortController();
    const lifetime = linkedLifetime(signal, current.signal);
    const inspectedParams = params();
    const epoch = ++generation;
    content.replaceChildren(loading('Tracing retained code origins…'));
    inspection.replaceChildren();
    try {
      const data = await operation('code.inspect', inspectedParams, lifetime);
      if (!lifetime.aborted && epoch === generation)
        content.replaceChildren(draw(data, lifetime, inspectedParams));
    } catch (error) {
      if (!lifetime.aborted && epoch === generation)
        content.replaceChildren(
          errorView(error, () => {
            void load();
          }),
        );
    }
  }
  anchor.addEventListener('change', () => {
    const selected = choices.anchors[Number(anchor.value)];
    context.investigation.workbench.mappingSelection = {
      scope,
      observationId: selected.observation_id,
      frameIndex: selected.frame_index,
    };
    context.persist('replace');
    void load();
  });
  await load();
  return root;
}
