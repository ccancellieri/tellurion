/** @vitest-environment happy-dom */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const map = vi.hoisted(() => ({
  addLayer: vi.fn(),
  addSource: vi.fn(),
  fitBounds: vi.fn(),
  getLayer: vi.fn(),
  getSource: vi.fn(),
  isStyleLoaded: vi.fn(),
  on: vi.fn(),
  remove: vi.fn(),
  removeLayer: vi.fn(),
  removeSource: vi.fn(),
}));

vi.mock('../lib/map', () => ({
  createMap: vi.fn(() => map),
  setDemoImageRequestLimit: vi.fn(),
  demoRasterMapHandoff: vi.fn((source: { id: string; attribution: string; extent: unknown }) => ({
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    template: `https://tellurion.example/demo/sources/${source.id}/tiles/WebMercatorQuad/{z}/{y}/{x}.png`,
    extent: source.extent,
    attribution: source.attribution,
  })),
  demoVectorMapHandoff: vi.fn((source: { id: string; attribution: string; extent: [number, number, number, number]; geometryType: string }) => ({
    sourceId: `demo-source-${source.id}`,
    layerId: `demo-layer-${source.id}`,
    sourceLayer: source.id,
    template: `https://tellurion.example/demo/sources/${source.id}/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt`,
    extent: source.extent,
    attribution: source.attribution,
    geometryType: source.geometryType,
  })),
  fitToExtent: vi.fn(),
}));

import './demo-map-viewer';
import { fitToExtent, setDemoImageRequestLimit } from '../lib/map';

const source = {
  id: 'opaque-source',
  format: 'tiled-geotiff',
  transport: 'range-native',
  revision: 'strong',
  capability_state: 'ready',
  extent: [12, 39, 15, 42] as [number, number, number, number],
  geometryType: null,
  srid: null,
  numberMatched: null,
  properties: [],
  attribution: 'Verified attribution',
  limits: { expires_in_seconds: 900, max_live_sources: 3, max_concurrent_operations: 2 },
  links: {
    self_href: '/demo/sources/opaque-source',
    tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png',
  },
};

function mount(): HTMLElement {
  const element = document.createElement('tellurion-demo-map-viewer');
  document.body.append(element);
  return element;
}

beforeEach(() => {
  Object.values(map).forEach((mock) => mock.mockReset());
  vi.mocked(fitToExtent).mockReset();
  vi.mocked(setDemoImageRequestLimit).mockReset();
  map.isStyleLoaded.mockReturnValue(true);
  map.getLayer.mockReturnValue(undefined);
  map.getSource.mockReturnValue(undefined);
});

afterEach(() => {
  document.querySelectorAll('tellurion-demo-map-viewer').forEach((element) => element.remove());
  document.body.replaceChildren();
});

describe('temporary demo map viewer', () => {
  it('maps the same-origin handoff and reports its attribution', () => {
    const element = mount();

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 0.7 } }));

    expect(map.addSource).toHaveBeenCalledWith('demo-source-opaque-source', {
      type: 'raster',
      tiles: ['https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.png'],
      tileSize: 256,
      minzoom: 0,
      maxzoom: 22,
    });
    expect(setDemoImageRequestLimit).toHaveBeenCalledWith(2);
    expect(vi.mocked(setDemoImageRequestLimit).mock.invocationCallOrder[0]).toBeLessThan(
      map.addSource.mock.invocationCallOrder[0],
    );
    expect(map.addLayer).toHaveBeenCalledWith({
      id: 'demo-layer-opaque-source',
      type: 'raster',
      source: 'demo-source-opaque-source',
      paint: { 'raster-opacity': 0.7 },
    });
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[12, 39, 15, 42]], crs: 'EPSG:4326' } });
    expect(element.textContent).toContain('Verified attribution');
    expect(element.textContent).toContain('Temporary source map opened');
  });

  it('maps a valid source whose upstream metadata has no geographic extent', () => {
    mount();

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
      detail: { source: { ...source, extent: null }, opacity: 1 },
    }));

    expect(map.addSource).toHaveBeenCalledOnce();
    expect(fitToExtent).not.toHaveBeenCalled();
  });

  it('removes only the requested temporary source and restores the empty state', () => {
    const element = mount();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    map.getLayer.mockReturnValue({});
    map.getSource.mockReturnValue({});

    document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', { detail: { sourceId: source.id } }));

    expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
    expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
    expect(element.textContent).toContain('Choose a public HTTPS source');
  });

  it('does not cancel the active expiry timer for an unrelated reset event', () => {
    vi.useFakeTimers();
    try {
      mount();
      document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
        detail: { source: { ...source, limits: { ...source.limits, expires_in_seconds: 2 } }, opacity: 1 },
      }));
      map.getLayer.mockReturnValue({});
      map.getSource.mockReturnValue({});

      document.dispatchEvent(new CustomEvent('tellurion-demo-map-reset', {
        detail: { sourceId: 'another-source' },
      }));
      vi.advanceTimersByTime(2_000);

      expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
      expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
    } finally {
      vi.useRealTimers();
    }
  });

  it('releases its map and document listeners when disconnected', () => {
    const element = mount();
    element.remove();

    expect(map.remove).toHaveBeenCalledOnce();
    document.dispatchEvent(new CustomEvent('tellurion-demo-map', { detail: { source, opacity: 1 } }));
    expect(map.addSource).not.toHaveBeenCalled();
  });

  it('uses the advertised MVT template, opaque source layer, selected style, and extent for a vector source', () => {
    mount();
    const vector = {
      ...source,
      format: 'geoparquet',
      geometryType: 'Polygon',
      srid: 4326,
      numberMatched: 7,
      properties: ['boundary_id'],
      links: {
        ...source.links,
        items_href: '/demo/sources/opaque-source/items',
        item_template: '/demo/sources/opaque-source/items/{featureId}',
        mvt_tile_template: '/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt',
      },
    };

    document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
      detail: { source: vector, opacity: 0.8, style: 'survey-ink' },
    }));

    expect(map.addSource).toHaveBeenCalledWith('demo-source-opaque-source', {
      type: 'vector',
      tiles: ['https://tellurion.example/demo/sources/opaque-source/tiles/WebMercatorQuad/{z}/{y}/{x}.mvt'],
      minzoom: 0,
      maxzoom: 22,
    });
    expect(map.addLayer).toHaveBeenCalledWith(expect.objectContaining({
      type: 'fill', source: 'demo-source-opaque-source', 'source-layer': 'opaque-source',
      paint: expect.objectContaining({ 'fill-color': '#2e6970', 'fill-opacity': 0.8 }),
    }));
    expect(fitToExtent).toHaveBeenCalledWith(map, { spatial: { bbox: [[12, 39, 15, 42]], crs: 'EPSG:4326' } });
  });

  it('removes the temporary layer when the server-reported lifetime expires', () => {
    vi.useFakeTimers();
    try {
      const element = mount();
      const expired = vi.fn();
      document.addEventListener('tellurion-demo-source-expired', expired, { once: true });
      document.dispatchEvent(new CustomEvent('tellurion-demo-map', {
        detail: { source: { ...source, limits: { ...source.limits, expires_in_seconds: 2 } }, opacity: 1 },
      }));
      map.getLayer.mockReturnValue({});
      map.getSource.mockReturnValue({});

      vi.advanceTimersByTime(2_000);

      expect(map.removeLayer).toHaveBeenCalledWith('demo-layer-opaque-source');
      expect(map.removeSource).toHaveBeenCalledWith('demo-source-opaque-source');
      expect(element.textContent).toContain('Temporary source expired');
      expect(expired).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });
});
