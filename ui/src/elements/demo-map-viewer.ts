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
  vector: boolean;
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
  #clickListener = (event: { point: { x: number; y: number } }): void => this.#inspect(event.point);

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
        <div class="demo-map__viewport">
          <div class="demo-map__canvas" data-field="map" aria-label="Temporary source map"></div>
          <div class="demo-map__empty" data-field="empty">
            <h3>A map starts with a source</h3>
            <p>Choose a sample dataset or paste a public HTTPS address. Inspect the source, choose its appearance, then open your map here.</p>
            <p>COG · GeoParquet · ZIP Shapefile</p>
          </div>
        </div>
        <div class="demo-map__inspect" data-field="inspect-controls" hidden>
          <div class="demo-map__inspect-actions">
            <p>Click a feature, or pan the map and inspect its center.</p>
            <button type="button" data-field="inspect-center">Inspect map center</button>
          </div>
          <p data-field="inspect-status" role="status"></p>
          <section data-field="feature-details" aria-labelledby="demo-feature-title" hidden>
            <div class="demo-map__inspect-actions">
              <h3 id="demo-feature-title">Tile feature details</h3>
              <button type="button" data-field="close-details">Close details</button>
            </div>
            <p>Rendered tile attributes and geometry may be simplified or incomplete compared with the original feature.</p>
            <p>Tile feature ID: <span data-field="feature-id"></span></p>
            <dl class="demo-map__properties" data-field="feature-properties"></dl>
          </section>
        </div>
        <p class="demo-map__attribution" data-field="attribution">Basemap intentionally omitted.</p>
      </section>
    `;
    document.addEventListener('tellurion-demo-map', this.#mapListener);
    document.addEventListener('tellurion-demo-map-reset', this.#resetListener);
    this.#tileTransport = createDemoTileTransport();
    this.#map = createMap(this.#field('map'));
    this.#map.on('click', this.#clickListener);
    this.#field('inspect-center').onclick = () => {
      const canvas = this.#map?.getCanvas();
      if (canvas) this.#inspect({ x: canvas.clientWidth / 2, y: canvas.clientHeight / 2 });
    };
    this.#field('close-details').onclick = () => {
      this.#clearInspection();
      this.#field('inspect-center').focus();
    };
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
    this.#clearInspection();
    this.#field('inspect-controls').hidden = true;
    this.#map?.off('click', this.#clickListener);
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
      this.#registration = { sourceId: handoff.sourceId, layerId: handoff.layerId, vector: true };
      this.#field('inspect-controls').hidden = false;
      this.#field('empty').hidden = true;
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
    this.#registration = { sourceId: handoff.sourceId, layerId: handoff.layerId, vector: false };
    this.#field('empty').hidden = true;
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
    this.#clearInspection();
    this.#field('inspect-controls').hidden = true;
    this.#tileTransport?.clear();
    const map = this.#map;
    if (map) {
      if (map.getLayer(registration.layerId)) map.removeLayer(registration.layerId);
      if (map.getSource(registration.sourceId)) map.removeSource(registration.sourceId);
    }
    this.#registration = null;
    this.#field('empty').hidden = false;
    this.#field('status').textContent = 'Choose a public HTTPS source to open a temporary layer.';
    this.#field('attribution').textContent = 'Basemap intentionally omitted.';
  }

  #field(name: string): HTMLElement {
    const field = this.querySelector<HTMLElement>(`[data-field="${name}"]`);
    if (!field) throw new Error(`demo map viewer is missing its ${name} field`);
    return field;
  }

  #clearInspection(): void {
    this.#field('feature-details').hidden = true;
    this.#field('feature-properties').replaceChildren();
    this.#field('feature-id').textContent = '';
    this.#field('inspect-status').textContent = '';
  }

  #inspect(point: { x: number; y: number }): void {
    const registration = this.#registration;
    if (!this.#map || !registration?.vector) return;
    this.#clearInspection();
    const feature = this.#map.queryRenderedFeatures(
      [[point.x - 4, point.y - 4], [point.x + 4, point.y + 4]],
      { layers: [registration.layerId] },
    )[0];
    if (!feature) {
      this.#field('inspect-status').textContent = 'No rendered feature here. Click a visible feature or pan the map and try again.';
      return;
    }
    let truncated = false;
    const boundedText = (value: unknown): string => {
      if (typeof value === 'number' && Number.isInteger(value) && !Number.isSafeInteger(value)) {
        return `${value} (approximate: exceeds JavaScript safe integer precision)`;
      }
      // MVT attributes are scalar values. Do not recursively expand unexpected objects.
      const text = value !== null && typeof value === 'object' ? '[Structured value omitted]' : String(value);
      if (text.length <= 1024) return text;
      truncated = true;
      return `${text.slice(0, 1024)}…`;
    };
    const properties = feature.properties ?? {};
    const attributeId = typeof properties.id === 'string' || typeof properties.id === 'number' ? properties.id : undefined;
    this.#field('feature-id').textContent = boundedText(feature.id ?? attributeId ?? 'Not available in this tile');
    const list = this.#field('feature-properties');
    let count = 0;
    for (const key in properties) {
      if (!Object.hasOwn(properties, key)) continue;
      if (count === 64) { truncated = true; break; }
      const name = document.createElement('dt');
      const value = document.createElement('dd');
      name.textContent = boundedText(key);
      value.textContent = boundedText(properties[key]);
      list.append(name, value);
      count += 1;
    }
    this.#field('feature-details').hidden = false;
    this.#field('inspect-status').textContent = truncated
      ? 'Tile feature selected. Display truncated: at most 64 properties and 1,024 characters per name, value or ID.'
      : count === 0 ? 'Tile feature selected. No attributes available in this tile.' : 'Tile feature selected.';
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
