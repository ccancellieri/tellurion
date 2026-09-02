import maplibregl from 'maplibre-gl';
import {
  DEFAULT_CATALOG_ID,
  DEFAULT_TENANT_ID,
  fetchDefaultCollections,
  type CollectionSummary,
} from '../lib/api';
import { createMap, fitToExtent } from '../lib/map';
import { buildTileUrlTemplate } from '../lib/tile-url';

const SOURCE_ID = 'tellurion-png';

/**
 * `<tellurion-png-panel>` — the same tile route as the vector panel, but
 * negotiated to the raster lane (`?f=png` / `.png` suffix — see the
 * server's `negotiate_format`) via `buildTileUrlTemplate(..., 'png')`, and
 * added as a MapLibre raster source instead of a vector one. Proves the
 * MVT-first PNG lane end to end: the same adapter, a different format
 * argument.
 */
export class TellurionPngPanel extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #select!: HTMLSelectElement;
  #status!: HTMLElement;
  #collections: CollectionSummary[] = [];

  connectedCallback(): void {
    this.innerHTML = `
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <div class="panel__map" data-field="map"></div>
    `;

    this.#select = this.querySelector<HTMLSelectElement>('[data-field="collection"]')!;
    this.#status = this.querySelector<HTMLElement>('[data-field="status"]')!;
    const mapContainer = this.querySelector<HTMLElement>('[data-field="map"]')!;
    this.#map = createMap(mapContainer);
    this.#map.on('load', () => this.#loadSelected());

    this.#select.addEventListener('change', () => this.#loadSelected());

    void this.#init();
  }

  disconnectedCallback(): void {
    this.#map?.remove();
    this.#map = null;
  }

  async #init(): Promise<void> {
    try {
      const response = await fetchDefaultCollections();
      this.#collections = response.collections;
      this.#select.replaceChildren(
        ...this.#collections.map((c) => {
          const option = document.createElement('option');
          option.value = c.id;
          option.textContent = c.id;
          return option;
        }),
      );
      if (this.#collections.length === 0) {
        this.#status.textContent = 'no collections available';
        return;
      }
      if (this.#map?.loaded()) this.#loadSelected();
    } catch (error) {
      this.#status.textContent = error instanceof Error ? error.message : String(error);
    }
  }

  #loadSelected(): void {
    const map = this.#map;
    if (!map) return;
    const collection = this.#collections.find((c) => c.id === this.#select.value);
    if (!collection) return;

    if (map.getLayer('png-raster')) map.removeLayer('png-raster');
    if (map.getSource(SOURCE_ID)) map.removeSource(SOURCE_ID);

    map.addSource(SOURCE_ID, {
      type: 'raster',
      tiles: [
        buildTileUrlTemplate('', DEFAULT_TENANT_ID, DEFAULT_CATALOG_ID, collection.id, 'png'),
      ],
      tileSize: 256,
    });
    map.addLayer({ id: 'png-raster', type: 'raster', source: SOURCE_ID });

    fitToExtent(map, collection.extent);
    this.#status.textContent = `serving PNG tiles for "${collection.id}"`;
  }
}

customElements.define('tellurion-png-panel', TellurionPngPanel);
