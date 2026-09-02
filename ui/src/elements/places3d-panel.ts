import maplibregl from 'maplibre-gl';
import { MapboxOverlay } from '@deck.gl/mapbox';
import { Tile3DLayer } from '@deck.gl/geo-layers';
import {
  DEFAULT_CATALOG_ID,
  DEFAULT_TENANT_ID,
  fetchDefaultCollections,
  type CollectionSummary,
} from '../lib/api';
import { createMap, fitToExtent } from '../lib/map';

const LAYER_ID = 'tellurion-3d-places';

/**
 * `<tellurion-places3d-panel>` — deck.gl's `Tile3DLayer` pointed at the 3D
 * Tiles 1.1 `tileset.json` the server serves at
 * `/{tenant}/3dtiles/catalogs/{catalog}/collections/{cid}/3dtiles`, composited through `MapboxOverlay` in
 * *interleaved* mode: it shares MapLibre's own WebGL2 context and camera
 * rather than opening a second canvas/engine, per the issue's design (the
 * whole reason deck.gl was picked for this lane over a standalone
 * three.js/Cesium viewer).
 *
 * `Tile3DLayer`'s default loader (`Tiles3DLoader` from `@loaders.gl/3d-tiles`)
 * is left unset here deliberately — it already covers 3D Tiles 1.1 implicit
 * tiling, which is exactly what the server's `tileset_json` emits.
 */
export class TellurionPlaces3dPanel extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #overlay: MapboxOverlay | null = null;
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
    this.#map.setPitch(45);
    this.#overlay = new MapboxOverlay({ interleaved: true, layers: [] });
    // MapboxOverlay implements the mapbox-gl `IControl` shape, which
    // maplibre-gl's own (structurally near-identical) `IControl` type
    // doesn't nominally match — this is deck.gl's documented interleaving
    // pattern with MapLibre, not a real type mismatch at runtime.
    this.#map.addControl(this.#overlay as unknown as maplibregl.IControl);
    this.#map.on('load', () => this.#loadSelected());

    this.#select.addEventListener('change', () => this.#loadSelected());

    void this.#init();
  }

  disconnectedCallback(): void {
    this.#map?.remove();
    this.#map = null;
    this.#overlay = null;
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
    const overlay = this.#overlay;
    if (!map || !overlay) return;
    const collection = this.#collections.find((c) => c.id === this.#select.value);
    if (!collection) return;

    this.#status.textContent = `loading 3D tiles for "${collection.id}"…`;
    fitToExtent(map, collection.extent);

    const layer = new Tile3DLayer({
      id: LAYER_ID,
      data: `/${DEFAULT_TENANT_ID}/3dtiles/catalogs/${DEFAULT_CATALOG_ID}/collections/${encodeURIComponent(collection.id)}/3dtiles`,
      onTilesetLoad: () => {
        this.#status.textContent = `serving 3D places for "${collection.id}"`;
      },
      onTileError: (_tile, _url, message) => {
        this.#status.textContent = `3D tiles error (collection may not declare places3d): ${message}`;
      },
    });
    overlay.setProps({ layers: [layer] });
  }
}

customElements.define('tellurion-places3d-panel', TellurionPlaces3dPanel);
