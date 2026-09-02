import maplibregl from 'maplibre-gl';
import {
  DEFAULT_CATALOG_ID,
  DEFAULT_TENANT_ID,
  fetchDefaultCollections,
  fetchStyleDocument,
  fetchStyles,
  type CollectionSummary,
} from '../lib/api';
import { createMap, fitToExtent } from '../lib/map';
import { buildStyledTileUrlTemplate } from '../lib/tile-url';

const SOURCE_ID = 'tellurion-styled';

/**
 * `<tellurion-styled-panel>` — fetches a MapLibre Style JSON document from
 * `/{tenant}/styles/catalogs/{catalog}/styles/{styleId}` and applies its layer
 * paint to the collection's styled raster lane
 * (`/{tenant}/tiles/catalogs/{catalog}/collections/{cid}/styles/{styleId}/map/tiles/...`,
 * raster-only per the server's `styled_tile` handler). The style document
 * itself only round-trips the collection's raster tiles here; a future
 * slice could also apply its `layers` directly to a vector source once one
 * style is guaranteed to cover more than one collection's source-layer
 * name.
 */
export class TellurionStyledPanel extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #collectionSelect!: HTMLSelectElement;
  #styleSelect!: HTMLSelectElement;
  #status!: HTMLElement;
  #styleName!: HTMLElement;
  #collections: CollectionSummary[] = [];
  #styleIds: string[] = [];

  connectedCallback(): void {
    this.innerHTML = `
      <div class="panel__controls">
        <label>
          Collection
          <select data-field="collection"></select>
        </label>
        <label>
          Style
          <select data-field="style"></select>
        </label>
      </div>
      <p class="panel__status" data-field="status"></p>
      <p class="panel__style-name" data-field="style-name"></p>
      <div class="panel__map" data-field="map"></div>
    `;

    this.#collectionSelect = this.querySelector<HTMLSelectElement>('[data-field="collection"]')!;
    this.#styleSelect = this.querySelector<HTMLSelectElement>('[data-field="style"]')!;
    this.#status = this.querySelector<HTMLElement>('[data-field="status"]')!;
    this.#styleName = this.querySelector<HTMLElement>('[data-field="style-name"]')!;
    const mapContainer = this.querySelector<HTMLElement>('[data-field="map"]')!;
    this.#map = createMap(mapContainer);
    this.#map.on('load', () => void this.#loadSelected());

    this.#collectionSelect.addEventListener('change', () => void this.#loadSelected());
    this.#styleSelect.addEventListener('change', () => void this.#loadSelected());

    void this.#init();
  }

  disconnectedCallback(): void {
    this.#map?.remove();
    this.#map = null;
  }

  async #init(): Promise<void> {
    try {
      const [collectionsResponse, stylesResponse] = await Promise.all([
        fetchDefaultCollections(),
        fetchStyles(),
      ]);
      this.#collections = collectionsResponse.collections;
      this.#styleIds = stylesResponse.styles.map((s) => s.id);

      this.#collectionSelect.replaceChildren(
        ...this.#collections.map((c) => {
          const option = document.createElement('option');
          option.value = c.id;
          option.textContent = c.id;
          return option;
        }),
      );
      this.#styleSelect.replaceChildren(
        ...this.#styleIds.map((id) => {
          const option = document.createElement('option');
          option.value = id;
          option.textContent = id;
          return option;
        }),
      );

      if (this.#collections.length === 0 || this.#styleIds.length === 0) {
        this.#status.textContent = 'no collections or no registered styles available';
        return;
      }
      if (this.#map?.loaded()) await this.#loadSelected();
    } catch (error) {
      this.#status.textContent = error instanceof Error ? error.message : String(error);
    }
  }

  async #loadSelected(): Promise<void> {
    const map = this.#map;
    if (!map) return;
    const collection = this.#collections.find((c) => c.id === this.#collectionSelect.value);
    const styleId = this.#styleSelect.value;
    if (!collection || !styleId) return;

    try {
      const styleDoc = await fetchStyleDocument(styleId);
      this.#styleName.textContent =
        typeof styleDoc.name === 'string' ? `style: ${styleDoc.name}` : `style: ${styleId}`;

      if (map.getLayer('styled-raster')) map.removeLayer('styled-raster');
      if (map.getSource(SOURCE_ID)) map.removeSource(SOURCE_ID);

      map.addSource(SOURCE_ID, {
        type: 'raster',
        tiles: [
          buildStyledTileUrlTemplate(
            '',
            DEFAULT_TENANT_ID,
            DEFAULT_CATALOG_ID,
            collection.id,
            styleId,
          ),
        ],
        tileSize: 256,
      });
      map.addLayer({ id: 'styled-raster', type: 'raster', source: SOURCE_ID });

      fitToExtent(map, collection.extent);
      this.#status.textContent = `serving "${styleId}"-styled tiles for "${collection.id}"`;
    } catch (error) {
      this.#status.textContent = error instanceof Error ? error.message : String(error);
    }
  }
}

customElements.define('tellurion-styled-panel', TellurionStyledPanel);
