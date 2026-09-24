import '../stac-panel.css';
import { discoverStac, fetchStacPage, stacContext, stacSearchHref,
  type StacEndpoint, type StacFilters, type StacPage } from '../lib/stac-api';

/** Read-only, one-page-at-a-time inspector. Assets remain navigation links. */
export class TellurionStacPanel extends HTMLElement {
  endpoint: StacEndpoint | null = null;
  #request: AbortController | null = null;
  #nextHref: string | null = null;
  #status!: HTMLElement;
  #results!: HTMLElement;
  #next!: HTMLButtonElement;
  #form!: HTMLFormElement;

  connectedCallback(): void {
    this.innerHTML = `
      <section class="stac-inspector" aria-label="STAC search inspector">
        <h2>STAC search inspector</h2>
        <p data-field="context"></p>
        <p>Read-only search: ten items per page. Asset links open separately; assets are not fetched by this inspector.</p>
        <form class="panel__controls">
          <label>Collections (comma-separated)<input name="collections" maxlength="1024" autocomplete="off"></label>
          <label>Item IDs (comma-separated)<input name="ids" maxlength="1024" autocomplete="off"></label>
          <label>Date/time or interval<input name="datetime" maxlength="1024" placeholder="2026-01-01T00:00:00Z/.." autocomplete="off"></label>
          <button type="submit">Search STAC</button>
        </form>
        <p class="panel__status" data-field="status" role="status"></p>
        <ol data-field="results"></ol>
        <button type="button" data-action="next" disabled>Next page</button>
      </section>`;
    this.#status = this.querySelector('[data-field="status"]')!;
    this.#results = this.querySelector('[data-field="results"]')!;
    this.#next = this.querySelector('[data-action="next"]')!;
    this.#form = this.querySelector('form')!;
    this.#form.onsubmit = (event) => { event.preventDefault(); void this.#load(); };
    this.#next.onclick = () => { if (this.#nextHref) void this.#load(this.#nextHref); };
    void this.#load();
  }

  disconnectedCallback(): void {
    this.#request?.abort();
    this.#request = null;
    this.#nextHref = null;
  }

  async #load(nextHref?: string): Promise<void> {
    this.#request?.abort();
    const request = new AbortController();
    this.#request = request;
    this.#nextHref = null;
    this.#next.disabled = true;
    this.#results.replaceChildren();
    this.#status.textContent = 'Loading STAC items…';
    this.#status.classList.remove('panel__status--error');
    try {
      const context = stacContext(location.search);
      this.querySelector('[data-field="context"]')!.textContent = `Tenant: ${context.tenantId} · Catalog: ${context.catalogId}`;
      if (this.endpoint?.tenantId !== context.tenantId || this.endpoint.catalogId !== context.catalogId) {
        this.endpoint = null;
        nextHref = undefined;
        const endpoint = await discoverStac(location.search, location.origin, request.signal);
        if (this.#request !== request || request.signal.aborted) return;
        this.endpoint = endpoint;
      }
      if (!this.endpoint) {
        this.#status.textContent = 'GET STAC search is not advertised for this tenant and catalog.';
        return;
      }
      const filters: StacFilters = {};
      for (const name of ['collections', 'ids', 'datetime'] as const) {
        filters[name] = (this.#form.elements.namedItem(name) as HTMLInputElement).value;
      }
      const page = await fetchStacPage(nextHref ?? stacSearchHref(this.endpoint.searchHref, filters), location.origin, request.signal);
      if (this.#request !== request || request.signal.aborted) return;
      this.#render(page);
    } catch {
      if (this.#request !== request || request.signal.aborted) return;
      this.#status.textContent = 'Could not load STAC items. Check the context and filters, then search again.';
      this.#status.classList.add('panel__status--error');
    }
  }

  #render(page: StacPage): void {
    this.#nextHref = page.nextHref;
    this.#next.disabled = !page.nextHref;
    for (const item of page.items) {
      const entry = document.createElement('li');
      const id = document.createElement('h3');
      id.textContent = item.id;
      const time = document.createElement('p');
      time.textContent = item.time;
      const assets = document.createElement('ul');
      for (const asset of item.assets) {
        const row = document.createElement('li');
        const link = document.createElement('a');
        link.href = asset.href;
        link.textContent = asset.title;
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
        row.append(link);
        assets.append(row);
      }
      if (item.assets.length === 0) assets.textContent = 'No navigable HTTP(S) assets.';
      entry.append(id, time, assets);
      this.#results.append(entry);
    }
    this.#status.textContent = [page.items.length ? `${page.items.length} STAC items on this page.` : 'No STAC items matched.',
      page.truncated ? 'Display truncated: at most 10 items, 16 asset entries per item and 256 characters per label.' : '',
      page.unsupportedNext ? 'Next page requires an unsupported request; only same-origin GET links without extra headers or bodies are supported.' : '',
    ].filter(Boolean).join(' ');
  }
}

customElements.define('tellurion-stac-panel', TellurionStacPanel);
