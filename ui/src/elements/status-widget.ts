import { parsePrometheusText, type ParsedMetrics } from '../lib/metrics';

const POLL_INTERVAL_MS = 5000;

/**
 * `<tellurion-status-widget>` — polls `/metrics` and renders three counters
 * parsed out of the Prometheus text (see `lib/metrics.ts`): total requests
 * served, average request latency, and process resident memory. No chart
 * library, no framework — a tiny self-contained panel, same contract as
 * every other element in this UI.
 */
export class TellurionStatusWidget extends HTMLElement {
  #timer: ReturnType<typeof setInterval> | null = null;

  connectedCallback(): void {
    this.innerHTML = `
      <dl class="status-widget__stats">
        <div class="status-widget__stat">
          <dt>Requests served</dt>
          <dd data-field="requests">&mdash;</dd>
        </div>
        <div class="status-widget__stat">
          <dt>Avg latency</dt>
          <dd data-field="latency">&mdash;</dd>
        </div>
        <div class="status-widget__stat">
          <dt>Resident memory</dt>
          <dd data-field="rss">&mdash;</dd>
        </div>
      </dl>
      <p class="status-widget__error" data-field="error" hidden></p>
    `;
    void this.#poll();
    this.#timer = setInterval(() => void this.#poll(), POLL_INTERVAL_MS);
  }

  disconnectedCallback(): void {
    if (this.#timer !== null) {
      clearInterval(this.#timer);
      this.#timer = null;
    }
  }

  async #poll(): Promise<void> {
    try {
      const response = await fetch('/metrics');
      if (!response.ok) {
        throw new Error(`GET /metrics failed: ${response.status} ${response.statusText}`);
      }
      const text = await response.text();
      this.#render(parsePrometheusText(text));
      this.#setError(null);
    } catch (error) {
      this.#setError(error instanceof Error ? error.message : String(error));
    }
  }

  #render(metrics: ParsedMetrics): void {
    this.#field('requests').textContent = metrics.totalRequests.toFixed(0);
    this.#field('latency').textContent =
      metrics.avgLatencyMs === null ? 'n/a' : `${metrics.avgLatencyMs.toFixed(1)} ms`;
    this.#field('rss').textContent =
      metrics.residentMemoryBytes === null
        ? 'n/a (not reported on this platform)'
        : `${(metrics.residentMemoryBytes / (1024 * 1024)).toFixed(1)} MB`;
  }

  #setError(message: string | null): void {
    const el = this.#field('error');
    el.hidden = message === null;
    el.textContent = message ?? '';
  }

  #field(name: string): HTMLElement {
    const el = this.querySelector<HTMLElement>(`[data-field="${name}"]`);
    if (!el) throw new Error(`status widget is missing its "${name}" field`);
    return el;
  }
}

customElements.define('tellurion-status-widget', TellurionStatusWidget);
