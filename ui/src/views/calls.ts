import { countForm } from '../calls/form';
import { countReport } from '../calls/report';
import { operation } from '../api';
import type {
  OperationInputs,
  OperationOutputs,
} from '../operations.generated';
import { appendCountPage, CountActivity, CountAttempt } from '../call-counts';
import { button, el, errorView, loading } from '../dom';
import { linkedRequest } from '../requests';
import type { ViewContext } from '../workspace';

type CountReport = OperationOutputs['calls.query'];
type CountParams = OperationInputs['calls.measure'];

/** Count observations belong to a separate recording, never to sampled stack weights. */
export async function calls(context: ViewContext): Promise<HTMLElement> {
  const state = context.investigation.calls;
  let sourceRunId = context.run.id;
  const form = countForm(context, {
    sourceRunId: () => sourceRunId,
    inspect: () => {
      void inspectPlan();
    },
  });
  const { parameters, renderPlan, planBody, feedback, measure } = form;
  const reportBody = el('div', { class: 'stack', 'aria-live': 'polite' });
  let report: CountReport | undefined;
  let request = 0;
  let pending: AbortController | undefined;
  let poll: ReturnType<typeof setTimeout> | undefined;
  let planning = 0;
  const activity = new CountActivity();
  const attempt = new CountAttempt();
  async function inspectPlan(): Promise<void> {
    const generation = ++planning;
    try {
      const params = parameters();
      planBody.replaceChildren(loading('Checking the counting method…'));
      const result = await operation('calls.plan', params, context.signal);
      if (generation === planning && !context.signal.aborted)
        renderPlan(result.plan);
    } catch (error) {
      if (generation === planning && !context.signal.aborted)
        planBody.replaceChildren(
          errorView(error, () => {
            void inspectPlan();
          }),
        );
    }
  }
  function syncBusy(): void {
    form.setBusy(activity.busy);
  }
  async function load(runId: string, cursor?: string): Promise<void> {
    if (activity.runId && activity.runId !== runId) {
      runId = activity.runId;
      cursor = undefined;
    }
    clearTimeout(poll);
    pending?.abort();
    pending = new AbortController();
    const linked = linkedRequest(context.signal, pending.signal);
    const generation = ++request;
    try {
      const next = await operation(
        'calls.query',
        { run_id: runId, cursor, limit: 100 },
        linked.signal,
      );
      if (linked.signal.aborted || generation !== request) return;
      report = cursor && report ? appendCountPage(report, next) : next;
      if (report.parent_run_id && sourceRunId !== report.parent_run_id) {
        sourceRunId = report.parent_run_id;
        void inspectPlan();
      }
      if (report.parent_run_id) {
        state.measurementRunId = report.run_id;
        context.persist();
      }
      renderReport(report);
      activity.observed(report.run_id, report.status);
      syncBusy();
      if (report.status === 'running')
        poll = setTimeout(() => {
          void load(report!.run_id);
        }, 1000);
    } catch (error) {
      if (!linked.signal.aborted && generation === request) {
        reportBody.replaceChildren(
          errorView(error, () => {
            void load(runId);
          }),
        );
        const child = activity.runId;
        if (child)
          reportBody.append(
            button('Cancel counting run', () => {
              void cancelRun(child).catch(() => {});
            }),
          );
      }
    } finally {
      linked.finish();
    }
  }
  async function cancelRun(runId: string): Promise<void> {
    try {
      await operation('runs.cancel', { run_id: runId });
      if (!context.signal.aborted) await load(runId);
    } catch (error) {
      if (!context.signal.aborted)
        feedback.replaceChildren(
          errorView(error, () => {
            void cancelRun(runId).catch(() => {});
          }),
        );
      throw error;
    }
  }
  function renderReport(data: CountReport): void {
    reportBody.replaceChildren(
      countReport(data, {
        cancel: () => cancelRun(data.run_id),
        refresh: () => {
          void load(data.run_id);
        },
        more: (cursor) => {
          void load(data.run_id, cursor);
        },
      }),
    );
  }
  form.element.addEventListener('submit', (event) => {
    event.preventDefault();
    if (!activity.begin()) return;
    syncBusy();
    planning++;
    feedback.replaceChildren(
      loading('Starting a separate counting recording…'),
    );
    let params: CountParams;
    try {
      params = parameters();
      params = attempt.request(params);
    } catch (error) {
      feedback.replaceChildren(errorView(error, () => measure.click()));
      activity.rejected();
      syncBusy();
      return;
    }
    void operation('calls.measure', params)
      .then((result) => {
        activity.accepted(result.run?.id);
        attempt.accepted();
        if (result.run) state.measurementRunId = result.run.id;
        if (context.signal.aborted) return;
        context.persist();
        feedback.replaceChildren(
          result.reused
            ? el(
                'p',
                { class: 'notice' },
                'Reopened the retained counting recording for this request.',
              )
            : el('span'),
        );
        renderPlan(result.plan);
        if (!result.run) {
          syncBusy();
          return;
        }
        void load(result.run.id);
      })
      .catch((error) => {
        if (!context.signal.aborted) {
          feedback.replaceChildren(errorView(error, () => measure.click()));
          activity.rejected();
          syncBusy();
        }
      });
  });
  context.signal.addEventListener(
    'abort',
    () => {
      clearTimeout(poll);
      pending?.abort();
    },
    { once: true },
  );
  const body = el(
    'div',
    { class: 'stack' },
    el(
      'div',
      { class: 'view-title' },
      el('h2', {}, 'Function calls'),
      el(
        'p',
        {},
        'Automatic counting · separate execution · explicit collection scope',
      ),
    ),
    form.element,
    reportBody,
  );
  reportBody.replaceChildren(loading('Loading retained function counts…'));
  await Promise.all([
    load(state.measurementRunId ?? context.run.id),
    inspectPlan(),
  ]);
  return body;
}
