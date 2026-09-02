import maplibregl from 'maplibre-gl';
import {
  collectionsWithFeatures,
  fetchDefaultCollections,
  fetchItems,
  type CollectionSummary,
  type FeatureCollectionResponse,
} from '../lib/api';
import { createMap, fitToExtent } from '../lib/map';

const ITEMS_SOURCE_ID = 'tellurion-items';
const PAGE_LIMIT = 50;

/**
 * `<tellurion-features-panel>` — the OGC API Features browser: lists
 * collections that expose an `items` link, fetches a page of GeoJSON
 * features for the selected one, renders them as a MapLibre GeoJSON
 * source, and pages forward through the server's keyset `next` link.
 */
export class TellurionFeaturesPanel extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #select!: HTMLSelectElement;
  #list!: HTMLUListElement;
  #status!: HTMLElement;
  #nextButton!: HTMLButtonElement;
  #nextHref: string | null = null;
  #collections: CollectionSummary[] = [];
  #accumulatedFeatures: GeoJSON.Feature[] = [];

  connectedCallback(): void {
    this.innerHTML = `
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
        <button type="button" data-action="next" disabled>Load next page</button>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
      <ul class="panel__feature-list" data-field="list"></ul>
    `;

    this.#select = this.querySelector<HTMLSelectElement>('[data-field="collection"]')!;
    this.#list = this.querySelector<HTMLUListElement>('[data-field="list"]')!;
    this.#status = this.querySelector<HTMLElement>('[data-field="status"]')!;
    this.#nextButton = this.querySelector<HTMLButtonElement>('[data-action="next"]')!;

    const mapContainer = this.querySelector<HTMLElement>('[data-field="map"]')!;
    this.#map = createMap(mapContainer);
    this.#map.on('load', () => {
      this.#map!.addSource(ITEMS_SOURCE_ID, {
        type: 'geojson',
        data: { type: 'FeatureCollection', features: [] },
      });
      this.#map!.addLayer({
        id: 'items-fill',
        type: 'fill',
        source: ITEMS_SOURCE_ID,
        filter: ['==', ['geometry-type'], 'Polygon'],
        paint: { 'fill-color': '#3388ff', 'fill-opacity': 0.4 },
      });
      this.#map!.addLayer({
        id: 'items-line',
        type: 'line',
        source: ITEMS_SOURCE_ID,
        filter: ['==', ['geometry-type'], 'LineString'],
        paint: { 'line-color': '#3366cc', 'line-width': 2 },
      });
      this.#map!.addLayer({
        id: 'items-point',
        type: 'circle',
        source: ITEMS_SOURCE_ID,
        filter: ['==', ['geometry-type'], 'Point'],
        paint: { 'circle-color': '#cc3366', 'circle-radius': 4 },
      });
    });

    this.#select.addEventListener('change', () => void this.#loadFirstPage());
    this.#nextButton.addEventListener('click', () => void this.#loadNextPage());

    void this.#init();
  }

  disconnectedCallback(): void {
    this.#map?.remove();
    this.#map = null;
  }

  async #init(): Promise<void> {
    try {
      const response = await fetchDefaultCollections();
      this.#collections = collectionsWithFeatures(response);
      this.#select.replaceChildren(
        ...this.#collections.map((c) => {
          const option = document.createElement('option');
          option.value = c.id;
          option.textContent = c.id;
          return option;
        }),
      );
      if (this.#collections.length === 0) {
        this.#setStatus('no collections expose a features (items) lane');
        return;
      }
      await this.#loadFirstPage();
    } catch (error) {
      this.#setStatus(error instanceof Error ? error.message : String(error), true);
    }
  }

  async #loadFirstPage(): Promise<void> {
    const collection = this.#selectedCollection();
    if (!collection) return;
    const itemsLink = collection.links.find((l) => l.rel === 'items');
    if (!itemsLink) return;

    fitToExtent(this.#map!, collection.extent);
    const separator = itemsLink.href.includes('?') ? '&' : '?';
    await this.#loadPage(`${itemsLink.href}${separator}limit=${PAGE_LIMIT}`);
  }

  async #loadNextPage(): Promise<void> {
    if (this.#nextHref) {
      await this.#loadPage(this.#nextHref, true);
    }
  }

  async #loadPage(href: string, append = false): Promise<void> {
    try {
      this.#setStatus('loading…');
      const page = await fetchItems(href);
      this.#nextHref = page.links.find((l) => l.rel === 'next')?.href ?? null;
      this.#nextButton.disabled = this.#nextHref === null;
      this.#renderFeatures(page, append);
      const matched = page.numberMatched !== undefined ? ` of ${page.numberMatched} matched` : '';
      this.#setStatus(`${page.numberReturned} feature(s) returned${matched}`);
    } catch (error) {
      this.#setStatus(error instanceof Error ? error.message : String(error), true);
    }
  }

  #renderFeatures(page: FeatureCollectionResponse, append: boolean): void {
    this.#accumulatedFeatures = append
      ? [...this.#accumulatedFeatures, ...page.features]
      : page.features;

    const source = this.#map?.getSource(ITEMS_SOURCE_ID) as maplibregl.GeoJSONSource | undefined;
    source?.setData({ type: 'FeatureCollection', features: this.#accumulatedFeatures });

    const items = page.features.map((feature) => {
      const li = document.createElement('li');
      const id = feature.id ?? (feature.properties as Record<string, unknown> | null)?.id;
      li.textContent = id !== undefined ? `feature ${String(id)}` : 'feature (no id)';
      return li;
    });
    if (append) {
      this.#list.append(...items);
    } else {
      this.#list.replaceChildren(...items);
    }
  }

  #selectedCollection(): CollectionSummary | undefined {
    return this.#collections.find((c) => c.id === this.#select.value);
  }

  #setStatus(message: string, isError = false): void {
    this.#status.textContent = message;
    this.#status.classList.toggle('panel__status--error', isError);
  }
}

customElements.define('tellurion-features-panel', TellurionFeaturesPanel);
