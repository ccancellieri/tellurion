import '../demo-map-viewer.css';
import {
  createMap,
  createDemoTileTransport,
  demoRasterMapHandoff,
  demoVectorMapHandoff,
  fitToExtent,
} from '../lib/map';
import type { AddLayerObject } from 'maplibre-gl';
import { isVectorDemoSource, type DemoSourceResponse, type DemoVectorStyle } from '../lib/demo-source';

const ElementBase: typeof HTMLElement =
  globalThis.HTMLElement ?? (class {} as unknown as typeof HTMLElement);

interface MapRegistration {
  sourceId: string;
  layerId: string;
}

/** A temporary, demo-only map surface. It consumes only the server-issued
 * same-origin handoff and never knows a tenant, catalog, or remote locator. */
export class TellurionDemoMapViewer extends ElementBase {
  #map: ReturnType<typeof createMap> | null = null;
  #tileTransport: ReturnType<typeof createDemoTileTransport> | null = null;
  #registration: MapRegistration | null = null;
  #pending: { source: DemoSourceResponse; opacity: number; style: DemoVectorStyle } | null = null;
  #expiryTimer: ReturnType<typeof setTimeout> | null = null;
  #mapListener = (event: Event): void => this.#receiveMap(event);
  #resetListener = (event: Event): void => this.#receiveReset(event);

  connectedCallback(): void {
    this.innerHTML = `
      <section class="demo-map" aria-labelledby="demo-map-title">
        <header class="demo-map__heading">
          <div>
            <p class="demo-map__eyebrow">Temporary map</p>
            <h2 id="demo-map-title">Remote source preview</h2>
          </div>
          <p class="demo-map__status" data-field="status" role="status">Choose a public HTTPS source to open a temporary layer.</p>
        </header>
        <div class="demo-map__canvas" data-field="map" aria-label="Temporary source map"></div>
        <p class="demo-map__attribution" data-field="attribution">Basemap intentionally omitted.</p>
      </section>
    `;
    document.addEventListener('tellurion-demo-map', this.#mapListener);
    document.addEventListener('tellurion-demo-map-reset', this.#resetListener);
    this.#tileTransport = createDemoTileTransport();
    this.#map = createMap(this.#field('map'));
    this.#map.on('load', () => {
      if (this.#pending) this.#open(this.#pending.source, this.#pending.opacity, this.#pending.style);
    });
  }

  disconnectedCallback(): void {
    document.removeEventListener('tellurion-demo-map', this.#mapListener);
    document.removeEventListener('tellurion-demo-map-reset', this.#resetListener);
    this.#clearExpiry();
    this.#pending = null;
    this.#registration = null;
    this.#map?.remove();
    this.#map = null;
    this.#tileTransport?.destroy();
    this.#tileTransport = null;
  }

  #receiveMap(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const detail = event.detail;
    if (!detail || typeof detail !== 'object') return;
    const source = (detail as { source?: unknown }).source;
    const opacity = (detail as { opacity?: unknown }).opacity;
    const style = (detail as { style?: unknown }).style;
    if (!isDemoSourceResponse(source) || typeof opacity !== 'number' || !Number.isFinite(opacity)) return;
    this.#pending = {
      source,
      opacity: Math.max(0, Math.min(1, opacity)),
      style: style === 'coastline-signal' ? style : 'survey-ink',
    };
    this.#open(source, this.#pending.opacity, this.#pending.style);
  }

  #receiveReset(event: Event): void {
    if (!(event instanceof CustomEvent)) return;
    const sourceId = (event.detail as { sourceId?: unknown } | null)?.sourceId;
    if (typeof sourceId !== 'string') return;
    if (this.#pending?.source.id === sourceId) {
      this.#pending = null;
      this.#clearExpiry();
    }
    this.#remove(sourceId);
  }

  #open(source: DemoSourceResponse, opacity: number, style: DemoVectorStyle): void {
    const map = this.#map;
    if (!map || !map.isStyleLoaded()) return;
    this.#clearExpiry();
    this.#remove();
    if (isVectorDemoSource(source)) {
      const handoff = demoVectorMapHandoff(source, location.origin);
      if (!handoff) return;
      map.addSource(handoff.sourceId, { type: 'vector', tiles: [handoff.template], minzoom: 0, maxzoom: 22 });
      map.addLayer(vectorLayer(handoff, opacity, style));
      this.#registration = { sourceId: handoff.sourceId, layerId: handoff.layerId };
      fitToExtent(map, { spatial: { bbox: [handoff.extent], crs: 'EPSG:4326' } });
      this.#field('status').textContent = 'Temporary vector map opened. It expires with this browser session.';
      this.#field('attribution').textContent = `Temporary source: ${handoff.attribution}`;
      this.#scheduleExpiry(source);
      return;
    }
    const handoff = demoRasterMapHandoff(source, location.origin);
    if (!handoff) return;
    const tileTemplate = this.#tileTransport?.activate(
      source.id,
      location.origin,
      handoff.template,
      source.limits.max_concurrent_operations,
    );
    if (!tileTemplate) return;
    map.addSource(handoff.sourceId, { type: 'raster', tiles: [tileTemplate], tileSize: 256, minzoom: 0, maxzoom: 22 });
    map.addLayer({ id: handoff.layerId, type: 'raster', source: handoff.sourceId, paint: { 'raster-opacity': opacity } });
    this.#registration = { sourceId: handoff.sourceId, layerId: handoff.layerId };
    if (handoff.extent) fitToExtent(map, { spatial: { bbox: [handoff.extent], crs: 'EPSG:4326' } });
    this.#field('status').textContent = 'Temporary source map opened. It expires with this browser session.';
    this.#field('attribution').textContent = `Temporary source: ${handoff.attribution}`;
    this.#scheduleExpiry(source);
  }

  #scheduleExpiry(source: DemoSourceResponse): void {
    this.#clearExpiry();
    this.#expiryTimer = setTimeout(() => {
      this.#expiryTimer = null;
      if (this.#pending?.source.id !== source.id) return;
      this.#pending = null;
      this.#remove(source.id);
      this.#field('status').textContent = 'Temporary source expired. Choose a public HTTPS source to continue.';
      document.dispatchEvent(new CustomEvent('tellurion-demo-source-expired', {
        detail: { sourceId: source.id },
      }));
    }, source.limits.expires_in_seconds * 1_000);
  }

  #clearExpiry(): void {
    if (this.#expiryTimer !== null) clearTimeout(this.#expiryTimer);
    this.#expiryTimer = null;
  }

  #remove(sourceId?: string): void {
    const registration = this.#registration;
    if (!registration || (sourceId && registration.sourceId !== `demo-source-${sourceId}`)) return;
    this.#tileTransport?.clear();
    const map = this.#map;
    if (map) {
      if (map.getLayer(registration.layerId)) map.removeLayer(registration.layerId);
      if (map.getSource(registration.sourceId)) map.removeSource(registration.sourceId);
    }
    this.#registration = null;
    this.#field('status').textContent = 'Choose a public HTTPS source to open a temporary layer.';
    this.#field('attribution').textContent = 'Basemap intentionally omitted.';
  }

  #field(name: string): HTMLElement {
    const field = this.querySelector<HTMLElement>(`[data-field="${name}"]`);
    if (!field) throw new Error(`demo map viewer is missing its ${name} field`);
    return field;
  }
}

function isDemoSourceResponse(value: unknown): value is DemoSourceResponse {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Partial<DemoSourceResponse>;
  const validExtent =
    candidate.extent === null ||
    (Array.isArray(candidate.extent) &&
      candidate.extent.length === 4 &&
      candidate.extent.every((coordinate) =>
        typeof coordinate === 'number' && Number.isFinite(coordinate)));
  const links = candidate.links;
  const expiresInSeconds = candidate.limits?.expires_in_seconds;
  const vector = candidate.format === 'geoparquet' || candidate.format === 'shapefile-zip';
  return validExtent &&
    typeof candidate.id === 'string' &&
    (candidate.format === 'tiled-geotiff' || vector) &&
    typeof candidate.attribution === 'string' &&
    Array.isArray(candidate.properties) &&
    Number.isInteger(expiresInSeconds) && expiresInSeconds! > 0 && expiresInSeconds! <= 15 * 60 &&
    typeof links?.tile_template === 'string' &&
    (!vector || (typeof candidate.geometryType === 'string' && candidate.srid === 4326 &&
      typeof candidate.numberMatched === 'number' && typeof links.mvt_tile_template === 'string'));
}

function vectorLayer(
  handoff: NonNullable<ReturnType<typeof demoVectorMapHandoff>>,
  opacity: number,
  style: DemoVectorStyle,
): AddLayerObject {
  const color = style === 'coastline-signal' ? '#d85f43' : '#2e6970';
  const geometry = handoff.geometryType.toLowerCase();
  const base = { id: handoff.layerId, source: handoff.sourceId, 'source-layer': handoff.sourceLayer };
  if (geometry.includes('point')) return { ...base, type: 'circle', paint: { 'circle-color': color, 'circle-radius': 4, 'circle-opacity': opacity } };
  if (geometry.includes('line')) return { ...base, type: 'line', paint: { 'line-color': color, 'line-width': 2, 'line-opacity': opacity } };
  return { ...base, type: 'fill', paint: { 'fill-color': color, 'fill-opacity': opacity, 'fill-outline-color': color } };
}

if (globalThis.customElements && !globalThis.customElements.get('tellurion-demo-map-viewer')) {
  globalThis.customElements.define('tellurion-demo-map-viewer', TellurionDemoMapViewer);
}
