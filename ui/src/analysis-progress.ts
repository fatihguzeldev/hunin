/** Observe an attempt's status transition; a row absent from a page is not completion. */
export class AnalysisProgress {
  private running = new Set<string>();

  observe(attempts: { id?: string; status: string }[]): boolean {
    let changed = false;
    for (const attempt of attempts) {
      if (!attempt.id) continue;
      if (attempt.status === 'running') this.running.add(attempt.id);
      else if (this.running.delete(attempt.id)) changed = true;
    }
    return changed;
  }
}
