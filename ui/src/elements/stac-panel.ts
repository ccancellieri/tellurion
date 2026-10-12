import '../stac-panel.css';
import { discoverStac, fetchStacPage, stacContext, stacSearchHref, validateStacAssetPreview,
  type StacEndpoint, type StacFilters, type StacPage } from '../lib/stac-api';
import {
  STAC_ASSET_PREVIEW_COMPLETE_EVENT,
  STAC_ASSET_PREVIEW_EVENT,
  type StacAssetPreviewCompletion,
  type StacAssetPreviewRequest,
} from '../lib/stac-preview-events';

/** Read-only, one-page-at-a-time inspector. Assets remain navigation links. */
export class TellurionStacPanel extends HTMLElement {
  endpoint: StacEndpoint | null = null;
  previewAvailable = false;
  #request: AbortController | null = null;
  #nextHref: string | null = null;
  #status!: HTMLElement;
  #results!: HTMLElement;
  #next!: HTMLButtonElement;
  #form!: HTMLFormElement;
  #previewSequence = 0;
  #pendingPreviewId: string | null = null;
  #previewComplete = (event: Event): void => this.#receivePreviewCompletion(event);

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
    this.addEventListener(STAC_ASSET_PREVIEW_COMPLETE_EVENT, this.#previewComplete);
    void this.#load();
  }

  disconnectedCallback(): void {
    this.#request?.abort();
    this.#request = null;
    this.#nextHref = null;
    this.#pendingPreviewId = null;
    this.removeEventListener(STAC_ASSET_PREVIEW_COMPLETE_EVENT, this.#previewComplete);
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
      id.textContent = item.title ? `${item.title} · ${item.id}` : item.id;
      entry.append(id);
      if (item.collectionId) {
        const collection = document.createElement('p');
        collection.textContent = `Collection: ${item.collectionId}`;
        entry.append(collection);
      }
      if (item.itemHref) {
        const itemLink = document.createElement('a');
        itemLink.href = item.itemHref;
        itemLink.textContent = 'Open STAC Item';
        itemLink.target = '_blank';
        itemLink.rel = 'noopener noreferrer';
        entry.append(itemLink);
      }
      const time = document.createElement('p');
      time.textContent = item.time;
      const assets = document.createElement('ul');
      for (const asset of item.assets) {
        const row = document.createElement('li');
        if (asset.navigationHref) {
          const link = document.createElement('a');
          link.href = asset.navigationHref;
          link.textContent = asset.title;
          link.target = '_blank';
          link.rel = 'noopener noreferrer';
          link.dataset.assetLink = '';
          row.append(link);
        } else {
          const label = document.createElement('span');
          label.textContent = asset.title;
          row.append(label);
        }
        const metadata = document.createElement('p');
        metadata.textContent = [asset.mediaType, asset.roles.length ? `roles: ${asset.roles.join(', ')}` : null]
          .filter(Boolean).join(' · ') || `Asset key: ${asset.key}`;
        row.append(metadata);
        if (asset.preview.state === 'eligible' && this.previewAvailable) {
          const eligiblePreview = asset.preview;
          if (eligiblePreview.descriptor.assetDescription) {
            const statement = document.createElement('p');
            statement.textContent = `Asset description (catalog supplied): ${eligiblePreview.descriptor.assetDescription}`;
            row.append(statement);
          }
          const preview = document.createElement('button');
          preview.type = 'button';
          preview.dataset.action = 'preview-asset';
          preview.textContent = 'Preview through temporary source';
          preview.disabled = this.#pendingPreviewId !== null;
          preview.addEventListener('click', () => this.#requestPreview(eligiblePreview));
          row.append(preview);
        } else {
          const reason = document.createElement('p');
          reason.textContent = asset.preview.state === 'eligible'
            ? 'Temporary-source preview is unavailable on this server. Enable the public-demo server feature to use it.'
            : asset.preview.reason;
          row.append(reason);
        }
        assets.append(row);
      }
      if (item.assets.length === 0) assets.textContent = 'No navigable HTTP(S) assets.';
      entry.append(time, assets);
      this.#results.append(entry);
    }
    this.#status.textContent = [page.items.length ? `${page.items.length} STAC items on this page.` : 'No STAC items matched.',
      page.truncated ? 'Display truncated: at most 10 items, 16 asset entries per item and 256 characters per label.' : '',
      page.unsupportedNext ? 'Next page requires an unsupported request; only same-origin GET links without extra headers or bodies are supported.' : '',
    ].filter(Boolean).join(' ');
  }

  #requestPreview(preview: Extract<import('../lib/stac-api').StacAssetPreviewDecision, { state: 'eligible' }>): void {
    if (this.#pendingPreviewId) return;
    const descriptor = validateStacAssetPreview(preview.descriptor);
    if (!descriptor) {
      this.#status.textContent = 'This asset preview is no longer valid. Search again.';
      this.#status.classList.add('panel__status--error');
      return;
    }
    const requestId = `stac-preview-${++this.#previewSequence}`;
    this.#pendingPreviewId = requestId;
    this.#setPreviewDisabled(true);
    this.#status.textContent = 'Opening this asset in the temporary source workflow…';
    const detail: StacAssetPreviewRequest = { requestId, descriptor };
    this.dispatchEvent(new CustomEvent(STAC_ASSET_PREVIEW_EVENT, { bubbles: true, composed: true, detail }));
  }

  #receivePreviewCompletion(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const detail = event.detail as Partial<StacAssetPreviewCompletion> | null;
    if (!detail || detail.requestId !== this.#pendingPreviewId ||
        !['accepted', 'busy', 'invalid', 'failed'].includes(detail.result ?? '')) return;
    this.#pendingPreviewId = null;
    this.#setPreviewDisabled(false);
    this.#status.classList.toggle('panel__status--error', detail.result !== 'accepted');
    this.#status.textContent = detail.result === 'accepted'
      ? 'Asset opened in the temporary source workflow.'
      : detail.result === 'busy'
        ? 'Discard the current temporary source before previewing another STAC asset.'
        : detail.result === 'invalid'
          ? 'This asset preview is no longer valid. Search again.'
          : 'The temporary source workflow could not inspect this asset.';
  }

  #setPreviewDisabled(disabled: boolean): void {
    this.querySelectorAll<HTMLButtonElement>('[data-action="preview-asset"]')
      .forEach((button) => { button.disabled = disabled; });
  }
}

customElements.define('tellurion-stac-panel', TellurionStacPanel);
