import { discoverStac, type StacEndpoint } from './lib/stac-api';

/** Capability discovery stays lightweight; the actual inspector is lazy-loaded. */
export class StacLaneDiscovery {
  endpoint: StacEndpoint | null = null;
  #context: string | null = null;
  #request: AbortController | null = null;
  #tab: HTMLButtonElement;
  #status: HTMLElement;
  #changed: () => void;

  constructor(tab: HTMLButtonElement, status: HTMLElement, changed: () => void) {
    this.#tab = tab;
    this.#status = status;
    this.#changed = changed;
  }

  async refresh(force = false): Promise<void> {
    const context = location.search;
    if (!force && this.#context === context) return;
    this.#request?.abort();
    const request = new AbortController();
    this.#request = request;
    this.#context = context;
    this.endpoint = null;
    this.#tab.hidden = true;
    this.#changed();
    this.#status.textContent = 'Discovering STAC search…';
    try {
      const endpoint = await discoverStac(context, location.origin, request.signal);
      if (this.#request !== request || request.signal.aborted) return;
      this.endpoint = endpoint;
      this.#tab.hidden = endpoint === null;
      this.#status.textContent = endpoint ? '' : 'GET STAC search is not advertised for the selected tenant and catalog.';
    } catch {
      if (this.#request !== request || request.signal.aborted) return;
      this.#context = null;
      this.#status.textContent = 'Could not discover STAC search. ';
      const retry = document.createElement('button');
      retry.type = 'button';
      retry.textContent = 'Retry STAC discovery';
      retry.onclick = () => void this.refresh(true);
      this.#status.append(retry);
    }
  }
}
