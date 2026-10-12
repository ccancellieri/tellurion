import * as maplibregl from 'maplibre-gl';
import { MapLibreOverlay } from '@deck.gl/maplibre';
import { Tile3DLayer } from '@deck.gl/geo-layers';
import {
  DEFAULT_CATALOG_ID,
  DEFAULT_TENANT_ID,
  fetchDefaultCollections,
  type CollectionSummary,
} from '../lib/api';
import { createMap, fitToExtent } from '../lib/map';
import { createEmptyTileAdapter } from '../lib/places3d-empty-content';
import {
  buildViewportTileset,
  type ViewportTilesetResult,
} from '../lib/places3d-manifest';

const LAYER_ID = 'tellurion-3d-places';

/**
 * `<tellurion-places3d-panel>` — deck.gl's `Tile3DLayer` pointed at the 3D
 * Tiles 1.1 `tileset.json` the server serves at
 * `/{tenant}/3dtiles/catalogs/{catalog}/collections/{cid}/3dtiles`, composited through `MapLibreOverlay` in
 * *interleaved* mode: it shares MapLibre's own WebGL2 context and camera
 * rather than opening a second canvas/engine, per the issue's design (the
 * whole reason deck.gl was picked for this lane over a standalone
 * three.js/Cesium viewer).
 *
 * Tellurion's constant-availability implicit root is converted into a finite
 * explicit manifest for the current view before it reaches `Tile3DLayer`.
 * This keeps loaders.gl from materializing the complete quadtree up front.
 */
export class TellurionPlaces3dPanel extends HTMLElement {
  #map: maplibregl.Map | null = null;
  #overlay: MapLibreOverlay | null = null;
  #select!: HTMLSelectElement;
  #status!: HTMLElement;
  #collections: CollectionSummary[] = [];
  #request: AbortController | null = null;
  #manifestUrl: string | null = null;
  #generation = 0;

  readonly #handleMapLoad = (): void => {
    void this.#loadSelected(true);
  };

  readonly #handleMoveEnd = (): void => {
    void this.#loadSelected(false);
  };

  readonly #handleCollectionChange = (): void => {
    void this.#loadSelected(true);
  };

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
    this.#overlay = new MapLibreOverlay({ interleaved: true, layers: [] });
    this.#map.addControl(this.#overlay);
    this.#map.addControl(new maplibregl.NavigationControl(), 'top-right');
    this.#map.on('load', this.#handleMapLoad);
    this.#map.on('moveend', this.#handleMoveEnd);

    this.#select.addEventListener('change', this.#handleCollectionChange);

    void this.#init();
  }

  disconnectedCallback(): void {
    this.#generation += 1;
    this.#request?.abort();
    this.#request = null;
    this.#revokeManifest();
    this.#select?.removeEventListener('change', this.#handleCollectionChange);
    this.#map?.off('load', this.#handleMapLoad);
    this.#map?.off('moveend', this.#handleMoveEnd);
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
      if (this.#map?.loaded()) void this.#loadSelected(true);
    } catch (error) {
      this.#status.textContent = error instanceof Error ? error.message : String(error);
    }
  }

  #revokeManifest(): void {
    if (!this.#manifestUrl) return;
    URL.revokeObjectURL(this.#manifestUrl);
    this.#manifestUrl = null;
  }

  #clearLayer(): void {
    this.#overlay?.setProps({ layers: [] });
    this.#revokeManifest();
  }

  async #loadSelected(fitCollection: boolean): Promise<void> {
    const map = this.#map;
    const overlay = this.#overlay;
    if (!map || !overlay) return;
    const collection = this.#collections.find((c) => c.id === this.#select.value);
    if (!collection) return;

    if (fitCollection) fitToExtent(map, collection.extent);
    this.#status.textContent = `loading 3D tiles for "${collection.id}"…`;

    const generation = this.#generation + 1;
    this.#generation = generation;
    this.#request?.abort();
    const request = new AbortController();
    this.#request = request;
    this.#clearLayer();

    const rootUrl = `/${DEFAULT_TENANT_ID}/3dtiles/catalogs/${DEFAULT_CATALOG_ID}/collections/${encodeURIComponent(collection.id)}/3dtiles`;
    try {
      const response = await fetch(rootUrl, {
        headers: { Accept: 'application/json' },
        signal: request.signal,
      });
      if (!response.ok) {
        throw new Error(`3D tileset request failed: ${response.status} ${response.statusText}`);
      }
      const tileset = await response.json() as unknown;
      if (generation !== this.#generation || request.signal.aborted) return;

      const root = tileset && typeof tileset === 'object'
        ? (tileset as { root?: unknown }).root
        : undefined;
      const version = tileset && typeof tileset === 'object'
        ? (tileset as { asset?: { version?: unknown } }).asset?.version
        : undefined;
      if (!root || typeof root !== 'object' || version !== '1.1') {
        throw new Error('Invalid 3D tileset: asset 1.1 and root are required');
      }

      let data = rootUrl;
      let emptyAdapter: ReturnType<typeof createEmptyTileAdapter> | undefined;
      if ('implicitTiling' in root) {
        const bbox = collection.extent?.spatial.bbox[0];
        if (!bbox) throw new Error('Invalid 3D tileset: collection extent is required');
        const bounds = map.getBounds();
        const result: ViewportTilesetResult = buildViewportTileset({
          tileset,
          rootUrl,
          pageUrl: window.location.href,
          collectionExtent: bbox,
          viewport: [
            bounds.getWest(),
            bounds.getSouth(),
            bounds.getEast(),
            bounds.getNorth(),
          ],
          requestedZoom: map.getZoom(),
        });
        if (result.kind === 'too-many') {
          this.#status.textContent = `zoom closer to view at most ${result.maxTiles} 3D tiles (${result.tileCount} requested)`;
          return;
        }
        if (result.kind === 'too-coarse') {
          this.#status.textContent = `zoom closer to level ${result.minimumZoom} for accurate 3D placement`;
          return;
        }
        if (result.kind === 'empty') {
          this.#status.textContent = `no 3D tiles in view for "${collection.id}"`;
          return;
        }
        const blob = new Blob([JSON.stringify(result.tileset)], { type: 'application/json' });
        emptyAdapter = createEmptyTileAdapter(
          result.tileset.root.children.map((child) => child.content.uri), request.signal,
        );
        data = URL.createObjectURL(blob);
        this.#manifestUrl = data;
      }

      if (generation !== this.#generation || request.signal.aborted) {
        if (data.startsWith('blob:')) URL.revokeObjectURL(data);
        return;
      }
      const layer = new Tile3DLayer({
        id: LAYER_ID,
        data,
        loadOptions: {
          '3d-tiles': { isTileset: true },
          ...(emptyAdapter ? {
            core: { fetch: emptyAdapter.fetch },
            tileset: { contentLoader: emptyAdapter.contentLoader },
          } : {}),
        },
        onTilesetLoad: () => {
          if (generation === this.#generation) {
            this.#status.textContent = `serving 3D places for "${collection.id}"`;
          }
        },
        onTileError: (_tile, message) => {
          if (generation === this.#generation) {
            this.#status.textContent = `3D tile failed to load: ${message}`;
          }
        },
      });
      overlay.setProps({ layers: [layer] });
    } catch (error) {
      if (generation !== this.#generation || request.signal.aborted) return;
      this.#clearLayer();
      this.#status.textContent = error instanceof Error ? error.message : String(error);
    }
  }
}

customElements.define('tellurion-places3d-panel', TellurionPlaces3dPanel);
